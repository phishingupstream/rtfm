//! Shared process output and opt-in schema. Legacy payloads remain unchanged.
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    fmt,
    io::{self, Write},
};
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
#[derive(Default)]
struct State {
    machine: bool,
    data: Value,
    actions: Vec<Value>,
    context: Value,
    write_error: Option<io::Error>,
}
#[derive(Debug)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
    pub details: Value,
}
impl Failure {
    pub fn new(code: &'static str, message: impl Into<String>, details: Value) -> Self {
        Self {
            code,
            message: message.into(),
            details,
        }
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for Failure {}
pub fn configure(machine: bool, context: Value) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.machine = machine;
        s.context = context;
    });
}
pub fn machine() -> bool {
    STATE.with(|s| s.borrow().machine)
}
pub fn data(value: Value) {
    STATE.with(|s| s.borrow_mut().data = value);
}
pub fn action(argv: Vec<String>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let mut contextual = vec!["rtfm".to_string()];
        for (key, flag) in [("config", "--config"), ("cache_dir", "--cache-dir")] {
            if let Some(v) = s.context[key].as_str() {
                contextual.extend([flag.into(), v.into()]);
            }
        }
        contextual.extend(argv);
        let cwd = s.context["cwd"].clone();
        s.actions.push(json!({"argv":contextual,"cwd":cwd}));
    });
}
pub fn action_at(config: &std::path::Path, cache: &std::path::Path, args: Vec<String>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let mut argv = vec![
            "rtfm".into(),
            "--config".into(),
            config.display().to_string(),
            "--cache-dir".into(),
            cache.display().to_string(),
        ];
        argv.extend(args);
        let cwd = s.context["cwd"].clone();
        s.actions.push(json!({"argv":argv,"cwd":cwd}));
    });
}
pub fn text(args: fmt::Arguments<'_>) {
    if !machine() {
        write(args);
    }
}
pub fn write(args: fmt::Arguments<'_>) {
    if let Err(e) = io::stdout().lock().write_fmt(args) {
        STATE.with(|s| s.borrow_mut().write_error = Some(e));
    }
}
pub fn finish(error: Option<&anyhow::Error>, usage: bool) -> i32 {
    let code = if let Some(interrupted) =
        error.and_then(|e| e.downcast_ref::<crate::cache::Interrupted>())
    {
        128 + interrupted.0
    } else if usage {
        2
    } else if error.is_some() {
        1
    } else {
        0
    };
    if let Some(failure) = error.and_then(|e| e.downcast_ref::<Failure>()) {
        match failure.code {
            "version_missing" => {
                let source = failure.details["source"].as_str();
                let mut versions = vec!["versions".into()];
                if let Some(source) = source {
                    versions.push(source.into());
                }
                action(versions);
                if let (Some(source), Some(reference)) =
                    (source, failure.details["reference"].as_str())
                {
                    action(vec![
                        "use".into(),
                        source.into(),
                        "--ref".into(),
                        reference.into(),
                    ]);
                    action(vec!["update".into(), source.into()]);
                }
            }
            "source_unknown" | "source_disabled" => action(vec!["sources".into()]),
            _ => {}
        }
    }
    if machine() {
        let envelope = STATE.with(|s| {
            let s = s.borrow();
            let err = error.map(|e| {
                let typed = e.downcast_ref::<Failure>();
                let fallback = if usage { "argument_invalid" } else if e.downcast_ref::<crate::cache::Interrupted>().is_some() { "interrupted" } else if e.downcast_ref::<crate::cache::ProcessTimeout>().is_some() || e.downcast_ref::<reqwest::Error>().is_some_and(reqwest::Error::is_timeout) { "timeout" } else if e.downcast_ref::<io::Error>().is_some() { "io_failed" } else { "operation_failed" };
                json!({"code":typed.map(|f|f.code).unwrap_or(fallback),"message":format!("{e:#}"),"details":typed.map(|f|f.details.clone()).unwrap_or_else(||json!({}))})
            });
            json!({"schema_version":1,"ok":error.is_none(),"data":s.data,"error":err,"next_actions":s.actions,"context":s.context})
        });
        write(format_args!("{}\n", envelope));
    } else if let Some(e) = error {
        let _ = writeln!(io::stderr().lock(), "rtfm: {e:#}");
        STATE.with(|s| {
            for action in &s.borrow().actions {
                if let Some(args) = action["argv"].as_array() {
                    let argv: Vec<String> = args
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect();
                    let _ = writeln!(io::stderr().lock(), "{}", crate::shell_command(&argv));
                }
            }
        });
    }
    STATE.with(|s| match s.borrow().write_error.as_ref() {
        Some(e) if e.kind() == io::ErrorKind::BrokenPipe => 0,
        Some(e) => {
            let _ = writeln!(io::stderr().lock(), "rtfm: stdout: {e}");
            1
        }
        None => code,
    })
}
