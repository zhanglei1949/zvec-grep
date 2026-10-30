//! Opt-in synchronous graph measurements. No changes to admission or budgets.
//! One thread-local summary is emitted on scope exit, including error exits.
use serde::Serialize;
use std::{cell::RefCell, collections::BTreeMap, time::Instant};

thread_local! {
    static TRACE: RefCell<Option<Trace>> = const { RefCell::new(None) };
}
struct Frame {
    stage: &'static str,
    started: Instant,
    children: u128,
}
#[derive(Default, Serialize)]
struct Measurement {
    calls: u64,
    inclusive_micros: u128,
    self_micros: u128,
}
struct Trace {
    name: &'static str,
    started: Instant,
    stack: Vec<Frame>,
    measurements: BTreeMap<(&'static str, &'static str), Measurement>,
    counters: BTreeMap<&'static str, u64>,
}

pub struct BuildProfile(bool);
impl BuildProfile {
    #[must_use]
    pub fn start(name: &'static str) -> Self {
        if std::env::var_os("ZG_GRAPH_PROFILE").is_none_or(|v| v != "1") {
            return Self(false);
        }
        TRACE.with(|trace| {
            let mut trace = trace.borrow_mut();
            if trace.is_some() {
                Self(false)
            } else {
                *trace = Some(Trace {
                    name,
                    started: Instant::now(),
                    stack: Vec::new(),
                    measurements: BTreeMap::new(),
                    counters: BTreeMap::new(),
                });
                Self(true)
            }
        })
    }
}
impl Drop for BuildProfile {
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        TRACE.with(|trace| {
            if let Some(trace) = trace.borrow_mut().take() {
                let measurements = trace
                    .measurements
                    .into_iter()
                    .map(|((parent, stage), value)| {
                        serde_json::json!({"parent":parent,"stage":stage,"calls":value.calls,
                        "inclusive_micros":value.inclusive_micros,"self_micros":value.self_micros})
                    })
                    .collect::<Vec<_>>();
                eprintln!(
                    "ZG_GRAPH_PROFILE {}",
                    serde_json::json!({
                        "operation":trace.name,"elapsed_micros":trace.started.elapsed().as_micros(),
                        "measurements":measurements,
                        "counters":trace.counters,
                    })
                );
            }
        });
    }
}

/// Diagnostic work counts; disabled unless the current synchronous operation
/// opted in. These do not change execution budgets or admission.
pub fn count(name: &'static str, amount: u64) {
    TRACE.with(|trace| {
        if let Some(trace) = trace.borrow_mut().as_mut() {
            let value = trace.counters.entry(name).or_default();
            *value = value.saturating_add(amount);
        }
    });
}

pub fn peak(name: &'static str, amount: u64) {
    TRACE.with(|trace| {
        if let Some(trace) = trace.borrow_mut().as_mut() {
            let value = trace.counters.entry(name).or_default();
            *value = (*value).max(amount);
        }
    });
}

pub struct Stage(bool);
#[must_use]
pub fn stage(name: &'static str) -> Stage {
    TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(trace) = trace.as_mut() else {
            return Stage(false);
        };
        trace.stack.push(Frame {
            stage: name,
            started: Instant::now(),
            children: 0,
        });
        Stage(true)
    })
}
impl Drop for Stage {
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        TRACE.with(|trace| {
            let mut trace = trace.borrow_mut();
            let Some(trace) = trace.as_mut() else { return };
            let Some(frame) = trace.stack.pop() else {
                return;
            };
            let elapsed = frame.started.elapsed().as_micros();
            let parent = if let Some(parent) = trace.stack.last_mut() {
                parent.children += elapsed;
                parent.stage
            } else {
                "root"
            };
            let measurement = trace.measurements.entry((parent, frame.stage)).or_default();
            measurement.calls += 1;
            measurement.inclusive_micros += elapsed;
            measurement.self_micros += elapsed.saturating_sub(frame.children);
        });
    }
}

pub fn measure<T>(name: &'static str, operation: impl FnOnce() -> T) -> T {
    let _stage = stage(name);
    operation()
}
