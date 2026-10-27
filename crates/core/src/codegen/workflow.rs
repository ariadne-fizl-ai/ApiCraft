//! Workflows as standalone Bash or PowerShell scripts. Each step is the same
//! request "Save as code" writes; values captured from a response are kept in
//! script variables and passed to the steps after it, and checks become
//! conditions that count passes and failures.
//!
//! Captured variables are resolved to a marker while the requests are
//! generated, and the markers are then turned into variable references
//! inside the quoted strings that hold them.

use super::{curl_parts, generate_with, ps, ps_request, request_plan, sh, Target};
use crate::extract::{describe, path_segments, Seg};
use crate::model::{Catalog, Check, CheckKind, ExtractSource, Extraction, KeyValue, Protocol};
use crate::vars::Vars;

const OPEN: char = '\u{1}';
const CLOSE: char = '\u{2}';
const ITERATION: &str = "$iteration";

fn marker(name: &str) -> String {
    format!("{OPEN}{name}{CLOSE}")
}

struct Step {
    /// Position in the workflow, from 1.
    number: usize,
    name: String,
    request_id: String,
    protocol: Protocol,
    delay_ms: u32,
    extract: Vec<Extraction>,
    checks: Vec<Check>,
}

impl Step {
    fn scripted(&self) -> bool {
        matches!(self.protocol, Protocol::Rest | Protocol::Soap | Protocol::GraphQl | Protocol::OData)
    }
    fn title(&self) -> String {
        format!("Step {}: {}", self.number, self.name)
    }
}

pub fn generate_workflow(catalog: &Catalog, workflow_id: &str, now_ms: u64, target: Target) -> Result<String, String> {
    let wf = catalog.workflow(workflow_id).ok_or("workflow not found")?;
    let mut steps = Vec::new();
    for (i, s) in wf.steps.iter().enumerate().filter(|(_, s)| s.enabled) {
        let Some((_, req)) = catalog.find_request(&s.request_id) else { continue };
        steps.push(Step {
            number: i + 1,
            name: req.name.clone(),
            request_id: s.request_id.clone(),
            protocol: req.protocol,
            delay_ms: s.delay_ms,
            extract: req.extract.iter().chain(&s.extract).filter(|e| e.enabled && !e.variable.trim().is_empty()).cloned().collect(),
            checks: req.checks.iter().chain(&s.checks).filter(|c| c.enabled).cloned().collect(),
        });
    }
    if steps.is_empty() {
        return Err("the workflow has no enabled steps".into());
    }

    let mut captured: Vec<String> = Vec::new();
    for e in steps.iter().filter(|s| s.scripted()).flat_map(|s| &s.extract) {
        let name = e.variable.trim().to_string();
        if !captured.contains(&name) {
            captured.push(name);
        }
    }
    let mut wf_vars = wf.variables.clone();
    wf_vars.push(KeyValue::new(ITERATION, marker(ITERATION)));
    let vars_for = |request_id: &str, markers: bool| {
        let mut v = Vars::for_request(catalog, catalog.collection_of(request_id), &wf_vars, now_ms);
        if markers {
            for name in &captured {
                v.set(name, &marker(name));
            }
        }
        v
    };
    // Captured variables start from the values the app has now, so a step
    // that reads one before it is captured behaves as it does in the app.
    let first = vars_for(&steps[0].request_id, false);
    let initial: Vec<(String, String)> = captured.iter().map(|n| (n.clone(), first.get(n).unwrap_or_default())).collect();

    let mut body = Vec::new();
    for step in &steps {
        let vars = vars_for(&step.request_id, true);
        let code = match target {
            Target::Curl => bash_step(catalog, step, &vars, now_ms)?,
            Target::PowerShell => ps_step(catalog, step, &vars, now_ms)?,
        };
        body.push(code);
    }
    let iterations = wf.iterations.max(1);
    Ok(match target {
        Target::Curl => bash_script(&wf.name, wf.stop_on_failure, iterations, &initial, &body),
        Target::PowerShell => ps_script(&wf.name, wf.stop_on_failure, iterations, &initial, &body),
    })
}

// ---------------------------------------------------------------- Bash

fn bash_var(name: &str) -> String {
    if name == ITERATION {
        return "iteration".into();
    }
    let clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    format!("v_{clean}")
}

/// Markers only ever sit inside single-quoted strings, so each becomes
/// "close the quote, expand the variable, reopen the quote".
fn bash_markers(code: &str) -> String {
    replace_markers(code, |name| format!("'\"${{{}}}\"'", bash_var(name)))
}

fn replace_markers(code: &str, with: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(code.len());
    let mut rest = code;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len_utf8()..];
        match after.find(CLOSE) {
            Some(end) => {
                out.push_str(&with(&after[..end]));
                rest = &after[end + CLOSE.len_utf8()..];
            }
            None => {
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

const BASH_HELPERS: &str = r#"tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
passed=0
failed=0

# json '["data", 0, "id"]' prints the value at that path (exit status 1 when there is none).
json() {
  P="$1" perl -MJSON::PP -0777 -ne '
    binmode STDOUT, ":utf8";
    my $c = eval { JSON::PP->new->utf8->allow_nonref->decode($_) };
    exit 1 if $@;
    for my $s (@{ JSON::PP->new->decode($ENV{P}) }) {
      if (ref $c eq "HASH") { exit 1 unless exists $c->{$s}; $c = $c->{$s} }
      elsif (ref $c eq "ARRAY") {
        if ($s eq "length") { $c = scalar @$c }
        elsif ($s =~ /^\d+$/ && $s < @$c) { $c = $c->[$s] }
        else { exit 1 }
      }
      elsif (defined $c && !ref $c && $s eq "length") { $c = length $c }
      else { exit 1 }
    }
    if (!defined $c) { print "" }
    elsif (JSON::PP::is_bool($c)) { print $c ? "true" : "false" }
    elsif (ref $c) { print JSON::PP->new->encode($c) }
    else { print $c }
  ' "$tmp/body" 2>/dev/null
}
json_has() { json "$1" >/dev/null; }
header() { grep -i "^$1:" "$tmp/headers" | tail -n 1 | cut -d: -f2- | sed 's/^ *//' | tr -d '\r'; }
xml() { xmllint --xpath "string($1)" "$tmp/body" 2>/dev/null; }
match() { RE="$1" perl -0777 -ne 'if ($_ =~ /$ENV{RE}/) { print defined $1 ? $1 : $& }' "$tmp/body"; }
status_is() {
  local s=$1 p parts
  IFS=',' read -ra parts <<<"${2:-2xx}"
  for p in "${parts[@]}"; do
    p=${p// /}
    case $p in
      [0-9][xX][xX]) [ "${s:0:1}" = "${p:0:1}" ] && return 0 ;;
      *-*) [ "$s" -ge "${p%-*}" ] 2>/dev/null && [ "$s" -le "${p#*-}" ] && return 0 ;;
      *) [ "$s" = "$p" ] && return 0 ;;
    esac
  done
  return 1
}
check() {
  local label=$1
  shift
  if "$@"; then echo "  ok   $label"; else echo "  FAIL $label" >&2; step_failed=1; fi
}
need() { if [ -z "$1" ]; then echo "  FAIL could not capture $2" >&2; step_failed=1; fi; }
finish() {
  echo
  echo "$passed passed, $failed failed"
  [ "$failed" -eq 0 ]
  exit
}
end_step() {
  if [ "$step_failed" -eq 0 ]; then
    passed=$((passed + 1))
  else
    failed=$((failed + 1))
    [ "$stop_on_failure" -eq 1 ] && finish
  fi
}
"#;

fn bash_script(name: &str, stop: bool, iterations: u32, initial: &[(String, String)], steps: &[String]) -> String {
    let mut s = String::from("#!/usr/bin/env bash\n");
    s.push_str(&format!("# Workflow {}, exported from Fizl ApiCraft.\n", sh(name)));
    s.push_str("# Needs curl and perl (for JSON and regex captures); xmllint for XML captures.\n");
    s.push_str("# Values captured from responses are kept in v_* variables and passed to later steps.\n\n");
    s.push_str(&format!("stop_on_failure={}\n", u8::from(stop)));
    s.push_str(BASH_HELPERS);
    if !initial.is_empty() {
        s.push_str("\n# Captured variables, starting from their current values\n");
        for (k, v) in initial {
            s.push_str(&format!("{}={}\n", bash_var(k), sh(v)));
        }
    }
    s.push('\n');
    if iterations > 1 {
        s.push_str(&format!("for iteration in $(seq 1 {iterations}); do\necho \"Iteration $iteration\"\n\n"));
    } else {
        s.push_str("iteration=1\n\n");
    }
    s.push_str(&steps.join("\n"));
    if iterations > 1 {
        s.push_str("done\n");
    }
    s.push_str("\nfinish\n");
    s
}

fn bash_step(catalog: &Catalog, step: &Step, vars: &Vars, now_ms: u64) -> Result<String, String> {
    let mut s = format!("# {}\necho {}\n", step.title(), sh(&step.title()));
    if step.delay_ms > 0 {
        s.push_str(&format!("sleep {}\n", seconds(step.delay_ms)));
    }
    if !step.scripted() {
        return Ok(s + &unscripted_step(catalog, step, vars, now_ms, Target::Curl, "#")?);
    }
    let (plan, _) = request_plan(catalog, &step.request_id, vars, now_ms, Target::Curl)?;
    for n in &plan.notes {
        s.push_str(&format!("# {n}\n"));
    }
    let mut parts = vec!["--silent --show-error --output \"$tmp/body\" --dump-header \"$tmp/headers\" --write-out '%{http_code} %{time_total}'".to_string()];
    parts.extend(curl_parts(&plan));
    s.push_str("step_failed=0\n");
    s.push_str("rm -f \"$tmp/body\" \"$tmp/headers\"; touch \"$tmp/body\" \"$tmp/headers\"\n");
    s.push_str(&format!("out=$(curl {}) || {{ echo '  FAIL the request did not complete' >&2; step_failed=1; }}\n", parts.join(" \\\n  ")));
    s.push_str("status=${out%% *}\nseconds=${out##* }\n");
    for e in &step.extract {
        let var = bash_var(e.variable.trim());
        let expr = e.expression.trim();
        let value = match e.source {
            ExtractSource::Status => "$status".to_string(),
            ExtractSource::Header => format!("$(header {})", sh(expr)),
            ExtractSource::Json => format!("$(json {})", sh(&json_path_array(expr))),
            ExtractSource::Xml => format!("$(xml {})", sh(&xpath(expr))),
            ExtractSource::Regex => format!("$(match {})", sh(expr)),
        };
        s.push_str(&format!("{var}={value}\nneed \"${var}\" {}\n", sh(e.variable.trim())));
    }
    for c in &step.checks {
        let c = Check { target: vars.apply(&c.target), expected: vars.apply(&c.expected), ..c.clone() };
        let (t, x) = (sh(&c.target), sh(&c.expected));
        let cond = match c.kind {
            CheckKind::Status => format!("status_is \"$status\" {x}"),
            CheckKind::BodyContains => format!("grep -qF -- {x} \"$tmp/body\""),
            CheckKind::JsonEquals if c.expected.is_empty() => format!("json_has {}", sh(&json_path_array(&c.target))),
            CheckKind::JsonEquals => format!("test \"$(json {})\" = {x}", sh(&json_path_array(&c.target))),
            CheckKind::Header if c.expected.is_empty() => format!("test -n \"$(header {t})\""),
            CheckKind::Header => format!("test \"$(header {t})\" = {x}"),
            CheckKind::TimeBelow => format!("awk -v s=\"$seconds\" -v l={x} 'BEGIN {{ exit !(s * 1000 < l) }}'"),
        };
        s.push_str(&format!("check {} {cond}\n", sh(&describe(&c))));
    }
    s.push_str("end_step\n");
    Ok(bash_markers(&s))
}

fn seconds(ms: u32) -> String {
    let s = format!("{:.3}", ms as f64 / 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// gRPC, WebSocket and MCP steps: the single-request code, without
/// captures or checks. WebSocket code would wait for frames forever, so
/// it is left commented out.
fn unscripted_step(catalog: &Catalog, step: &Step, vars: &Vars, now_ms: u64, target: Target, comment: &str) -> Result<String, String> {
    let kind = match step.protocol {
        Protocol::Grpc => "gRPC",
        Protocol::WebSocket => "WebSocket",
        _ => "MCP",
    };
    let code = generate_with(catalog, &step.request_id, vars, now_ms, target)?;
    let code = match target {
        Target::Curl => bash_markers(&code),
        Target::PowerShell => ps_markers(&code),
    };
    let mut s = format!("{comment} Captures and checks are not scripted for {kind} steps.\n");
    if step.protocol == Protocol::WebSocket {
        s.push_str(&format!("{comment} WebSocket: run this separately; it keeps the connection open.\n"));
        for line in code.lines() {
            s.push_str(&format!("{comment} {line}\n"));
        }
    } else {
        s.push_str(&code);
    }
    Ok(s)
}

/// A JSON path (`$.a.b[0]['c d']`) as the JSON array the script's `json` helper walks.
fn json_path_array(path: &str) -> String {
    let segs = path_segments(path).unwrap_or_else(|| vec![Seg::Key(path.trim().to_string())]);
    let items: Vec<serde_json::Value> = segs
        .into_iter()
        .map(|s| match s {
            Seg::Key(k) => k.into(),
            Seg::Index(i) => i.into(),
        })
        .collect();
    serde_json::Value::Array(items).to_string()
}

/// The app's XML path (`Envelope/Body/AddResponse/AddResult`, local names,
/// each step any descendant, `@attr` last) as XPath.
fn xpath(path: &str) -> String {
    let mut x = String::new();
    for seg in path.split('/').map(str::trim).filter(|s| !s.is_empty()) {
        match seg.strip_prefix('@') {
            Some(attr) => x.push_str(&format!("/@*[local-name()='{attr}']")),
            None => x.push_str(&format!("//*[local-name()='{}']", seg.rsplit(':').next().unwrap_or(seg))),
        }
    }
    if x.is_empty() {
        "/*".into()
    } else {
        x
    }
}

// ---------------------------------------------------------------- PowerShell

const PS_HELPERS: &str = r#"$ErrorActionPreference = 'Stop'
$passed = 0
$failed = 0

function Get-JsonValue([string]$Text, [object[]]$Path) {
    try {
        if ((Get-Command ConvertFrom-Json).Parameters.ContainsKey('DateKind')) {
            $cur = ConvertFrom-Json -InputObject $Text -DateKind String
        } else {
            $cur = ConvertFrom-Json -InputObject $Text
        }
    } catch { return $null }
    foreach ($seg in $Path) {
        if ($null -eq $cur) { return $null }
        if ($seg -is [string] -and $seg -eq 'length' -and ($cur -is [array] -or $cur -is [string])) { $cur = $cur.Length }
        elseif ($cur -is [array]) {
            if ("$seg" -match '^\d+$' -and [int]"$seg" -lt $cur.Count) { $cur = $cur[[int]"$seg"] } else { return $null }
        } elseif ($cur -is [System.Management.Automation.PSCustomObject]) {
            $prop = $cur.PSObject.Properties["$seg"]
            if ($null -eq $prop) { return $null }
            $cur = $prop.Value
        } else { return $null }
    }
    if ($null -eq $cur) { return '' }
    if ($cur -is [string]) { return $cur }
    if ($cur -is [bool]) { return $cur.ToString().ToLowerInvariant() }
    if ($cur -is [datetime]) { return $cur.ToString('o') }
    if ($cur -is [array] -or $cur -is [System.Management.Automation.PSCustomObject]) { return ConvertTo-Json -InputObject $cur -Compress -Depth 32 }
    return [string]$cur
}

function Get-Header($Response, [string]$Name) {
    if ($null -eq $Response) { return $null }
    $value = $Response.Headers[$Name]
    if ($null -eq $value) { return $null }
    return ($value -join ', ')
}

function Get-XmlValue([string]$Text, [string]$XPath) {
    try { $node = ([xml]$Text).SelectSingleNode($XPath) } catch { return $null }
    if ($null -eq $node) { return $null }
    return $node.InnerText
}

function Get-RegexMatch([string]$Text, [string]$Pattern) {
    $m = [regex]::Match($Text, $Pattern)
    if (-not $m.Success) { return $null }
    if ($m.Groups.Count -gt 1 -and $m.Groups[1].Success) { return $m.Groups[1].Value }
    return $m.Value
}

function Test-Status([int]$Status, [string]$Expected) {
    if (-not $Expected.Trim()) { $Expected = '2xx' }
    foreach ($part in $Expected.Split(',')) {
        $p = $part.Trim()
        if ($p -match '^(\d)xx$') { if ([math]::Floor($Status / 100) -eq [int]$Matches[1]) { return $true } }
        elseif ($p -match '^(\d+)\s*-\s*(\d+)$') { if ($Status -ge [int]$Matches[1] -and $Status -le [int]$Matches[2]) { return $true } }
        elseif ($p -eq "$Status") { return $true }
    }
    return $false
}

function Check([string]$Label, [bool]$Ok) {
    if ($Ok) { Write-Host "  ok   $Label" }
    else { Write-Host "  FAIL $Label" -ForegroundColor Red; $script:stepFailed = $true }
}

function Confirm-Captured([string]$Name) {
    if ($null -eq $v[$Name]) { Write-Host "  FAIL could not capture $Name" -ForegroundColor Red; $script:stepFailed = $true }
}

function Write-Summary {
    Write-Host ''
    Write-Host "$script:passed passed, $script:failed failed"
}

function Complete-Step {
    if (-not $script:stepFailed) { $script:passed++; return }
    $script:failed++
    if ($stopOnFailure) { Write-Summary; exit 1 }
}
"#;

fn ps_script(name: &str, stop: bool, iterations: u32, initial: &[(String, String)], steps: &[String]) -> String {
    let mut s = format!("# Workflow {}, exported from Fizl ApiCraft.\n", ps(name));
    s.push_str("# Needs PowerShell 7 or later. Values captured from responses are kept in $v and passed to later steps.\n\n");
    s.push_str(&format!("$stopOnFailure = ${stop}\n"));
    s.push_str(PS_HELPERS);
    s.push_str("\n# Captured variables, starting from their current values\n$v = @{");
    if initial.is_empty() {
        s.push_str("}\n");
    } else {
        s.push('\n');
        for (k, val) in initial {
            s.push_str(&format!("    {} = {}\n", ps(k), ps(val)));
        }
        s.push_str("}\n");
    }
    s.push_str(&format!("\nforeach ($iteration in 1..{iterations}) {{\n"));
    if iterations > 1 {
        s.push_str("Write-Host \"Iteration $iteration\"\n");
    }
    s.push('\n');
    s.push_str(&steps.join("\n"));
    s.push_str("}\n\nWrite-Summary\nif ($failed -gt 0) { exit 1 }\n");
    s
}

fn ps_step(catalog: &Catalog, step: &Step, vars: &Vars, now_ms: u64) -> Result<String, String> {
    let mut s = format!("# {}\nWrite-Host {}\n", step.title(), ps(&step.title()));
    if step.delay_ms > 0 {
        s.push_str(&format!("Start-Sleep -Milliseconds {}\n", step.delay_ms));
    }
    if !step.scripted() {
        return Ok(s + &unscripted_step(catalog, step, vars, now_ms, Target::PowerShell, "#")?);
    }
    let (plan, _) = request_plan(catalog, &step.request_id, vars, now_ms, Target::PowerShell)?;
    let (lines, mut call) = ps_request(&plan);
    call.push("-SkipHttpErrorCheck".into());
    s.push_str("$stepFailed = $false\n");
    for l in lines {
        s.push_str(&l);
        s.push('\n');
    }
    s.push_str("$sw = [System.Diagnostics.Stopwatch]::StartNew()\n");
    s.push_str(&format!("try {{\n    $response = {}\n}} catch {{\n", call.join(" `\n        ")));
    s.push_str("    Write-Host \"  FAIL $($_.Exception.Message)\" -ForegroundColor Red\n    $stepFailed = $true\n    $response = $null\n}\n");
    s.push_str("$sw.Stop()\n");
    s.push_str("$status = if ($response) { [int]$response.StatusCode } else { 0 }\n");
    s.push_str("$text = if ($null -eq $response) { '' } elseif ($response.Content -is [byte[]]) { [System.Text.Encoding]::UTF8.GetString($response.Content) } else { [string]$response.Content }\n");
    for e in &step.extract {
        let name = e.variable.trim();
        let expr = e.expression.trim();
        let value = match e.source {
            ExtractSource::Status => "[string]$status".to_string(),
            ExtractSource::Header => format!("Get-Header $response {}", ps(expr)),
            ExtractSource::Json => format!("Get-JsonValue $text {}", ps_path(expr)),
            ExtractSource::Xml => format!("Get-XmlValue $text {}", ps(&xpath(expr))),
            ExtractSource::Regex => format!("Get-RegexMatch $text {}", ps(expr)),
        };
        s.push_str(&format!("$v[{0}] = {value}\nConfirm-Captured {0}\n", ps(name)));
    }
    for c in &step.checks {
        let c = Check { target: vars.apply(&c.target), expected: vars.apply(&c.expected), ..c.clone() };
        let (t, x) = (ps(&c.target), ps(&c.expected));
        let cond = match c.kind {
            CheckKind::Status => format!("(Test-Status $status {x})"),
            CheckKind::BodyContains => format!("($text.Contains({x}))"),
            CheckKind::JsonEquals if c.expected.is_empty() => format!("($null -ne (Get-JsonValue $text {}))", ps_path(&c.target)),
            CheckKind::JsonEquals => format!("((Get-JsonValue $text {}) -ceq {x})", ps_path(&c.target)),
            CheckKind::Header if c.expected.is_empty() => format!("($null -ne (Get-Header $response {t}))"),
            CheckKind::Header => format!("((Get-Header $response {t}) -ceq {x})"),
            CheckKind::TimeBelow => format!("($sw.ElapsedMilliseconds -lt [double]{x})"),
        };
        s.push_str(&format!("Check {} {cond}\n", ps(&describe(&c))));
    }
    s.push_str("Complete-Step\n");
    Ok(ps_markers(&s))
}

/// Backtick-escapes the characters `special` picks in the text between
/// markers, and expands the markers.
fn expand_between(content: &str, special: impl Fn(char) -> bool, expand: &impl Fn(&str) -> String) -> String {
    let escape = |text: &str| text.chars().map(|ch| if special(ch) { format!("`{ch}") } else { ch.to_string() }).collect::<String>();
    let mut out = String::new();
    let mut rest = content;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&escape(&rest[..start]));
        let after = &rest[start + OPEN.len_utf8()..];
        let Some(end) = after.find(CLOSE) else {
            rest = after;
            continue;
        };
        out.push_str(&expand(&after[..end]));
        rest = &after[end + CLOSE.len_utf8()..];
    }
    out.push_str(&escape(rest));
    out
}

/// A JSON path as the segment array `Get-JsonValue` walks.
fn ps_path(path: &str) -> String {
    let segs = path_segments(path).unwrap_or_else(|| vec![Seg::Key(path.trim().to_string())]);
    let items: Vec<String> = segs
        .into_iter()
        .map(|s| match s {
            Seg::Key(k) => ps(&k),
            Seg::Index(i) => i.to_string(),
        })
        .collect();
    format!("@({})", items.join(", "))
}

/// PowerShell can't splice a variable into a single-quoted string, so each
/// single-quoted string or here-string holding a marker is rewritten as its
/// double-quoted form with `$($v['name'])` in place of the marker.
fn ps_markers(code: &str) -> String {
    if !code.contains(OPEN) {
        return code.to_string();
    }
    let expand = |name: &str| if name == ITERATION { "$($iteration)".to_string() } else { format!("$($v[{}])", ps(name)) };
    let is_quote = |c: char| matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}');
    let chars: Vec<char> = code.chars().collect();
    let mut out = String::with_capacity(code.len() + 32);
    let mut i = 0;
    let mut line_start = true;
    while i < chars.len() {
        let c = chars[i];
        // Comment lines are copied as they are (they may hold apostrophes).
        if line_start && c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }
        if !c.is_whitespace() {
            line_start = false;
        }
        if c == '\n' {
            line_start = true;
        }
        // Here-string: @'<newline> ... <newline>'@
        if c == '@' && chars.get(i + 1).copied().is_some_and(is_quote) && chars.get(i + 2) == Some(&'\n') {
            let start = i + 3;
            let mut j = start;
            let end = loop {
                if j >= chars.len() {
                    break None;
                }
                if chars[j] == '\n' && chars.get(j + 1).copied().is_some_and(is_quote) && chars.get(j + 2) == Some(&'@') {
                    break Some(j);
                }
                j += 1;
            };
            if let Some(end) = end {
                let content: String = chars[start..end].iter().collect();
                if content.contains(OPEN) {
                    let body = expand_between(&content, |ch| matches!(ch, '`' | '$'), &expand);
                    out.push_str(&format!("@\"\n{body}\n\"@"));
                } else {
                    out.push_str(&format!("@'\n{content}\n'@"));
                }
                i = end + 3;
                continue;
            }
        }
        // Double-quoted strings are copied, honouring backtick escapes.
        if c == '"' {
            out.push(c);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '`' && i + 1 < chars.len() {
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                i += 1;
                if chars[i - 1] == '"' {
                    break;
                }
            }
            continue;
        }
        if is_quote(c) {
            let mut j = i + 1;
            let mut content = String::new();
            while j < chars.len() {
                if is_quote(chars[j]) {
                    if chars.get(j + 1).copied().is_some_and(is_quote) {
                        content.push(chars[j]);
                        j += 2;
                        continue;
                    }
                    break;
                }
                content.push(chars[j]);
                j += 1;
            }
            if content.contains(OPEN) {
                out.push('"');
                out.push_str(&expand_between(&content, |ch| matches!(ch, '`' | '$' | '"' | '\u{201c}' | '\u{201d}' | '\u{201e}'), &expand));
                out.push('"');
            } else {
                out.extend(&chars[i..=j.min(chars.len() - 1)]);
            }
            i = j + 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Collection, Item, Request, VarScope, Workflow, WorkflowStep};

    fn catalog() -> (Catalog, String) {
        let mut login = Request::new("Login", Protocol::Rest);
        login.method = "POST".into();
        login.url = "{{base}}/login".into();
        login.extract.push(Extraction {
            variable: "token".into(),
            source: ExtractSource::Json,
            expression: "$.data['access token']".into(),
            scope: VarScope::Runtime,
            enabled: true,
        });
        login.checks.push(Check { kind: CheckKind::Status, target: String::new(), expected: "2xx".into(), enabled: true });
        let mut me = Request::new("Me", Protocol::Rest);
        me.url = "{{base}}/me?page={{$iteration}}".into();
        me.headers.push(KeyValue::new("Authorization", "Bearer {{token}}"));
        me.checks.push(Check { kind: CheckKind::JsonEquals, target: "$.name".into(), expected: "it's {{token}}".into(), enabled: true });
        let mut col = Collection::new("API");
        col.variables.push(KeyValue::new("base", "https://api.example.com"));
        let step =
            |r: &Request| WorkflowStep { id: r.id.clone(), request_id: r.id.clone(), enabled: true, delay_ms: 0, extract: Vec::new(), checks: Vec::new() };
        let wf = Workflow {
            id: "wf".into(),
            name: "Sign in".into(),
            steps: vec![step(&login), step(&me)],
            stop_on_failure: true,
            variables: Vec::new(),
            iterations: 1,
        };
        col.items = vec![Item::Request(login), Item::Request(me)];
        (Catalog { collections: vec![col], workflows: vec![wf], ..Default::default() }, "wf".into())
    }

    #[test]
    fn bash_passes_captures_to_later_steps() {
        let (cat, wf) = catalog();
        let s = generate_workflow(&cat, &wf, 0, Target::Curl).unwrap();
        assert!(!s.contains(OPEN), "{s}");
        assert!(s.contains("v_token=$(json '[\"data\",\"access token\"]')"), "{s}");
        assert!(s.contains("--header 'Authorization: Bearer '\"${v_token}\"''"), "{s}");
        assert!(s.contains("--url 'https://api.example.com/me?page='\"${iteration}\"''"), "{s}");
        assert!(s.contains("check 'status is 2xx' status_is \"$status\" '2xx'"), "{s}");
        assert!(s.contains("test \"$(json '[\"name\"]')\" = 'it'\\''s '\"${v_token}\"''"), "{s}");
    }

    #[test]
    fn powershell_interpolates_captures() {
        let (cat, wf) = catalog();
        let s = generate_workflow(&cat, &wf, 0, Target::PowerShell).unwrap();
        assert!(!s.contains(OPEN), "{s}");
        assert!(s.contains("$v['token'] = Get-JsonValue $text @('data', 'access token')"), "{s}");
        assert!(s.contains("'Authorization' = \"Bearer $($v['token'])\""), "{s}");
        assert!(s.contains("-Uri \"https://api.example.com/me?page=$($iteration)\""), "{s}");
        assert!(s.contains("-ceq \"it's $($v['token'])\""), "{s}");
        assert!(s.contains("-SkipHttpErrorCheck"), "{s}");
    }

    #[test]
    fn here_strings_with_markers_become_expandable() {
        let code = format!("$body = @'\n{{\"t\":\"{}\",\"cost\":\"$5\"}}\n'@\n# don't touch\n", marker("token"));
        let out = ps_markers(&code);
        assert_eq!(out, "$body = @\"\n{\"t\":\"$($v['token'])\",\"cost\":\"`$5\"}\n\"@\n# don't touch\n");
    }

    #[test]
    fn json_paths_become_segment_arrays() {
        assert_eq!(json_path_array("$.a.b[0]"), r#"["a","b",0]"#);
        assert_eq!(json_path_array("$[0]['x y'].length"), r#"[0,"x y","length"]"#);
        assert_eq!(json_path_array("$"), "[]");
        assert_eq!(
            xpath("Envelope/Body/AddResult/@id"),
            "//*[local-name()='Envelope']//*[local-name()='Body']//*[local-name()='AddResult']/@*[local-name()='id']"
        );
    }
}
