use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::macro_def::{KeyAction, Macro, MacroStep};
use crate::state::{AppState, View};
use crate::{audio, input, RECORDING, RECORD_MOUSE_MOVES};

/// Begin capturing input into a new macro.
///
/// Sets the global `RECORDING` flag, installs the OS scroll hook and
/// switches the view to `Recording`.
pub fn start_recording(state: &mut AppState) {
    state.recording_name = state.next_macro_name();
    state.recording_steps.clear();
    reset_capture_bookkeeping(state);
    state.view = View::Recording;
    RECORDING.store(true, Ordering::SeqCst);
    if let Some(ref tx) = state.event_tx {
        input::start_scroll_hook(tx.clone());
    }
    audio::play_record_start();
}

/// Finish capturing and return the recorded macro.
///
/// Clears the global `RECORDING` and `RECORD_MOUSE_MOVES` flags, removes the
/// scroll hook and drains the captured steps out of `state`, so calling it
/// twice yields an empty macro.
pub fn stop_recording(state: &mut AppState) -> Macro {
    RECORDING.store(false, Ordering::SeqCst);
    RECORD_MOUSE_MOVES.store(false, Ordering::SeqCst);
    input::stop_scroll_hook();

    let mut mac = Macro::new(state.recording_name.clone());
    mac.steps = std::mem::take(&mut state.recording_steps);
    reset_capture_bookkeeping(state);
    // Mouse-move tracking is opt-in per recording; only stop clears it.
    state.recording_mouse_moves = false;
    mac
}

/// Clear the per-recording capture state shared by start and stop.
fn reset_capture_bookkeeping(state: &mut AppState) {
    state.recording_last_instant = None;
    state.recording_pressed_keys.clear();
    state.recording_pressed_mouse.clear();
    state.recording_paused = false;
}

/// Pause or resume capture, discarding the time spent paused.
pub fn toggle_pause(state: &mut AppState) {
    state.recording_paused = !state.recording_paused;
    if state.recording_paused {
        // When pausing, clear the last instant so resuming doesn't count pause time
        state.recording_last_instant = None;
    }
}

/// Append a captured input event to the recording.
///
/// Drops release events with no matching press, and records the delay since
/// the previous event (zero for the first). Ignored while paused.
pub fn process_record_event(state: &mut AppState, action: KeyAction, instant: Instant) {
    if state.recording_paused {
        return;
    }

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
    // Past this the load clamp would silently drop the tail at next launch.
    if state.recording_steps.len() < crate::macro_def::MAX_STEPS {
        state.recording_steps.push(MacroStep { action, delay_ms });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macro_def::MouseButton;
    use std::time::Duration;

    fn recording_state() -> AppState {
        // See main.rs tests: real audio threads must not outlive the tests.
        crate::AUDIO_ENABLED.store(false, Ordering::SeqCst);
        AppState::new()
    }

    #[test]
    fn resume_after_pause_does_not_bake_in_the_pause_time() {
        let mut s = recording_state();
        let t0 = Instant::now();
        process_record_event(&mut s, KeyAction::KeyDown(30), t0);
        process_record_event(&mut s, KeyAction::KeyUp(30), t0 + Duration::from_millis(20));

        toggle_pause(&mut s);
        toggle_pause(&mut s);

        // A long pause elapsed between the key-up and this event.
        process_record_event(&mut s, KeyAction::KeyDown(31), t0 + Duration::from_secs(60));
        assert_eq!(
            s.recording_steps.last().unwrap().delay_ms,
            0,
            "the first post-resume event must not inherit the pause duration"
        );
    }

    #[test]
    fn events_while_paused_are_dropped() {
        let mut s = recording_state();
        toggle_pause(&mut s);
        process_record_event(&mut s, KeyAction::KeyDown(30), Instant::now());
        assert!(s.recording_steps.is_empty());
    }

    #[test]
    fn stop_recording_drains_the_captured_steps() {
        let mut s = recording_state();
        crate::recorder::start_recording(&mut s);
        process_record_event(&mut s, KeyAction::KeyDown(30), Instant::now());
        let mac = stop_recording(&mut s);
        assert_eq!(mac.steps.len(), 1);
        assert!(s.recording_steps.is_empty(), "stop must drain the buffer");

        let empty = stop_recording(&mut s);
        assert!(empty.steps.is_empty(), "double stop yields an empty macro");
    }

    #[test]
    fn orphan_release_events_are_dropped() {
        let mut s = recording_state();
        let now = Instant::now();

        process_record_event(&mut s, KeyAction::KeyUp(30), now);
        assert!(s.recording_steps.is_empty(), "release with no matching press");

        process_record_event(&mut s, KeyAction::MouseUp(MouseButton::Left), now);
        assert!(s.recording_steps.is_empty(), "mouse release with no matching press");
    }

    #[test]
    fn matched_press_release_pair_is_recorded() {
        let mut s = recording_state();
        let now = Instant::now();

        process_record_event(&mut s, KeyAction::KeyDown(30), now);
        process_record_event(&mut s, KeyAction::KeyUp(30), now + Duration::from_millis(40));

        assert_eq!(s.recording_steps.len(), 2);
        assert_eq!(s.recording_steps[0].delay_ms, 0, "first event has no delay");
        assert_eq!(s.recording_steps[1].delay_ms, 40);
    }

    #[test]
    fn events_are_ignored_while_paused() {
        let mut s = recording_state();
        s.recording_paused = true;

        process_record_event(&mut s, KeyAction::KeyDown(30), Instant::now());
        assert!(s.recording_steps.is_empty());
    }

    #[test]
    fn release_is_dropped_only_once_per_press() {
        let mut s = recording_state();
        let now = Instant::now();

        process_record_event(&mut s, KeyAction::KeyDown(30), now);
        process_record_event(&mut s, KeyAction::KeyUp(30), now);
        process_record_event(&mut s, KeyAction::KeyUp(30), now);

        assert_eq!(s.recording_steps.len(), 2, "the second release is an orphan");
    }
}
