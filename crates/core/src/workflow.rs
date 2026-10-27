//! Running a [`Workflow`]: its steps in order, feeding each step the
//! variables captured by the ones before it.

use crate::engine::{apply_outcome, execute, Outcome};
use crate::http::Transport;
use crate::model::Catalog;

#[derive(Debug, Clone)]
pub struct StepReport {
    pub iteration: u32,
    /// Position in the workflow (0-based).
    pub index: usize,
    pub step_id: String,
    pub request_id: String,
    pub name: String,
    /// `None` when the step was skipped.
    pub outcome: Option<Outcome>,
    pub skipped: Option<String>,
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

/// Runs the workflow against a working copy of the catalog, calling
/// `on_step` after every step. The caller should apply each outcome to its
/// own catalog (with [`apply_outcome`]) to keep the captured values.
/// Returning `false` from `on_step` stops the run.
pub async fn run(catalog: &Catalog, workflow_id: &str, transport: &dyn Transport, on_step: &mut dyn FnMut(&StepReport) -> bool) -> Result<RunSummary, String> {
    let wf = catalog.workflow(workflow_id).ok_or("workflow not found")?.clone();
    let mut cat = catalog.clone();
    let mut summary = RunSummary::default();
    let started = transport.now_ms();
    'outer: for iteration in 1..=wf.iterations.max(1) {
        for (index, step) in wf.steps.iter().enumerate() {
            let name = cat.find_request(&step.request_id).map(|(_, r)| r.name.clone()).unwrap_or_else(|| "(deleted request)".into());
            let mut report = StepReport { iteration, index, step_id: step.id.clone(), request_id: step.request_id.clone(), name, outcome: None, skipped: None };
            if !step.enabled {
                report.skipped = Some("disabled".into());
                summary.skipped += 1;
                if !on_step(&report) {
                    summary.stopped_early = true;
                    break 'outer;
                }
                continue;
            }
            if step.delay_ms > 0 {
                transport.sleep(step.delay_ms).await;
            }
            let mut vars = wf.variables.clone();
            vars.push(crate::model::KeyValue::new("$iteration", iteration.to_string()));
            let outcome = execute(&cat, &step.request_id, &vars, &step.extract, &step.checks, transport).await;
            apply_outcome(&mut cat, &outcome);
            let ok = outcome.passed();
            report.outcome = Some(outcome);
            if ok {
                summary.passed += 1;
            } else {
                summary.failed += 1;
            }
            let keep_going = on_step(&report);
            if !keep_going || (!ok && wf.stop_on_failure) {
                summary.stopped_early = true;
                break 'outer;
            }
        }
    }
    summary.elapsed_ms = transport.now_ms().saturating_sub(started) as f64;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{reply, Scripted};
    use crate::model::*;

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
}
