use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use std::collections::HashSet;

use crate::input;
use crate::macro_def::{KeyAction, Macro, MouseButton, RepetitionMode, SendMode, StopCondition};
use crate::state::{AppEvent, ExecutorCommand};
use crate::{MACROS_ENABLED, RUNNING, SPEED_MULTIPLIER};

struct ActiveMacro {
    stop_flag: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ActiveMacro {
    fn stop(self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle {
            let _ = h.join();
        }
    }
}

/// Own the running macros: spawn a thread per started macro, stop and join
/// them on command, and exit after draining a `StopAll`.
///
/// Runs on its own thread until `RUNNING` drops or the channel closes.
pub fn executor_loop(rx: mpsc::Receiver<ExecutorCommand>, feedback_tx: mpsc::Sender<AppEvent>) {
    let mut active: HashMap<i32, ActiveMacro> = HashMap::new();

    while RUNNING.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(cmd) => match cmd {
                ExecutorCommand::StartMacro(mac) => {
                    let vk = mac.hotkey_vk;

                    if let Some(existing) = active.remove(&vk) {
                        existing.stop();
                    }

                    let stop_flag = Arc::new(AtomicBool::new(false));
                    let flag_clone = Arc::clone(&stop_flag);
                    let fb_tx = feedback_tx.clone();

                    let spawned = thread::Builder::new()
                        .name(format!("macro-{}", vk))
                        .spawn(move || {
                            let completed = execute_macro(&mac, &flag_clone, &fb_tx);
                            let _ = fb_tx.send(AppEvent::MacroFinished(vk));
                            // Chaining an aborted macro would let a
                            // self-chaining one restart forever with no way
                            // to stop it.
                            if completed
                                && let Some(ref chain_name) = mac.chain_macro {
                                    let _ = fb_tx.send(AppEvent::ChainMacro(chain_name.clone()));
                                }
                        });

                    match spawned {
                        Ok(handle) => {
                            active.insert(vk, ActiveMacro { stop_flag, handle: Some(handle) });
                        }
                        // The UI already marked the macro active; without this
                        // event it would show as running forever.
                        Err(e) => {
                            let _ = feedback_tx.send(AppEvent::MacroFinished(vk));
                            let _ = feedback_tx
                                .send(AppEvent::Warning(format!("Could not start macro: {e}")));
                        }
                    }
                }
                ExecutorCommand::StopMacro(vk) => {
                    if let Some(existing) = active.remove(&vk) {
                        existing.stop();
                    }
                }
                ExecutorCommand::StopAll => {
                    for (_, am) in active.drain() {
                        am.stop();
                    }
                    break;
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn should_stop(stop_flag: &AtomicBool) -> bool {
    stop_flag.load(Ordering::SeqCst) || !MACROS_ENABLED.load(Ordering::SeqCst)
}

fn get_speed_multiplier() -> f64 {
    f64::from_bits(SPEED_MULTIPLIER.load(Ordering::Relaxed))
}

fn apply_speed(delay_ms: u64) -> u64 {
    let mult = get_speed_multiplier();
    (delay_ms as f64 * mult) as u64
}

/// Pick a delay within `[min, max]` from the RNG state.
///
/// Saturates on a full-range span: `max - min + 1` wraps to zero, and a zero
/// divisor panics in release as well as debug.
fn random_delay_in_range(min: u64, max: u64, rng_state: u64) -> u64 {
    if min >= max {
        return min;
    }
    match (max - min).checked_add(1) {
        Some(span) => min + (rng_state % span),
        None => min + rng_state,
    }
}

/// Offset a nonzero `raw_delay` by up to ±`humanize_ms`, flooring at 1ms;
/// a zero delay is returned unchanged.
///
/// Saturates the span so an out-of-range `humanize_ms` cannot overflow.
fn apply_humanize(raw_delay: u64, humanize_ms: u64, rng_state: u64) -> u64 {
    if humanize_ms == 0 || raw_delay == 0 {
        return raw_delay;
    }
    let span = humanize_ms.saturating_mul(2).saturating_add(1);
    let jitter = (rng_state % span) as i64 - humanize_ms as i64;
    (raw_delay as i64).saturating_add(jitter).max(1) as u64
}

fn sleep_interruptible(ms: u64, stop_flag: &AtomicBool) -> bool {
    let mut remaining = ms;
    while remaining > 0 && !should_stop(stop_flag) {
        let sleep_ms = remaining.min(5);
        thread::sleep(Duration::from_millis(sleep_ms));
        remaining = remaining.saturating_sub(sleep_ms);
    }
    should_stop(stop_flag)
}

/// Poll `cond` every `poll_ms` until it holds or the macro is stopped.
///
/// Returns true when stopped, mirroring `sleep_interruptible`.
fn wait_until(stop_flag: &AtomicBool, poll_ms: u64, cond: impl Fn() -> bool) -> bool {
    while !should_stop(stop_flag) && !cond() {
        thread::sleep(Duration::from_millis(poll_ms));
    }
    should_stop(stop_flag)
}

/// Keys and buttons pressed during execution, released on `Drop` so they are
/// freed even when the step loop panics or the thread is unwound.
struct HeldInputs {
    keys: HashSet<u16>,
    mouse: HashSet<MouseButton>,
    /// `Some(process)` when presses were posted to that process's window;
    /// releasing must use the same channel, or the target never sees its
    /// key-ups while whichever window has focus receives spurious ones.
    window_process: Option<String>,
    window_target: input::WindowTarget,
}

impl HeldInputs {
    fn new(mac: &Macro) -> Self {
        let window_process = (mac.send_mode == SendMode::Window)
            .then(|| mac.bound_process.clone())
            .flatten();
        Self {
            keys: HashSet::new(),
            mouse: HashSet::new(),
            window_process,
            window_target: input::WindowTarget::default(),
        }
    }

    fn window_mode(&self) -> bool {
        self.window_process.is_some()
    }
}

impl Drop for HeldInputs {
    fn drop(&mut self) {
        if let Some(process) = self.window_process.take() {
            // A vanished target has nothing left to release; falling back to
            // the global path would inject key-ups into the focused window.
            let never_stop = AtomicBool::new(false);
            for &sc in &self.keys {
                input::execute_action_to_process(
                    &KeyAction::KeyUp(sc),
                    &process,
                    &mut self.window_target,
                    &never_stop,
                );
            }
            for &btn in &self.mouse {
                input::execute_action_to_process(
                    &KeyAction::MouseUp(btn),
                    &process,
                    &mut self.window_target,
                    &never_stop,
                );
            }
        } else {
            for &sc in &self.keys {
                input::send_key(sc, true);
            }
            for &btn in &self.mouse {
                input::send_mouse(btn, true);
            }
        }
    }
}

/// Run a macro to completion, releasing held inputs on the way out.
///
/// Returns true when the macro finished on its own rather than being stopped
/// or disabled.
fn execute_macro(mac: &Macro, stop_flag: &AtomicBool, fb_tx: &mpsc::Sender<AppEvent>) -> bool {
    let mut held = HeldInputs::new(mac);
    execute_macro_inner(mac, stop_flag, fb_tx, &mut held);
    !should_stop(stop_flag)
}

fn execute_macro_inner(
    mac: &Macro,
    stop_flag: &AtomicBool,
    fb_tx: &mpsc::Sender<AppEvent>,
    held: &mut HeldInputs,
) {
    let is_single = mac.repetition == RepetitionMode::SingleShot;
    let start_time = Instant::now();
    let mut reps: u32 = 0;
    let mut last_progress_secs: u32 = u32::MAX;
    let vk = mac.hotkey_vk;

    // Seeding from `start_time.elapsed()` here would yield a near-constant
    // value, making every run's "random" delays identical.
    let mut rng_state: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
        ^ (vk as u64) << 32
        | 1;

    let window_mode = held.window_mode();
    let mut warned_blocked = false;
    let mut focus_cache: Option<(Instant, bool)> = None;

    if mac.start_delay_ms > 0
        && sleep_interruptible(mac.start_delay_ms, stop_flag) {
            return;
        }

    loop {
        if should_stop(stop_flag) {
            return;
        }

        for step in &mac.steps {
            if should_stop(stop_flag) {
                return;
            }

            if mac.require_held_vk != 0
                && !input::is_key_pressed(mac.require_held_vk)
                && wait_until(stop_flag, 10, || input::is_key_pressed(mac.require_held_vk))
            {
                return;
            }

            if !window_mode
                && let Some(ref bound) = mac.bound_process {
                    // The foreground window changes at human speed; querying
                    // it for every step of a fast macro is ~100 syscall
                    // clusters per second.
                    let focused = match focus_cache {
                        Some((at, val)) if at.elapsed() < FOCUS_RECHECK_INTERVAL => val,
                        _ => {
                            let val = is_bound_process_focused(bound);
                            focus_cache = Some((Instant::now(), val));
                            val
                        }
                    };
                    if !focused {
                        if wait_until(stop_flag, 50, || is_bound_process_focused(bound)) {
                            return;
                        }
                        focus_cache = Some((Instant::now(), true));
                    }
                }

            let raw_delay = if let Some((min, max)) = mac.random_delay {
                random_delay_in_range(min, max, input::xorshift64(&mut rng_state))
            } else if mac.use_recorded_delays {
                step.delay_ms
            } else {
                mac.fixed_interval_ms
            };

            let humanized = if mac.humanize_ms > 0 && raw_delay > 0 {
                apply_humanize(raw_delay, mac.humanize_ms, input::xorshift64(&mut rng_state))
            } else {
                raw_delay
            };

            let delay = apply_speed(humanized);

            if delay > 0
                && sleep_interruptible(delay, stop_flag) {
                    return;
                }

            match &step.action {
                KeyAction::KeyDown(sc) => { held.keys.insert(*sc); }
                KeyAction::KeyUp(sc) => { held.keys.remove(sc); }
                KeyAction::MouseDown(btn) => { held.mouse.insert(*btn); }
                KeyAction::MouseUp(btn) => { held.mouse.remove(btn); }
                _ => {}
            }

            // Conditional steps — wait before proceeding
            if let KeyAction::WaitForWindow(ref name) = step.action {
                let found = || input::get_foreground_process_name().is_some_and(|fg| fg == *name);
                if wait_until(stop_flag, 50, found) {
                    return;
                }
            } else if window_mode {
                let process = mac.bound_process.as_deref().unwrap_or("");
                while !input::execute_action_to_process(
                    &step.action,
                    process,
                    &mut held.window_target,
                    stop_flag,
                ) {
                    if should_stop(stop_flag) {
                        return;
                    }
                    // The cached handle is already gone by now; drop it so the
                    // retry re-resolves instead of failing on a stale one.
                    held.window_target.invalidate();
                    thread::sleep(Duration::from_millis(50));
                }
            } else {
                let sent = if mac.mouse_jitter > 0 {
                    input::execute_action_with_jitter(
                        &step.action,
                        mac.mouse_jitter,
                        &mut rng_state,
                        stop_flag,
                    )
                } else {
                    input::execute_action(&step.action, stop_flag)
                };
                // UIPI or a secure desktop blocks injection silently; without
                // this the macro "runs" while sending nothing.
                if !sent && !warned_blocked {
                    warned_blocked = true;
                    let _ = fb_tx.send(AppEvent::Warning(
                        "Input injection blocked (elevated target?)".into(),
                    ));
                }
            }
        }

        reps += 1;

        let elapsed_secs = start_time.elapsed().as_secs() as u32;
        if elapsed_secs != last_progress_secs || reps <= 3 {
            last_progress_secs = elapsed_secs;
            let _ = fb_tx.send(AppEvent::MacroProgress(vk, reps, elapsed_secs));
        }

        if is_single {
            return;
        }

        match mac.stop_condition {
            StopCondition::None => {}
            StopCondition::AfterReps(max_reps) => {
                if reps >= max_reps {
                    return;
                }
            }
            StopCondition::AfterSecs(max_secs) => {
                if start_time.elapsed().as_secs() >= max_secs as u64 {
                    return;
                }
            }
        }

        if mac.cycle_delay_ms > 0 {
            let delay = apply_speed(mac.cycle_delay_ms);
            if sleep_interruptible(delay, stop_flag) {
                return;
            }
        }
    }
}

/// How long a bound-process focus answer stays valid before re-querying.
const FOCUS_RECHECK_INTERVAL: Duration = Duration::from_millis(75);

fn is_bound_process_focused(bound: &str) -> bool {
    input::get_foreground_process_name()
        .is_some_and(|fg| fg == bound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_delay_stays_within_range() {
        for rng in [0, 1, u64::MAX / 2, u64::MAX] {
            let d = random_delay_in_range(10, 20, rng);
            assert!((10..=20).contains(&d), "{d} outside [10, 20]");
        }
    }

    #[test]
    fn random_delay_collapsed_range_returns_min() {
        assert_eq!(random_delay_in_range(50, 50, 12345), 50);
        assert_eq!(random_delay_in_range(80, 20, 12345), 80);
    }

    #[test]
    fn random_delay_full_span_does_not_panic() {
        // max - min + 1 wraps to zero here; a zero divisor would panic even in
        // release, killing the macro thread mid-execution.
        let d = random_delay_in_range(0, u64::MAX, 999);
        assert_eq!(d, 999);
    }

    #[test]
    fn humanize_stays_within_jitter_bounds() {
        for rng in [0, 7, u64::MAX] {
            let d = apply_humanize(100, 10, rng);
            assert!((90..=110).contains(&d), "{d} outside 100±10");
        }
    }

    #[test]
    fn humanize_floors_at_one_ms() {
        assert_eq!(apply_humanize(1, 500, 0), 1, "never returns a zero delay");
    }

    #[test]
    fn humanize_zero_is_identity() {
        assert_eq!(apply_humanize(42, 0, 999), 42);
        assert_eq!(apply_humanize(0, 99, 999), 0);
    }

    #[test]
    fn humanize_extreme_value_does_not_overflow() {
        let d = apply_humanize(50, u64::MAX, 12345);
        assert!(d >= 1);
    }

    #[test]
    fn apply_speed_scales_and_keeps_zero() {
        assert_eq!(apply_speed(0), 0);
        assert_eq!(apply_speed(100), 100, "default multiplier is 1.0");
    }

    fn instant_macro(reps: u32) -> Macro {
        let mut mac = Macro::new("t".into());
        mac.use_recorded_delays = false;
        mac.fixed_interval_ms = 0;
        mac.stop_condition = StopCondition::AfterReps(reps);
        // An empty TypeText sends nothing on any platform: on Windows a
        // KeyDown step would inject a real keystroke into the CI runner.
        mac.steps.push(crate::macro_def::MacroStep {
            action: KeyAction::TypeText(String::new()),
            delay_ms: 0,
        });
        mac
    }

    #[test]
    fn execute_macro_stops_after_exact_reps() {
        let mac = instant_macro(3);
        let (tx, rx) = mpsc::channel();
        let flag = AtomicBool::new(false);

        let completed = execute_macro(&mac, &flag, &tx);

        assert!(completed, "AfterReps is a natural finish");
        let reps: Vec<u32> = rx
            .try_iter()
            .filter_map(|e| match e {
                AppEvent::MacroProgress(_, reps, _) => Some(reps),
                _ => None,
            })
            .collect();
        assert_eq!(reps.last(), Some(&3), "must stop at exactly 3 reps");
    }

    #[test]
    fn execute_macro_reports_stop_as_not_completed() {
        let mac = instant_macro(3);
        let (tx, _rx) = mpsc::channel();
        let flag = AtomicBool::new(true);

        assert!(!execute_macro(&mac, &flag, &tx), "an aborted run must not chain");
    }
}
