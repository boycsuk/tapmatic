use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::macro_def::{KeyAction, Macro, MacroStep};
use crate::state::{AppState, View};
use crate::{audio, input, RECORDING, RECORD_MOUSE_MOVES};

pub fn start_recording(state: &mut AppState) {
    state.recording_name = state.next_macro_name();
    state.recording_steps.clear();
    state.recording_last_instant = None;
    state.recording_start = Some(Instant::now());
    state.recording_pressed_keys.clear();
    state.recording_pressed_mouse.clear();
    state.recording_paused = false;
    state.view = View::Recording;
    RECORDING.store(true, Ordering::SeqCst);
    if let Some(ref tx) = state.event_tx {
        input::start_scroll_hook(tx.clone());
    }
    audio::play_record_start();
}

pub fn stop_recording(state: &mut AppState) -> Macro {
    RECORDING.store(false, Ordering::SeqCst);
    RECORD_MOUSE_MOVES.store(false, Ordering::SeqCst);
    input::stop_scroll_hook();

    let mut mac = Macro::new(state.recording_name.clone());
    mac.steps = std::mem::take(&mut state.recording_steps);
    state.recording_last_instant = None;
    state.recording_start = None;
    state.recording_pressed_keys.clear();
    state.recording_pressed_mouse.clear();
    state.recording_paused = false;
    state.recording_mouse_moves = false;
    mac
}

pub fn toggle_pause(state: &mut AppState) {
    state.recording_paused = !state.recording_paused;
    if state.recording_paused {
        // When pausing, clear the last instant so resuming doesn't count pause time
        state.recording_last_instant = None;
    }
}

pub fn process_record_event(state: &mut AppState, action: KeyAction, instant: Instant) {
    if state.recording_paused {
        return;
    }

    // Ignore orphan release events
    match action {
        KeyAction::KeyDown(sc) => {
            state.recording_pressed_keys.insert(sc);
        }
        KeyAction::KeyUp(sc) => {
            if !state.recording_pressed_keys.remove(&sc) {
                return;
            }
        }
        KeyAction::MouseDown(btn) => {
            state.recording_pressed_mouse.insert(btn);
        }
        KeyAction::MouseUp(btn) => {
            if !state.recording_pressed_mouse.remove(&btn) {
                return;
            }
        }
        KeyAction::TypeText(_) | KeyAction::MouseScroll(_, _) | KeyAction::MouseMove(_, _) | KeyAction::WaitForWindow(_) => {}
    }

    let delay_ms = if let Some(last) = state.recording_last_instant {
        instant.duration_since(last).as_millis() as u64
    } else {
        0
    };

    state.recording_last_instant = Some(instant);
    state.recording_steps.push(MacroStep { action, delay_ms });
}
