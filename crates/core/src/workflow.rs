//! Running a [`Workflow`]: its steps in order, feeding each step the
//! variables captured by the ones before it, and repeating looped steps
//! once per item of a list.

use serde_json::Value;

use crate::engine::{apply_outcome, execute, Outcome};
use crate::extract::{json_path, value_text};
use crate::http::Transport;
use crate::model::{Catalog, ForEach, KeyValue, VarScope, Workflow};
use crate::vars::Vars;

#[derive(Debug, Clone)]
pub struct StepReport {
    pub iteration: u32,
    /// Position in the workflow (0-based).
    pub index: usize,
    pub step_id: String,
    pub request_id: String,
    pub name: String,
    /// Set when the step ran inside a loop.
    pub item: Option<LoopItem>,
    /// `None` when the step was skipped.
    pub outcome: Option<Outcome>,
    pub skipped: Option<String>,
}

/// Which pass of a loop a step report belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopItem {
    /// From 0.
    pub index: usize,
    pub count: usize,
    /// A short form of the item, for display.
    pub label: String,
}

impl StepReport {
    pub fn passed(&self) -> bool {
        self.outcome.as_ref().map(|o| o.passed()).unwrap_or(true)
    }
}

#[derive(Debug, Clone, Default)]
pub struct RunSummary {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub stopped_early: bool,
    pub elapsed_ms: f64,
}

/// The items a loop runs over: the list variable (a JSON array, or lines
/// of text), narrowed by the optional JSON path. A path may contain one
/// `[*]` (or `.*`) to collect a field from every element.
pub fn loop_items(raw: &str, path: &str) -> Result<Vec<Value>, String> {
    let raw = raw.trim();
    let value: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) if path.trim().is_empty() => {
            // Plain text: one item per line, or comma separated on one line.
            let parts: Vec<&str> = if raw.contains('\n') { raw.lines().collect() } else { raw.split(',').collect() };
            return Ok(parts.into_iter().map(str::trim).filter(|s| !s.is_empty()).map(|s| Value::String(s.into())).collect());
        }
        Err(_) => return Err("the list isn't JSON, so a JSON path can't be applied".into()),
    };
    let path = path.trim();
    let (head, tail) = match path.find("[*]").map(|i| (i, 3)).or_else(|| path.find(".*").map(|i| (i, 2))) {
        Some((i, len)) => (&path[..i], Some(&path[i + len..])),
        None => (path, None),
    };
    let list = if head.is_empty() || head == "$" {
        value.clone()
    } else {
        json_path(&value, head).map(|v| v.into_owned()).ok_or_else(|| format!("nothing at {head}"))?
    };
    let items = match list {
        Value::Array(a) => a,
        Value::Null => Vec::new(),
        other => return Err(format!("expected a list but found {}", short(&value_text(&other), 60))),
    };
    Ok(match tail.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => items.iter().filter_map(|i| json_path(i, &format!("${t}")).map(|v| v.into_owned())).collect(),
        None => items,
    })
}

/// The variables one item provides: `{{item}}`, `{{item.field}}` (nested
/// fields too, a few levels deep), `{{$index}}` and `{{$count}}`.
pub fn item_variables(name: &str, item: &Value, index: usize, count: usize) -> Vec<(String, String)> {
    fn walk(prefix: &str, v: &Value, depth: usize, out: &mut Vec<(String, String)>) {
        out.push((prefix.to_string(), value_text(v)));
        if depth >= 4 {
            return;
        }
        match v {
            Value::Object(o) => o.iter().for_each(|(k, x)| walk(&format!("{prefix}.{k}"), x, depth + 1, out)),
            Value::Array(a) => a.iter().enumerate().take(50).for_each(|(i, x)| walk(&format!("{prefix}.{i}"), x, depth + 1, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(name, item, 0, &mut out);
    out.push(("$index".into(), index.to_string()));
    out.push(("$count".into(), count.to_string()));
    out
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}\u{2026}", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}

/// What the run does after a step.
enum Flow {
    Continue,
    Stop,
}

struct Runner<'a> {
    wf: Workflow,
    cat: Catalog,
    summary: RunSummary,
    transport: &'a dyn Transport,
    on_step: &'a mut dyn FnMut(&StepReport) -> bool,
}

impl Runner<'_> {
    fn report(&mut self, report: StepReport) -> Flow {
        let ok = report.passed();
        match (&report.outcome, &report.skipped) {
            (_, Some(_)) => self.summary.skipped += 1,
            (Some(_), _) if ok => self.summary.passed += 1,
            _ => self.summary.failed += 1,
        }
        let keep_going = (self.on_step)(&report);
        if !keep_going || (!ok && self.wf.stop_on_failure) {
            self.summary.stopped_early = true;
            Flow::Stop
        } else {
            Flow::Continue
        }
    }

    fn blank_report(&self, iteration: u32, index: usize, item: Option<LoopItem>) -> StepReport {
        let step = &self.wf.steps[index];
        let name = self.cat.find_request(&step.request_id).map(|(_, r)| r.name.clone()).unwrap_or_else(|| "(deleted request)".into());
        StepReport { iteration, index, step_id: step.id.clone(), request_id: step.request_id.clone(), name, item, outcome: None, skipped: None }
    }

    fn workflow_vars(&self, iteration: u32) -> Vec<KeyValue> {
        let mut vars = self.wf.variables.clone();
        vars.push(KeyValue::new("$iteration", iteration.to_string()));
        vars
    }

    async fn step(&mut self, iteration: u32, index: usize, item: Option<LoopItem>) -> Flow {
        let mut report = self.blank_report(iteration, index, item);
        let step = self.wf.steps[index].clone();
        if !step.enabled {
            report.skipped = Some("disabled".into());
            return self.report(report);
        }
        if step.delay_ms > 0 {
            self.transport.sleep(step.delay_ms).await;
        }
        let vars = self.workflow_vars(iteration);
        let outcome = execute(&self.cat, &step.request_id, &vars, &step.extract, &step.checks, self.transport).await;
        apply_outcome(&mut self.cat, &outcome);
        report.outcome = Some(outcome);
        self.report(report)
    }

    /// Runs steps `index..end` once per item of the loop on step `index`.
    async fn loop_steps(&mut self, iteration: u32, index: usize, end: usize, fe: &ForEach) -> Flow {
        let first = self.wf.steps[index].request_id.clone();
        let vars = Vars::for_request(&self.cat, self.cat.collection_of(&first), &self.workflow_vars(iteration), self.transport.now_ms());
        let list = fe.list.trim().trim_start_matches("{{").trim_end_matches("}}").trim().to_string();
        let items = match vars.get(&list) {
            Some(raw) => loop_items(&raw, &fe.path),
            None => Err(format!("{{{{{list}}}}} has no value (capture it in an earlier step)")),
        };
        let items = match items {
            Ok(items) => items,
            Err(e) => {
                let mut report = self.blank_report(iteration, index, None);
                report.outcome = Some(Outcome {
                    request_id: report.request_id.clone(),
                    request_name: report.name.clone(),
                    error: Some(format!("loop: {e}")),
                    ..Default::default()
                });
                return self.report(report);
            }
        };
        if items.is_empty() {
            for i in index..end {
                let mut report = self.blank_report(iteration, i, None);
                report.skipped = Some(format!("{{{{{list}}}}} is an empty list"));
                if let Flow::Stop = self.report(report) {
                    return Flow::Stop;
                }
            }
            return Flow::Continue;
        }
        // Item values go in as runtime variables, so nothing overrides them;
        // whatever they shadow is put back afterwards.
        let name = if fe.item.trim().is_empty() { "item".to_string() } else { fe.item.trim().to_string() };
        let is_loop_var = |k: &str| k == name || k.starts_with(&format!("{name}.")) || k == "$index" || k == "$count";
        let shadowed: Vec<KeyValue> = self.cat.runtime.iter().filter(|kv| is_loop_var(&kv.key)).cloned().collect();
        let mut flow = Flow::Continue;
        'items: for (n, item) in items.iter().enumerate() {
            self.cat.runtime.retain(|kv| !is_loop_var(&kv.key));
            for (k, v) in item_variables(&name, item, n, items.len()) {
                self.cat.set_variable(VarScope::Runtime, &k, &v);
            }
            let label = LoopItem { index: n, count: items.len(), label: short(&value_text(item), 40) };
            for i in index..end {
                if let Flow::Stop = self.step(iteration, i, Some(label.clone())).await {
                    flow = Flow::Stop;
                    break 'items;
                }
            }
        }
        self.cat.runtime.retain(|kv| !is_loop_var(&kv.key));
        self.cat.runtime.extend(shadowed);
        flow
    }
}

/// Runs the workflow against a working copy of the catalog, calling
/// `on_step` after every step. The caller should apply each outcome to its
/// own catalog (with [`apply_outcome`]) to keep the captured values.
/// Returning `false` from `on_step` stops the run.
///
/// A step with a `for_each` runs, with the steps it covers, once per item
/// of its list. Loops don't nest: a loop inside another's steps is ignored.
pub async fn run(catalog: &Catalog, workflow_id: &str, transport: &dyn Transport, on_step: &mut dyn FnMut(&StepReport) -> bool) -> Result<RunSummary, String> {
    let wf = catalog.workflow(workflow_id).ok_or("workflow not found")?.clone();
    let started = transport.now_ms();
    let iterations = wf.iterations.max(1);
    let n = wf.steps.len();
    let mut r = Runner { wf, cat: catalog.clone(), summary: RunSummary::default(), transport, on_step };
    'outer: for iteration in 1..=iterations {
        let mut i = 0;
        while i < n {
            let fe = r.wf.steps[i].for_each.clone().filter(|_| r.wf.steps[i].enabled);
            let flow = match fe {
                Some(fe) => {
                    let end = (i + fe.steps.max(1) as usize).min(n);
                    let flow = r.loop_steps(iteration, i, end, &fe).await;
                    i = end;
                    flow
                }
                None => {
                    i += 1;
                    r.step(iteration, i - 1, None).await
                }
            };
            if let Flow::Stop = flow {
                break 'outer;
            }
        }
    }
    let mut summary = r.summary;
    summary.elapsed_ms = transport.now_ms().saturating_sub(started) as f64;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{reply, Scripted};
    use crate::model::*;
    use serde_json::json;

    #[test]
    fn passes_values_between_steps() {
        let mut cat = Catalog::default();
        let mut col = Collection::new("c");
        let mut login = Request::new("login", Protocol::Rest);
        login.method = "POST".into();
        login.url = "https://api/login".into();
        login.extract.push(Extraction {
            variable: "token".into(),
            source: ExtractSource::Json,
            expression: "$.token".into(),
            scope: VarScope::Runtime,
            enabled: true,
        });
        let mut me = Request::new("me", Protocol::Rest);
        me.url = "https://api/me".into();
        me.auth = Auth::Bearer { token: "{{token}}".into() };
        me.checks.push(Check { kind: CheckKind::Status, target: String::new(), expected: "200".into(), enabled: true });
        let mut wf = Workflow::new("flow");
        wf.steps.push(WorkflowStep::new(login.id.clone()));
        wf.steps.push(WorkflowStep::new(me.id.clone()));
        col.items.push(Item::Request(login));
        col.items.push(Item::Request(me));
        cat.collections.push(col);
        let wid = wf.id.clone();
        cat.workflows.push(wf);

        let t = Scripted::new(vec![reply(200, &[], r#"{"token":"abc"}"#), reply(500, &[], "boom")]);
        let mut reports = Vec::new();
        let s = pollster::block_on(run(&cat, &wid, &t, &mut |r| {
            reports.push(r.clone());
            true
        }))
        .unwrap();
        assert_eq!((s.passed, s.failed), (1, 1));
        assert!(t.sent.borrow()[1].headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer abc"));
        assert!(!reports[1].passed());
    }

    #[test]
    fn list_items() {
        let raw = r#"{"value":[{"id":1,"tags":["a"]},{"id":2}]}"#;
        assert_eq!(loop_items(raw, "$.value[*].id").unwrap(), vec![json!(1), json!(2)]);
        assert_eq!(loop_items(raw, "$.value").unwrap().len(), 2);
        assert_eq!(loop_items("[1,2,3]", "").unwrap().len(), 3);
        assert_eq!(loop_items("a, b,c", "").unwrap(), vec![json!("a"), json!("b"), json!("c")]);
        assert_eq!(loop_items("x\ny\n", "").unwrap(), vec![json!("x"), json!("y")]);
        assert!(loop_items(raw, "$.missing").is_err());
        assert!(loop_items(r#"{"a":1}"#, "").is_err());
        let vars = item_variables("p", &json!({"id": 7, "tags": ["a"], "who": {"name": "Ann"}}), 2, 5);
        let get = |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("p.id"), Some("7"));
        assert_eq!(get("p.tags.0"), Some("a"));
        assert_eq!(get("p.who.name"), Some("Ann"));
        assert_eq!((get("$index"), get("$count")), (Some("2"), Some("5")));
    }

    #[test]
    fn loops_over_a_captured_list() {
        let mut cat = Catalog::default();
        let mut col = Collection::new("c");
        let mut list = Request::new("list", Protocol::Rest);
        list.url = "https://api/people".into();
        list.extract.push(Extraction {
            variable: "people".into(),
            source: ExtractSource::Json,
            expression: "$.value".into(),
            scope: VarScope::Runtime,
            enabled: true,
        });
        let mut one = Request::new("one", Protocol::Rest);
        one.url = "https://api/people/{{person.id}}?n={{$index}}".into();
        let mut audit = Request::new("audit", Protocol::Rest);
        audit.url = "https://api/audit/{{person.name}}".into();
        let mut after = Request::new("after", Protocol::Rest);
        after.url = "https://api/done/{{person}}".into();
        let mut wf = Workflow::new("flow");
        wf.steps.push(WorkflowStep::new(list.id.clone()));
        let mut looped = WorkflowStep::new(one.id.clone());
        looped.for_each = Some(ForEach { list: "{{people}}".into(), item: "person".into(), steps: 2, ..Default::default() });
        wf.steps.push(looped);
        wf.steps.push(WorkflowStep::new(audit.id.clone()));
        wf.steps.push(WorkflowStep::new(after.id.clone()));
        for r in [list, one, audit, after] {
            col.items.push(Item::Request(r));
        }
        cat.collections.push(col);
        // A value the loop variable shadows, and must not leak past the loop.
        cat.runtime.push(KeyValue::new("person", "outside"));
        let wid = wf.id.clone();
        cat.workflows.push(wf);

        let people = r#"{"value":[{"id":1,"name":"ann"},{"id":2,"name":"bo"}]}"#;
        let t = Scripted::new(vec![
            reply(200, &[], people),
            reply(200, &[], "{}"),
            reply(200, &[], "{}"),
            reply(200, &[], "{}"),
            reply(200, &[], "{}"),
            reply(200, &[], "{}"),
        ]);
        let mut reports = Vec::new();
        let s = pollster::block_on(run(&cat, &wid, &t, &mut |r| {
            reports.push(r.clone());
            true
        }))
        .unwrap();
        let urls: Vec<String> = t.sent.borrow().iter().map(|r| r.url.clone()).collect();
        assert_eq!(
            urls,
            [
                "https://api/people",
                "https://api/people/1?n=0",
                "https://api/audit/ann",
                "https://api/people/2?n=1",
                "https://api/audit/bo",
                "https://api/done/outside"
            ]
        );
        assert_eq!(s.passed, 6);
        assert_eq!(reports[3].item.as_ref().map(|i| (i.index, i.count)), Some((1, 2)));
        assert_eq!(reports[5].item, None);
    }

    #[test]
    fn empty_and_missing_lists() {
        let mut cat = Catalog::default();
        let mut col = Collection::new("c");
        col.variables.push(KeyValue::new("ids", "[]"));
        let r = Request::new("r", Protocol::Rest);
        let mut wf = Workflow::new("flow");
        let mut s = WorkflowStep::new(r.id.clone());
        s.for_each = Some(ForEach { list: "ids".into(), ..Default::default() });
        wf.steps.push(s);
        let mut s = WorkflowStep::new(r.id.clone());
        s.for_each = Some(ForEach { list: "nope".into(), ..Default::default() });
        wf.steps.push(s);
        col.items.push(Item::Request(r));
        cat.collections.push(col);
        let wid = wf.id.clone();
        cat.workflows.push(wf);
        let t = Scripted::new(vec![]);
        let mut reports = Vec::new();
        let s = pollster::block_on(run(&cat, &wid, &t, &mut |r| {
            reports.push(r.clone());
            true
        }))
        .unwrap();
        assert_eq!((s.skipped, s.failed), (1, 1));
        assert!(reports[1].outcome.as_ref().unwrap().error.as_deref().unwrap().contains("{{nope}} has no value"));
    }
}
