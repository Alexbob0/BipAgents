//! The loop brake replayed on real days (2026-10-28). After the brake fix of 2026-10-28 morning, Vie (aibox) ran
//! Hermes' `session_search` 3 times with the same arguments while diagnosing a broken mail scan, and was stopped: the
//! bench of 2026-10-27 had replayed invented loops, never what agents really do day to day.
//!
//! Each JSON line is one turn as `scripts/brake-traces/export.py` writes it (no content: tool names, hashed
//! arguments, errors, writes, seconds since the turn started). Every turn is fed through the brake's decision logic as
//! the bridge would: the run hub's count (`TurnCount::note`) for the turns a run carries, the MCP endpoint's
//! (`Turns::check` / `record`) for the bridge's own tools, a routine's turn (`kind: cron`) only through the latter.
//! The report lists every turn the brake would warn about, refuse a call in, or stop, with the reason and the call's
//! index.
//!
//! - `sample_*`: tests/fixtures/brake-traces-sample.jsonl (synthetic, `scripts/brake-traces/make_sample.py`), each
//!   turn named and labelled normal or loop: no normal turn is cut, every loop is;
//! - `real_traces_report`: the files Alex brings back from aibox, dropped in tests/fixtures/brake-traces-real/
//!   (git-ignored) or a folder named by `BRAKE_TRACES`; a report, no assertion (they carry no labels):
//!
//!     cargo test -p bipbridge --test push-runs_brake_replay -- --nocapture --test-threads=1
//!     BRAKE_TRACES=/some/folder cargo test -p bipbridge --test push-runs_brake_replay real_traces -- --nocapture
//!
//! Only `adapter` below touches the brake's API: point it at the new one when it changes.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// One call of a turn, as export.py writes it.
#[derive(Debug, Clone)]
struct Call {
    tool: String,
    /// sha256 of the canonical JSON arguments, 12 hex.
    args: String,
    error: bool,
    write: bool,
    /// Seconds since the turn started.
    t: f64,
}

#[derive(Debug, Clone)]
struct Trace {
    /// `user/agent`.
    agent: String,
    /// cron, task, live, relay, conversation.
    kind: String,
    session: String,
    start: f64,
    calls: Vec<Call>,
    /// The sample's own: its name and what it is (normal / loop). Real traces have neither.
    name: Option<String>,
    expect: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Action {
    /// A plain warning (the model told « already done », or the brake's soft warning).
    Warn,
    /// A call refused with an error the model must stop on (the routines' cap, the writes' cap).
    Refuse,
    /// The run stopped.
    Stop,
}

#[derive(Debug, Clone)]
struct Decision {
    /// The call's index in the turn (0 = first).
    index: usize,
    tool: String,
    action: Action,
    reason: String,
}

impl Decision {
    fn cuts(&self) -> bool {
        matches!(self.action, Action::Stop | Action::Refuse)
    }
}

// -- the adapter: the only part that knows the brake's API ------------------------------------------------------------

mod adapter {
    use super::{Action, Call, Decision, Trace};
    use bipbridge::loop_brake::{self, Settings, Turn, TurnCount, Turns};
    use bipbridge::Dict;
    use serde_json::{json, Value};

    /// sha256("{}")[..12]: a call with no arguments (the hub then tells calls apart by the event's preview, which a
    /// trace doesn't have: no signature).
    pub const NO_ARGS: &str = "44136fa355b3";

    fn bridge_tool(tool: &str) -> bool {
        tool.starts_with("mcp__bipagents__") || tool.starts_with("mcp_bipagents_")
    }

    /// The arguments as the MCP endpoint sees them: only their hash is known, which tells equal calls apart as well.
    fn arguments(call: &Call) -> Dict {
        let mut args = Dict::new();
        if call.args != NO_ARGS {
            args.insert("sha".into(), json!(call.args));
        }
        args
    }

    /// One replay: the bridge's MCP counts live across turns (a routine's turn is the agent's calls until it has been
    /// quiet `idle_seconds`), so the traces are fed in time order through the same `Turns`.
    pub struct Replay {
        settings: Settings,
        turns: Turns,
    }

    impl Replay {
        pub fn new() -> Self {
            Replay { settings: Settings::default(), turns: Turns::default() }
        }

        pub fn turn(&mut self, trace: &Trace) -> Vec<Decision> {
            let mut out = Vec::new();
            let routine = trace.kind == "cron";
            let background = trace.kind == "task";
            // runs.rs `brake`: the hub's limits, a browsing turn's total with the browser's on top
            let base = self.settings.limits(background);
            let mut count = TurnCount::default();
            let mut used_browser = false;
            let run = (!routine).then(|| Turn { run_id: format!("{}@{}", trace.session, trace.start), background });
            for (index, call) in trace.calls.iter().enumerate() {
                if !routine {
                    let browser = bipbridge::runs::browser_tool(&call.tool);
                    used_browser |= browser;
                    let mut limits = base;
                    if used_browser && !background {
                        limits.tool_calls += limits.browser_calls;
                    }
                    let signature = (call.args != NO_ARGS).then_some(call.args.as_str());
                    if let Some(over) = count.note(&call.tool, signature, browser, &limits) {
                        out.push(Decision {
                            index,
                            tool: call.tool.clone(),
                            action: Action::Stop,
                            reason: loop_brake::reason(&over),
                        });
                        break;
                    }
                }
                if !bridge_tool(&call.tool) {
                    continue;
                }
                let args = arguments(call);
                let now = trace.start + call.t;
                match self.turns.check(&self.settings, &trace.agent, run.as_ref(), &call.tool, &args, now) {
                    None => {
                        if !call.error {
                            self.turns.record(&trace.agent, run.as_ref(), &call.tool, &args);
                        }
                    }
                    Some(answer) => {
                        let hard = answer.get("isError") == Some(&Value::Bool(true));
                        let answer = Value::Object(answer);
                        let text = answer
                            .pointer("/content/0/text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .chars()
                            .take(90)
                            .collect::<String>();
                        out.push(Decision {
                            index,
                            tool: call.tool.clone(),
                            action: if hard { Action::Refuse } else { Action::Warn },
                            reason: text,
                        });
                        if hard {
                            // the model is told to stop and answer: the rest of the trace is what it did instead
                            break;
                        }
                    }
                }
            }
            out
        }
    }
}

// -- reading and reporting ---------------------------------------------------------------------------------------------

fn parse(line: &str) -> Option<Trace> {
    let v: Value = serde_json::from_str(line).ok()?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let calls = v
        .get("calls")?
        .as_array()?
        .iter()
        .map(|c| Call {
            tool: c.get("tool").and_then(Value::as_str).unwrap_or("?").to_string(),
            args: c.get("args").and_then(Value::as_str).unwrap_or("").to_string(),
            error: c.get("error").and_then(Value::as_bool).unwrap_or(false),
            write: c.get("write").and_then(Value::as_bool).unwrap_or(false),
            t: c.get("t").and_then(Value::as_f64).unwrap_or(0.0),
        })
        .collect();
    Some(Trace {
        agent: format!("{}/{}", s("user"), s("agent")),
        kind: s("kind"),
        session: s("session"),
        start: v.get("start").and_then(Value::as_f64).unwrap_or(0.0),
        calls,
        name: v.get("name").and_then(Value::as_str).map(str::to_string),
        expect: v.get("expect").and_then(Value::as_str).map(str::to_string),
    })
}

fn load(paths: &[PathBuf]) -> Vec<Trace> {
    let mut traces = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
            traces.push(parse(line).unwrap_or_else(|| panic!("{}:{}: not a trace", path.display(), n + 1)));
        }
    }
    traces.sort_by(|a, b| a.start.total_cmp(&b.start));
    traces
}

fn label(trace: &Trace) -> String {
    trace.name.clone().unwrap_or_else(|| format!("{} {}", trace.session, trace.start as i64))
}

/// Every trace replayed, in time order: (trace, its decisions).
fn replay(traces: &[Trace]) -> Vec<(Trace, Vec<Decision>)> {
    let mut replay = adapter::Replay::new();
    traces.iter().map(|t| (t.clone(), replay.turn(t))).collect()
}

fn report(results: &[(Trace, Vec<Decision>)]) -> String {
    let mut out = String::new();
    let cut = results.iter().filter(|(_, d)| d.iter().any(Decision::cuts)).count();
    let warned = results.iter().filter(|(_, d)| !d.is_empty() && !d.iter().any(Decision::cuts)).count();
    let calls: usize = results.iter().map(|(t, _)| t.calls.len()).sum();
    out.push_str(&format!(
        "{} turns, {calls} calls: {cut} cut (stopped or refused), {warned} only warned\n",
        results.len()
    ));
    for (trace, decisions) in results.iter().filter(|(_, d)| !d.is_empty()) {
        let writes = trace.calls.iter().filter(|c| c.write).count();
        let errors = trace.calls.iter().filter(|c| c.error).count();
        out.push_str(&format!(
            "- {} [{} {}{}] {} calls ({writes} writes, {errors} errors)\n",
            label(trace),
            trace.agent,
            trace.kind,
            trace.expect.as_deref().map(|e| format!(", {e}")).unwrap_or_default(),
            trace.calls.len()
        ));
        for d in decisions {
            out.push_str(&format!("    {:?} at call #{} ({}): {}\n", d.action, d.index + 1, d.tool, d.reason));
        }
    }
    out
}

fn sample() -> Vec<Trace> {
    load(&[Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/brake-traces-sample.jsonl")])
}

/// Normal turns the brake of main still cuts (2026-10-28): the cases the gentler brake must let through. When the
/// new brake merges, empty this list: the test then holds every normal turn to « never cut ».
const KNOWN_FALSE_CUTS: [&str; 2] = ["history_diagnostic_same_search", "mail_scan_busy_night"];

#[test]
fn sample_traces_normal_days_go_on_and_loops_are_cut() {
    let results = replay(&sample());
    println!("{}", report(&results));
    assert!(results.len() >= 20, "the sample has its turns");
    let mut wrong = Vec::new();
    for (trace, decisions) in &results {
        let name = trace.name.clone().expect("every sample turn is named");
        let cut = decisions.iter().any(Decision::cuts);
        match trace.expect.as_deref() {
            Some("loop") if !cut => wrong.push(format!("{name}: a loop not cut")),
            Some("normal") if cut && !KNOWN_FALSE_CUTS.contains(&name.as_str()) => {
                wrong.push(format!("{name}: a normal turn cut: {:?}", decisions))
            }
            Some("normal") if !cut && KNOWN_FALSE_CUTS.contains(&name.as_str()) => {
                wrong.push(format!("{name}: no longer cut, take it out of KNOWN_FALSE_CUTS"))
            }
            Some("normal") | Some("loop") => {}
            other => wrong.push(format!("{name}: expect {other:?}")),
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn sample_loops_are_cut_early() {
    // a loop is cut long before Hermes' own cap of 150 calls, and the reasons are the ones expected
    let results = replay(&sample());
    let by_name = |name: &str| results.iter().find(|(t, _)| t.name.as_deref() == Some(name)).unwrap();
    for (trace, decisions) in results.iter().filter(|(t, _)| t.expect.as_deref() == Some("loop")) {
        let first = decisions.iter().find(|d| d.cuts()).unwrap();
        assert!(first.index < 70, "{}: cut at {}", label(trace), first.index);
    }
    let (_, d) = by_name("browser_page_loop");
    assert!(d[0].reason.contains("read the page"), "{d:?}");
    let (_, d) = by_name("routine_search_loop");
    assert_eq!((d[0].action, d[0].index), (Action::Refuse, 10));
    let (_, d) = by_name("terminal_runaway");
    assert_eq!(d[0].reason, "too many tool calls");
    // a write that failed may be made again with the same arguments, unwarned
    let (_, d) = by_name("write_failed_then_retried");
    assert!(d.is_empty(), "{d:?}");
}

#[test]
fn real_traces_report() {
    let dir = std::env::var_os("BRAKE_TRACES")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/brake-traces-real"));
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect()
        })
        .unwrap_or_default();
    files.sort();
    if files.is_empty() {
        println!("no real traces in {} (export.py's .jsonl files go there)", dir.display());
        return;
    }
    let traces = load(&files);
    println!("{} file(s) from {}", files.len(), dir.display());
    println!("{}", report(&replay(&traces)));
}
