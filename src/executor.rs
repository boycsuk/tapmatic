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

                    let handle = thread::Builder::new()
                        .name(format!("macro-{}", vk))
                        .spawn(move || {
                            execute_macro(&mac, &flag_clone, &fb_tx);
                            let _ = fb_tx.send(AppEvent::MacroFinished(vk));
                            if let Some(ref chain_name) = mac.chain_macro {
                                let _ = fb_tx.send(AppEvent::ChainMacro(chain_name.clone()));
                            }
                        })
                        .ok();

                    active.insert(vk, ActiveMacro { stop_flag, handle });
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

fn sleep_interruptible(ms: u64, stop_flag: &AtomicBool) -> bool {
    let mut remaining = ms;
    while remaining > 0 && !should_stop(stop_flag) {
        let sleep_ms = remaining.min(5);
        thread::sleep(Duration::from_millis(sleep_ms));
        remaining = remaining.saturating_sub(sleep_ms);
    }
    should_stop(stop_flag)
}

/// Release all keys and mouse buttons that were pressed during execution.
fn release_all(pressed_keys: &HashSet<u16>, pressed_mouse: &HashSet<MouseButton>) {
    for &sc in pressed_keys {
        input::send_key(sc, true);
    }
    for &btn in pressed_mouse {
        input::send_mouse(btn, true);
    }
}

fn execute_macro(mac: &Macro, stop_flag: &AtomicBool, fb_tx: &mpsc::Sender<AppEvent>) {
    let mut pressed_keys: HashSet<u16> = HashSet::new();
    let mut pressed_mouse: HashSet<MouseButton> = HashSet::new();

    execute_macro_inner(mac, stop_flag, fb_tx, &mut pressed_keys, &mut pressed_mouse);

    // Release any keys/buttons still held when macro stops
    release_all(&pressed_keys, &pressed_mouse);
}

fn execute_macro_inner(
    mac: &Macro,
    stop_flag: &AtomicBool,
    fb_tx: &mpsc::Sender<AppEvent>,
    pressed_keys: &mut HashSet<u16>,
    pressed_mouse: &mut HashSet<MouseButton>,
) {
    let is_single = mac.repetition == RepetitionMode::SingleShot;
    let start_time = Instant::now();
    let mut reps: u32 = 0;
    let mut last_progress_secs: u32 = u32::MAX;
    let vk = mac.hotkey_vk;

    let mut rng_state: u64 = start_time.elapsed().as_nanos() as u64 | 1;

    let window_mode = mac.send_mode == SendMode::Window && mac.bound_process.is_some();

    // Start delay (countdown before first execution)
    if mac.start_delay_ms > 0 {
        if sleep_interruptible(mac.start_delay_ms, stop_flag) {
            return;
        }
    }

    loop {
        for step in &mac.steps {
            if should_stop(stop_flag) {
                return;
            }

            // Require held key check
            if mac.require_held_vk != 0 && !input::is_key_pressed(mac.require_held_vk) {
                while !should_stop(stop_flag) && !input::is_key_pressed(mac.require_held_vk) {
                    thread::sleep(Duration::from_millis(10));
                }
                if should_stop(stop_flag) {
                    return;
                }
            }

            // Process focus check (Global mode only)
            if !window_mode {
                if let Some(ref bound) = mac.bound_process {
                    if !is_bound_process_focused(bound) {
                        while !should_stop(stop_flag) && !is_bound_process_focused(bound) {
                            thread::sleep(Duration::from_millis(50));
                        }
                        if should_stop(stop_flag) {
                            return;
                        }
                    }
                }
            }

            // Compute delay
            let raw_delay = if let Some((min, max)) = mac.random_delay {
                if min >= max {
                    min
                } else {
                    rng_state ^= rng_state << 13;
                    rng_state ^= rng_state >> 7;
                    rng_state ^= rng_state << 17;
                    min + (rng_state % (max - min + 1))
                }
            } else if mac.use_recorded_delays {
                step.delay_ms
            } else {
                mac.fixed_interval_ms
            };

            // Apply humanize jitter: ±humanize_ms
            let humanized = if mac.humanize_ms > 0 && raw_delay > 0 {
                rng_state ^= rng_state << 13;
                rng_state ^= rng_state >> 7;
                rng_state ^= rng_state << 17;
                let jitter = (rng_state % (mac.humanize_ms * 2 + 1)) as i64 - mac.humanize_ms as i64;
                (raw_delay as i64 + jitter).max(1) as u64
            } else {
                raw_delay
            };

            let delay = apply_speed(humanized);

            if delay > 0 {
                if sleep_interruptible(delay, stop_flag) {
                    return;
                }
            }

            // Track pressed state before executing
            match &step.action {
                KeyAction::KeyDown(sc) => { pressed_keys.insert(*sc); }
                KeyAction::KeyUp(sc) => { pressed_keys.remove(sc); }
                KeyAction::MouseDown(btn) => { pressed_mouse.insert(*btn); }
                KeyAction::MouseUp(btn) => { pressed_mouse.remove(btn); }
                _ => {}
            }

            // Conditional steps — wait before proceeding
            if let KeyAction::WaitForWindow(ref name) = step.action {
                while !should_stop(stop_flag) {
                    if input::get_foreground_process_name()
                        .is_some_and(|fg| fg == *name)
                    {
                        break;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                if should_stop(stop_flag) { return; }
            } else if window_mode {
                let process = mac.bound_process.as_deref().unwrap_or("");
                while !input::execute_action_to_process(&step.action, process) {
                    if should_stop(stop_flag) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            } else if mac.mouse_jitter > 0 {
                input::execute_action_with_jitter(&step.action, mac.mouse_jitter, &mut rng_state);
            } else {
                input::execute_action(&step.action);
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

fn is_bound_process_focused(bound: &str) -> bool {
    input::get_foreground_process_name()
        .is_some_and(|fg| fg == bound)
}
