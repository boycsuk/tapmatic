use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::input;
use crate::macro_def::{KeyAction, MouseButton, RepetitionMode};
use crate::state::AppEvent;
use crate::{AWAITING_HOTKEY, MOUSE_MOVE_THRESHOLD, QUICK_RECORD, RECORDING, RECORD_MOUSE_MOVES, RUNNING};

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Recording,
    Capture,
}

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
            for _ in 0..15 {
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

fn capture_scan(tx: &mpsc::Sender<AppEvent>, prev_state: &mut [bool; 256]) {
    for vk in 0x01i32..=0xFE {
        if vk == VK_ESCAPE {
            continue;
        }
        let pressed = input::is_key_pressed(vk);
        let was = prev_state[vk as usize];
        if was && !pressed {
            let _ = tx.send(AppEvent::HotkeyCaptured(vk));
        }
        prev_state[vk as usize] = pressed;
    }
}

fn record_scan(
    tx: &mpsc::Sender<AppEvent>,
    prev_state: &mut [bool; 256],
    last_mouse_pos: &mut (i32, i32),
) {
    // Quick-record stop: Alt+Z during recording
    if QUICK_RECORD.load(Ordering::SeqCst) {
        const VK_Z: i32 = 0x5A;
        let z_pressed = input::is_key_pressed(VK_Z);
        let z_was = prev_state[VK_Z as usize];
        if z_was && !z_pressed && input::is_key_pressed(0x12) { // VK_MENU (Alt)
            let _ = tx.send(AppEvent::QuickRecordToggle);
            prev_state[VK_Z as usize] = z_pressed;
            return;
        }
        prev_state[VK_Z as usize] = z_pressed;
    }

    // Esc = stop recording (falling edge)
    {
        let esc_pressed = input::is_key_pressed(VK_ESCAPE);
        let esc_was = prev_state[VK_ESCAPE as usize];
        if esc_was && !esc_pressed {
            let _ = tx.send(AppEvent::RecordStop);
            prev_state[VK_ESCAPE as usize] = esc_pressed;
            return;
        }
        prev_state[VK_ESCAPE as usize] = esc_pressed;
    }

    // Recording control keys (falling edge): F1=pause, F2=mouse, F3=threshold
    // Detected here so they work without TUI focus; excluded from recorded steps below.
    const VK_F1: i32 = 0x70;
    const VK_F2: i32 = 0x71;
    const VK_F3: i32 = 0x72;
    for &(vk, is_f1, is_f2) in &[(VK_F1, true, false), (VK_F2, false, true), (VK_F3, false, false)] {
        let pressed = input::is_key_pressed(vk);
        let was = prev_state[vk as usize];
        if was && !pressed {
            let ev = if is_f1 { AppEvent::RecordPauseToggle }
                else if is_f2 { AppEvent::RecordMouseToggle }
                else { AppEvent::RecordThresholdCycle };
            let _ = tx.send(ev);
        }
        prev_state[vk as usize] = pressed;
    }

    let now = Instant::now();

    // Continuous mouse movement recording
    if RECORD_MOUSE_MOVES.load(Ordering::SeqCst) {
        let pos = input::get_cursor_pos();
        if pos != *last_mouse_pos {
            let threshold = MOUSE_MOVE_THRESHOLD.load(Ordering::Relaxed) as i32;
            let dx = (pos.0 - last_mouse_pos.0).abs();
            let dy = (pos.1 - last_mouse_pos.1).abs();
            if dx >= threshold || dy >= threshold {
                let _ = tx.send(AppEvent::RecordKey(KeyAction::MouseMove(pos.0, pos.1), now));
                *last_mouse_pos = pos;
            }
        }
    }

    // Mouse buttons (0x01=LMB, 0x02=RMB, 0x04=MMB)
    for &(vk, button) in &[
        (0x01, MouseButton::Left),
        (0x02, MouseButton::Right),
        (0x04, MouseButton::Middle),
    ] {
        let pressed = input::is_key_pressed(vk);
        let was = prev_state[vk as usize];
        if pressed && !was {
            // Record mouse position before mouse-down events
            let pos = input::get_cursor_pos();
            let mut click_time = now;
            if pos != *last_mouse_pos {
                let _ = tx.send(AppEvent::RecordKey(KeyAction::MouseMove(pos.0, pos.1), now));
                *last_mouse_pos = pos;
                // Small gap so the click doesn't arrive before the move completes
                click_time = now + Duration::from_millis(10);
            }
            let _ = tx.send(AppEvent::RecordKey(KeyAction::MouseDown(button), click_time));
        } else if !pressed && was {
            let _ = tx.send(AppEvent::RecordKey(KeyAction::MouseUp(button), now));
        }
        prev_state[vk as usize] = pressed;
    }

    // Keyboard keys (0x08 onwards, skip Escape, control keys, and Alt+Z combo)
    for vk in 0x08i32..=0xFE {
        if vk == VK_ESCAPE || vk == VK_F1 || vk == VK_F2 || vk == VK_F3 {
            continue;
        }
        // Skip Z when Alt is held (quick-record combo)
        if vk == 0x5A && input::is_key_pressed(0x12) {
            prev_state[vk as usize] = input::is_key_pressed(vk);
            continue;
        }
        let pressed = input::is_key_pressed(vk);
        let was = prev_state[vk as usize];
        if pressed && !was {
            let sc = input::vk_to_scancode(vk);
            if sc != 0 {
                let _ = tx.send(AppEvent::RecordKey(KeyAction::KeyDown(sc), now));
            }
        } else if !pressed && was {
            let sc = input::vk_to_scancode(vk);
            if sc != 0 {
                let _ = tx.send(AppEvent::RecordKey(KeyAction::KeyUp(sc), now));
            }
        }
        prev_state[vk as usize] = pressed;
    }
}

fn normal_scan(
    tx: &mpsc::Sender<AppEvent>,
    config: &Arc<Mutex<HotkeyConfig>>,
    prev_state: &mut [bool; 256],
) {
    // Quick-record: Alt+Z (falling edge on Z while Alt held)
    const VK_Z: i32 = 0x5A;
    let z_pressed = input::is_key_pressed(VK_Z);
    let z_was = prev_state[VK_Z as usize];
    if z_was && !z_pressed && input::is_key_pressed(0x12) { // VK_MENU (Alt)
        let _ = tx.send(AppEvent::QuickRecordToggle);
    }
    prev_state[VK_Z as usize] = z_pressed;

    let keys = if let Ok(cfg) = config.lock() {
        cfg.bound_keys.clone()
    } else {
        return;
    };

    for (vk, mode) in &keys {
        let vk = *vk;
        if vk <= 0 || vk >= 256 {
            continue;
        }
        let pressed = input::is_key_pressed(vk);
        let was = prev_state[vk as usize];
        match mode {
            RepetitionMode::Toggle | RepetitionMode::SingleShot => {
                if was && !pressed {
                    let _ = tx.send(AppEvent::HotkeyPressed(vk));
                }
            }
            RepetitionMode::HoldToRepeat => {
                if !was && pressed {
                    let _ = tx.send(AppEvent::HotkeyPressed(vk));
                }
                if was && !pressed {
                    let _ = tx.send(AppEvent::HotkeyReleased(vk));
                }
            }
        }
        prev_state[vk as usize] = pressed;
    }
}
