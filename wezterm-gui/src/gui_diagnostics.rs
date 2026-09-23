//! Opt-in diagnostics for a GUI attached to an independently running mux.
//! Never inspect pane contents or queue more than one heartbeat while stalled.
use mux::MuxNotification;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Counters whose per-second deltas are always logged when non-zero.
const ALWAYS_LOGGED_PREFIX: &str = "diag.focus.";
/// Title rebuilds per second above which every counter delta is logged.
const TITLE_STORM_PER_SECOND: usize = 100;
/// Seconds without a message pump iteration before reporting starvation.
const PUMP_STARVED_SECONDS: u64 = 3;

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Counts a mux notification delivered to a TermWindow by variant name.
pub fn count_mux_notification(n: MuxNotification) -> MuxNotification {
    if enabled() {
        let kind = format!("{n:?}");
        let kind = kind
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap_or_default()
            .to_string();
        metrics::counter!("diag.termwindow.mux_notif", "kind" => kind).increment(1);
    }
    n
}

fn format_deltas(deltas: &[(String, usize)]) -> String {
    deltas
        .iter()
        .map(|(name, delta)| format!("{name}+{delta}"))
        .collect::<Vec<_>>()
        .join(" ")
}

struct CounterDeltas {
    previous: HashMap<String, usize>,
}

impl CounterDeltas {
    fn new() -> Self {
        Self {
            previous: crate::stats::counter_snapshot(),
        }
    }

    /// Returns counters that changed since the last call, largest first.
    fn take(&mut self) -> Vec<(String, usize)> {
        let current = crate::stats::counter_snapshot();
        let mut deltas: Vec<(String, usize)> = current
            .iter()
            .filter_map(|(name, value)| {
                let delta = value.saturating_sub(*self.previous.get(name).unwrap_or(&0));
                (delta > 0).then(|| (name.clone(), delta))
            })
            .collect();
        deltas.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        self.previous = current;
        deltas
    }
}

pub fn start() {
    if std::env::var_os("WEZTERM_GUI_DIAGNOSTICS").is_none() {
        return;
    }
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    ENABLED.store(true, Ordering::Relaxed);
    log::info!(target: "gui_diagnostics", "enabled pid={} revision=33891b4a local diagnostic build", std::process::id());
    if let Err(err) = std::thread::Builder::new()
        .name("gui-watchdog".into())
        .spawn(|| {
            let origin = Instant::now();
            let acknowledged = Arc::new(AtomicU64::new(0));
            let pending = Arc::new(AtomicBool::new(false));
            let mut reported = false;
            let mut last_report = 0;
            let mut counters = CounterDeltas::new();
            let mut last_pump = window::diagnostics::message_pump_iterations();
            let mut last_tasks = window::diagnostics::spawn_tasks_run();
            let mut pump_starved_secs = 0;
            let mut last_starved_report = 0;
            loop {
                std::thread::sleep(Duration::from_secs(1));
                log::logger().flush();
                let now = origin.elapsed().as_secs();
                let age = now.saturating_sub(acknowledged.load(Ordering::Acquire));
                if age >= 5 && (!reported || now - last_report >= 10) {
                    log::warn!(target: "gui_diagnostics", "GUI heartbeat stalled for {}s; capture main-thread stack with matching PDB", age);
                    reported = true;
                    last_report = now;
                } else if age < 5 && reported {
                    log::info!(target: "gui_diagnostics", "GUI heartbeat recovered");
                    reported = false;
                }

                let pump = window::diagnostics::message_pump_iterations();
                let tasks = window::diagnostics::spawn_tasks_run();
                let tasks_per_second = tasks - last_tasks;
                let longest_drain = window::diagnostics::take_longest_drain_tasks();
                if pump == last_pump && tasks_per_second > 0 {
                    pump_starved_secs += 1;
                } else if pump != last_pump {
                    if pump_starved_secs >= PUMP_STARVED_SECONDS {
                        log::info!(target: "gui_diagnostics", "message pump recovered after {}s", pump_starved_secs);
                    }
                    pump_starved_secs = 0;
                }
                last_pump = pump;
                last_tasks = tasks;

                let deltas = counters.take();
                let title_rebuilds: usize = deltas
                    .iter()
                    .filter(|(name, _)| name.starts_with("diag.termwindow.update_title"))
                    .map(|(_, delta)| *delta)
                    .sum();
                let starved = pump_starved_secs >= PUMP_STARVED_SECONDS;
                if starved && (pump_starved_secs == PUMP_STARVED_SECONDS || now - last_starved_report >= 10) {
                    log::warn!(
                        target: "gui_diagnostics",
                        "message pump starved for {}s while spawn queue ran {} tasks/s (longest drain {} tasks, queue depth {:?})",
                        pump_starved_secs,
                        tasks_per_second,
                        longest_drain,
                        window::diagnostics::spawn_queue_depth()
                    );
                    last_starved_report = now;
                }
                if starved || title_rebuilds > TITLE_STORM_PER_SECOND {
                    if !deltas.is_empty() {
                        log::warn!(target: "gui_diagnostics", "counters/s: {}", format_deltas(&deltas));
                    }
                } else {
                    let focus: Vec<_> = deltas
                        .into_iter()
                        .filter(|(name, _)| name.starts_with(ALWAYS_LOGGED_PREFIX))
                        .collect();
                    if !focus.is_empty() {
                        log::info!(target: "gui_diagnostics", "focus counters/s: {}", format_deltas(&focus));
                    }
                }

                if !pending.swap(true, Ordering::AcqRel) {
                    let pending = Arc::clone(&pending);
                    let acknowledged = Arc::clone(&acknowledged);
                    promise::spawn::spawn_into_main_thread(async move {
                        acknowledged.store(origin.elapsed().as_secs(), Ordering::Release);
                        pending.store(false, Ordering::Release);
                    }).detach();
                }
            }
        })
    {
        log::error!(target: "gui_diagnostics", "could not start watchdog: {}", err);
    }
}
