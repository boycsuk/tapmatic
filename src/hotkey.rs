use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::input;
use crate::macro_def::{KeyAction, MouseButton, RepetitionMode, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON, VK_XBUTTON1, VK_XBUTTON2};
use crate::state::AppEvent;
use crate::{AWAITING_HOTKEY, CAPTURE_MOUSE_BUTTONS, MOUSE_MOVE_THRESHOLD, QUICK_RECORD, RECORDING, RECORD_MOUSE_MOVES, RUNNING};

/// Bound hotkeys shared between the UI thread (writer) and the scan loop.
pub struct HotkeyConfig {
    pub bound_keys: Vec<(i32, RepetitionMode)>,
}

impl HotkeyConfig {
    pub fn new() -> Self {
        Self {
            bound_keys: Vec::new(),
        }
    }
}

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const VK_ESCAPE: i32 = 0x1B;
/// Polls (at POLL_INTERVAL) spent re-snapshotting keys after a mode change.
const MODE_SETTLE_POLLS: u32 = 15;
const VK_MENU: i32 = 0x12;
const VK_Z: i32 = 0x5A;
const VK_F1: i32 = 0x70;
const VK_F2: i32 = 0x71;
const VK_F3: i32 = 0x72;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Edge {
    None,
    Pressed,
    Released,
}

fn edge(was: bool, pressed: bool) -> Edge {
    match (was, pressed) {
        (false, true) => Edge::Pressed,
        (true, false) => Edge::Released,
        _ => Edge::None,
    }
}

/// Poll one key, update its previous state, and report the transition.
fn key_edge(vk: i32, prev_state: &mut [bool; 256]) -> Edge {
    let pressed = input::is_key_pressed(vk);
    let e = edge(prev_state[vk as usize], pressed);
    prev_state[vk as usize] = pressed;
    e
}

/// The event a bound key's transition produces for its repetition mode.
///
/// Toggle and SingleShot fire on release so the key's own down-state cannot
/// re-trigger them; HoldToRepeat needs both edges.
fn bound_key_event(vk: i32, mode: RepetitionMode, e: Edge) -> Option<AppEvent> {
    match (mode, e) {
        (RepetitionMode::Toggle | RepetitionMode::SingleShot, Edge::Released) => {
            Some(AppEvent::HotkeyPressed(vk))
        }
        (RepetitionMode::HoldToRepeat, Edge::Pressed) => Some(AppEvent::HotkeyPressed(vk)),
        (RepetitionMode::HoldToRepeat, Edge::Released) => Some(AppEvent::HotkeyReleased(vk)),
        _ => None,
    }
}

/// Falling edge on Z while Alt is held: the quick-record combo.
fn quick_record_triggered(prev_state: &mut [bool; 256]) -> bool {
    key_edge(VK_Z, prev_state) == Edge::Released && input::is_key_pressed(VK_MENU)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Recording,
    Capture,
}

/// Poll global key state every `POLL_INTERVAL`, translating transitions into
/// `AppEvent`s according to the current mode (normal, recording, capture).
///
/// Runs on its own thread until `RUNNING` drops.
pub fn hotkey_loop(tx: mpsc::Sender<AppEvent>, config: Arc<Mutex<HotkeyConfig>>) {
    let mut prev_state = [false; 256];
    let mut last_mode = Mode::Normal;
    let mut last_mouse_pos = (0i32, 0i32);

    while RUNNING.load(Ordering::SeqCst) {
        let mode = if AWAITING_HOTKEY.load(Ordering::SeqCst) {
            Mode::Capture
        } else if RECORDING.load(Ordering::SeqCst) {
            Mode::Recording
        } else {
            Mode::Normal
        };

        if mode != last_mode {
            last_mode = mode;
            // Re-snapshot key state for a settle window so the keystroke that
            // triggered the mode change does not leak into the new mode.
            for _ in 0..MODE_SETTLE_POLLS {
                snapshot_all_keys(&mut prev_state);
                thread::sleep(POLL_INTERVAL);
                if !RUNNING.load(Ordering::SeqCst) {
                    return;
                }
            }
            continue;
        }

        match mode {
            Mode::Capture => capture_scan(&tx, &mut prev_state),
            Mode::Recording => record_scan(&tx, &mut prev_state, &mut last_mouse_pos),
            Mode::Normal => normal_scan(&tx, &config, &mut prev_state),
        }

        thread::sleep(POLL_INTERVAL);
    }
}

fn snapshot_all_keys(prev_state: &mut [bool; 256]) {
    for vk in 0x01i32..=0xFE {
        prev_state[vk as usize] = input::is_key_pressed(vk);
    }
}

/// Whether `vk` must be ignored during a "press a key" capture.
///
/// Clicking the UI while binding a hotkey must not bind the macro to a mouse
/// button, which would then fire on every click anywhere. Step captures DO
/// accept LMB/RMB/MMB; the X side buttons never pass (unbindable for the same
/// reason, and they have no step mapping).
fn capture_excluded(vk: i32, capture_mouse: bool) -> bool {
    vk == VK_ESCAPE
        || (!capture_mouse && matches!(vk, VK_LBUTTON | VK_RBUTTON | VK_MBUTTON))
        || matches!(vk, VK_XBUTTON1 | VK_XBUTTON2)
}

fn capture_scan(tx: &mpsc::Sender<AppEvent>, prev_state: &mut [bool; 256]) {
    let capture_mouse = CAPTURE_MOUSE_BUTTONS.load(Ordering::SeqCst);
    for vk in 0x01i32..=0xFE {
        if capture_excluded(vk, capture_mouse) {
            continue;
        }
        if key_edge(vk, prev_state) == Edge::Released {
            let _ = tx.send(AppEvent::HotkeyCaptured(vk));
        }
    }
}

fn record_scan(
    tx: &mpsc::Sender<AppEvent>,
    prev_state: &mut [bool; 256],
    last_mouse_pos: &mut (i32, i32),
) {
    // Quick-record stop: Alt+Z during recording
    if QUICK_RECORD.load(Ordering::SeqCst) && quick_record_triggered(prev_state) {
        let _ = tx.send(AppEvent::QuickRecordToggle);
        return;
    }

    if key_edge(VK_ESCAPE, prev_state) == Edge::Released {
        let _ = tx.send(AppEvent::RecordStop);
        return;
    }

    // Recording control keys: F1=pause, F2=mouse, F3=threshold.
    // Detected here so they work without TUI focus; excluded from recorded steps below.
    for (vk, ev) in [
        (VK_F1, AppEvent::RecordPauseToggle),
        (VK_F2, AppEvent::RecordMouseToggle),
        (VK_F3, AppEvent::RecordThresholdCycle),
    ] {
        if key_edge(vk, prev_state) == Edge::Released {
            let _ = tx.send(ev);
        }
    }

    let now = Instant::now();

    if RECORD_MOUSE_MOVES.load(Ordering::SeqCst)
        && let Some(pos) = input::get_cursor_pos()
        && pos != *last_mouse_pos {
            let threshold = MOUSE_MOVE_THRESHOLD.load(Ordering::Relaxed) as i32;
            let dx = (pos.0 - last_mouse_pos.0).abs();
            let dy = (pos.1 - last_mouse_pos.1).abs();
            if dx >= threshold || dy >= threshold {
                let _ = tx.send(AppEvent::RecordKey(KeyAction::MouseMove(pos.0, pos.1), now));
                *last_mouse_pos = pos;
            }
    }

    record_button_edges(tx, prev_state, last_mouse_pos, now);
}

fn record_button_edges(
    tx: &mpsc::Sender<AppEvent>,
    prev_state: &mut [bool; 256],
    last_mouse_pos: &mut (i32, i32),
    now: Instant,
) {
    for &(vk, button) in &[
        (VK_LBUTTON, MouseButton::Left),
        (VK_RBUTTON, MouseButton::Right),
        (VK_MBUTTON, MouseButton::Middle),
    ] {
        match key_edge(vk, prev_state) {
            Edge::Pressed => {
                // Record mouse position before mouse-down events; a failed
                // query records the click without a position update.
                let pos = input::get_cursor_pos().unwrap_or(*last_mouse_pos);
                let mut click_time = now;
                if pos != *last_mouse_pos {
                    let _ = tx.send(AppEvent::RecordKey(KeyAction::MouseMove(pos.0, pos.1), now));
                    *last_mouse_pos = pos;
                    // Small gap so the click doesn't arrive before the move completes
                    click_time = now + Duration::from_millis(10);
                }
                let _ = tx.send(AppEvent::RecordKey(KeyAction::MouseDown(button), click_time));
            }
            Edge::Released => {
                let _ = tx.send(AppEvent::RecordKey(KeyAction::MouseUp(button), now));
            }
            Edge::None => {}
        }
    }

    // Keyboard keys (0x08 onwards, skip Escape, control keys, and Alt+Z combo)
    for vk in 0x08i32..=0xFE {
        if vk == VK_ESCAPE || vk == VK_F1 || vk == VK_F2 || vk == VK_F3 {
            continue;
        }
        // Skip Z when Alt is held (quick-record combo)
        if vk == VK_Z && input::is_key_pressed(VK_MENU) {
            prev_state[vk as usize] = input::is_key_pressed(vk);
            continue;
        }
        let down = match key_edge(vk, prev_state) {
            Edge::Pressed => true,
            Edge::Released => false,
            Edge::None => continue,
        };
        let sc = input::vk_to_scancode(vk);
        if sc != 0 {
            let action = if down { KeyAction::KeyDown(sc) } else { KeyAction::KeyUp(sc) };
            let _ = tx.send(AppEvent::RecordKey(action, now));
        }
    }
}

fn normal_scan(
    tx: &mpsc::Sender<AppEvent>,
    config: &Arc<Mutex<HotkeyConfig>>,
    prev_state: &mut [bool; 256],
) {
    if quick_record_triggered(prev_state) {
        let _ = tx.send(AppEvent::QuickRecordToggle);
    }

    // A poisoned lock only means the UI thread panicked while holding it;
    // the data itself cannot be half-written, and silently returning here
    // would disable every hotkey for the rest of the session. Iterating
    // under the lock beats cloning the Vec on every 10 ms poll: the sends
    // are non-blocking.
    let cfg = config.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for (vk, mode) in &cfg.bound_keys {
        let vk = *vk;
        if !crate::macro_def::is_valid_vk(vk) {
            continue;
        }
        let e = key_edge(vk, prev_state);
        if let Some(ev) = bound_key_event(vk, *mode, e) {
            let _ = tx.send(ev);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_exclusion_gates_mouse_buttons_by_mode() {
        // Hotkey binding: no mouse button may bind.
        for vk in [0x01, 0x02, 0x04, 0x05, 0x06] {
            assert!(capture_excluded(vk, false), "vk {vk:#x} must not bind a hotkey");
        }
        // Step capture: the three real buttons pass, the X buttons never do.
        for vk in [0x01, 0x02, 0x04] {
            assert!(!capture_excluded(vk, true), "vk {vk:#x} must reach step capture");
        }
        for vk in [0x05, 0x06] {
            assert!(capture_excluded(vk, true));
        }
        // Escape always cancels instead of binding; keys always pass.
        assert!(capture_excluded(VK_ESCAPE, true));
        assert!(!capture_excluded(0x41, false));
    }

    #[test]
    fn edge_table() {
        assert_eq!(edge(false, false), Edge::None);
        assert_eq!(edge(false, true), Edge::Pressed);
        assert_eq!(edge(true, true), Edge::None);
        assert_eq!(edge(true, false), Edge::Released);
    }

    #[test]
    fn toggle_and_single_shot_fire_on_release_only() {
        for mode in [RepetitionMode::Toggle, RepetitionMode::SingleShot] {
            assert!(bound_key_event(65, mode, Edge::Pressed).is_none());
            assert!(bound_key_event(65, mode, Edge::None).is_none());
            assert!(matches!(
                bound_key_event(65, mode, Edge::Released),
                Some(AppEvent::HotkeyPressed(65))
            ));
        }
    }

    #[test]
    fn hold_to_repeat_fires_on_both_edges() {
        assert!(matches!(
            bound_key_event(65, RepetitionMode::HoldToRepeat, Edge::Pressed),
            Some(AppEvent::HotkeyPressed(65))
        ));
        assert!(matches!(
            bound_key_event(65, RepetitionMode::HoldToRepeat, Edge::Released),
            Some(AppEvent::HotkeyReleased(65))
        ));
        assert!(bound_key_event(65, RepetitionMode::HoldToRepeat, Edge::None).is_none());
    }

    #[test]
    fn held_key_does_not_refire() {
        // A key held across many polls must produce exactly one transition
        // per physical press and release, whatever the mode.
        let states = [false, true, true, true, false, false];
        let edges: Vec<Edge> = states.windows(2).map(|w| edge(w[0], w[1])).collect();
        let fired: Vec<&Edge> = edges.iter().filter(|e| **e != Edge::None).collect();
        assert_eq!(fired, [&Edge::Pressed, &Edge::Released]);
    }
}
