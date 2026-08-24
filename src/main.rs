mod audio;
mod executor;
mod hotkey;
mod input;
mod macro_def;
mod persistence;
mod recorder;
mod state;
mod ui;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use hotkey::HotkeyConfig;
use macro_def::{KeyAction, Macro, MacroStep, RepetitionMode, ScrollDirection, SendMode, StopCondition, vk_name};
use state::{wrap_nav, AppEvent, AppState, ConfigField, ExecutorCommand, View};

/// App-wide liveness flag; cleared once by quit so every thread exits.
pub static RUNNING: AtomicBool = AtomicBool::new(true);
/// True while a recording captures input; read by the hotkey scan thread.
pub static RECORDING: AtomicBool = AtomicBool::new(false);
/// Gates every sound; mirrors `AppState::audio_enabled` for the audio threads.
pub static AUDIO_ENABLED: AtomicBool = AtomicBool::new(true);
/// Master kill switch mirrored from `AppState::macros_enabled`; macro threads
/// stop when it goes false.
pub static MACROS_ENABLED: AtomicBool = AtomicBool::new(true);
/// True while any "press a key" capture is active: the scan thread swallows
/// key events and reports them as `HotkeyCaptured` instead of firing macros.
pub static AWAITING_HOTKEY: AtomicBool = AtomicBool::new(false);
/// Whether the active capture also accepts mouse buttons (step captures do,
/// hotkey bindings must not). Meaningless while `AWAITING_HOTKEY` is false.
pub static CAPTURE_MOUSE_BUTTONS: AtomicBool = AtomicBool::new(false);
/// The delay multiplier as raw `f64` bits: readers go through
/// `f64::from_bits`, writers through `f64::to_bits`. Initialized to 1.0.
pub static SPEED_MULTIPLIER: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0x3FF0000000000000);
/// Distinguishes an Alt+Z quick recording (commits straight to the list)
/// from a normal one (opens the config view).
pub static QUICK_RECORD: AtomicBool = AtomicBool::new(false);
/// Whether the recorder samples continuous mouse movement.
pub static RECORD_MOUSE_MOVES: AtomicBool = AtomicBool::new(false);
pub static MOUSE_MOVE_THRESHOLD: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(DEFAULT_MOUSE_MOVE_THRESHOLD);

/// Startup value for both the recorder atomic and `AppState`; must stay a
/// member of `MOUSE_THRESHOLDS` or F3 cycling restarts from the list head.
pub const DEFAULT_MOUSE_MOVE_THRESHOLD: u32 = 5;

/// Starting values when cycling a stop condition onto reps/seconds.
const DEFAULT_STOP_REPS: u32 = 10;
const DEFAULT_STOP_SECS: u32 = 30;

pub const MAIN_BUTTONS: &[&str] = &[
    "Record", "Edit", "Copy", "Delete", "Save", "Toggle", "Mute", "Quit",
];

/// Status-bar hints for [`MAIN_BUTTONS`], index-aligned with it.
pub const MAIN_BUTTON_HINTS: &[&str] = &[
    "Start recording a new macro",
    "Edit selected macro settings",
    "Duplicate selected macro",
    "Delete selected macro",
    "Save settings and macros to ~/.tapmatic.json",
    "Enable/disable all macro hotkeys",
    "Toggle audio feedback",
    "Exit tapmatic",
];

/// Indices into [`MAIN_BUTTONS`], used by the toolbar dispatch.
const BTN_RECORD: usize = 0;
const BTN_EDIT: usize = 1;
const BTN_COPY: usize = 2;
const BTN_DELETE: usize = 3;
const BTN_SAVE: usize = 4;
const BTN_TOGGLE: usize = 5;
const BTN_MUTE: usize = 6;
const BTN_QUIT: usize = 7;

/// How long a config change may sit in memory before being written to disk.
const SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// Poll the foreground window every N frames (~4x/second at 60fps).
const FOREGROUND_POLL_FRAMES: u32 = 15;
/// System stats (CPU, RAM, uptime) refresh every ~1 second at 60 fps.
const STATS_POLL_FRAMES: u32 = 60;

/// Delay multiplier bounds. Values below 1.0 speed playback up, above slow it
/// down; the UI presents the reciprocal.
const MIN_SPEED_MULTIPLIER: f64 = 0.25;
const MAX_SPEED_MULTIPLIER: f64 = 4.0;
const SPEED_MULTIPLIER_STEP: f64 = 0.25;

fn main() {
    // Panic handler: restore terminal before printing panic info
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        default_hook(info);
    }));

    input::begin_high_res_timer();

    let (config, load_error, fresh) = match persistence::load() {
        persistence::LoadOutcome::Loaded(c) => (c, None, false),
        persistence::LoadOutcome::Fresh(c) => (c, None, true),
        persistence::LoadOutcome::Corrupt(reason) => {
            (persistence::AppConfig::default(), Some(reason), false)
        }
    };

    let (event_tx, event_rx) = mpsc::channel::<AppEvent>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<ExecutorCommand>();

    let hotkey_config = Arc::new(Mutex::new(HotkeyConfig::new()));

    let mut state = AppState::new();
    state.macros = config.macros;
    state.audio_enabled = config.audio_enabled;
    state.macros_enabled = config.macros_enabled;
    state.speed_multiplier = config.speed_multiplier.clamp(MIN_SPEED_MULTIPLIER, MAX_SPEED_MULTIPLIER);
    if let Some(reason) = load_error {
        state.config_read_failed = true;
        state.set_status_persistent(format!("Config not loaded ({reason}). Saving is disabled."));
    } else if fresh {
        if state.macros.is_empty() {
            state.set_status(format!("First run — config will be saved to {}", persistence::FILE_NAME));
        } else {
            state.set_status(format!(
                "Migrated {} macro(s) from the legacy macros.json", state.macros.len()
            ));
        }
    }
    AUDIO_ENABLED.store(state.audio_enabled, Ordering::SeqCst);
    MACROS_ENABLED.store(state.macros_enabled, Ordering::SeqCst);
    SPEED_MULTIPLIER.store(state.speed_multiplier.to_bits(), Ordering::SeqCst);
    state.event_tx = Some(event_tx.clone());
    update_hotkey_config(&state, &hotkey_config);

    let hk_config = Arc::clone(&hotkey_config);
    let hk_tx = event_tx.clone();
    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(move || hotkey::hotkey_loop(hk_tx, hk_config))
        .expect("failed to spawn hotkey thread");

    let exec_tx = event_tx.clone();
    let executor_handle = std::thread::Builder::new()
        .name("executor".into())
        .spawn(move || executor::executor_loop(cmd_rx, exec_tx))
        .expect("failed to spawn executor thread");

    crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle("tapmatic")).ok();

    let mut terminal = ratatui::init();
    let result = run_app(&mut terminal, &mut state, &event_rx, &cmd_tx, &hotkey_config);
    ratatui::restore();

    // run_app flushes on a clean quit; this covers the error path.
    flush_pending_save(&mut state);

    // Stop and join the executor before dropping RUNNING: process exit kills
    // the macro threads, so leaving before they release their held keys
    // leaves those keys pressed system-wide.
    let _ = cmd_tx.send(ExecutorCommand::StopAll);
    let _ = executor_handle.join();
    RUNNING.store(false, Ordering::SeqCst);
    input::end_high_res_timer();

    if let Err(e) = result {
        eprintln!("Error: {e}");
    }
}

fn run_app(
    terminal: &mut DefaultTerminal,
    state: &mut AppState,
    event_rx: &mpsc::Receiver<AppEvent>,
    cmd_tx: &mpsc::Sender<ExecutorCommand>,
    hotkey_config: &Arc<Mutex<HotkeyConfig>>,
) -> std::io::Result<()> {
    let mut frame_count: u32 = 0;
    let mut cpu_snapshot = input::get_cpu_usage_snapshot();
    let app_start = std::time::Instant::now();
    let mut last_save = std::time::Instant::now();
    let mut last_title = String::new();

    loop {
        // The foreground window changes at human speed; polling it every frame
        // costs four syscalls and a handle open/close for nothing.
        if frame_count.is_multiple_of(FOREGROUND_POLL_FRAMES) {
            state.foreground_process = input::get_foreground_process_name();
        }

        if state.unsaved_changes && last_save.elapsed() >= SAVE_INTERVAL {
            flush_pending_save(state);
            last_save = std::time::Instant::now();
        }

        frame_count += 1;
        if frame_count.is_multiple_of(STATS_POLL_FRAMES) {
            let new_snapshot = input::get_cpu_usage_snapshot();
            state.cpu_usage = input::calc_cpu_percent(cpu_snapshot, new_snapshot);
            cpu_snapshot = new_snapshot;
            let (used, total) = input::get_ram_usage();
            state.ram_used_mb = used;
            state.ram_total_mb = total;
            state.uptime_secs = app_start.elapsed().as_secs();
        }

        // Update console title (visible in taskbar when minimized). Its
        // inputs change at human speed; rebuilding the string every frame
        // is allocation churn for a value that compares equal.
        if frame_count.is_multiple_of(FOREGROUND_POLL_FRAMES) {
            let title = if state.view == View::Recording {
                format!("tapmatic - REC ({} steps)", state.recording_steps.len())
            } else if !state.active_macros.is_empty() {
                let names: Vec<&str> = state
                    .active_macros
                    .iter()
                    .filter_map(|&vk| state.macros.iter().find(|m| m.hotkey_vk == vk).map(|m| m.name.as_str()))
                    .collect();
                format!("tapmatic - ACTIVE: {}", names.join(", "))
            } else {
                "tapmatic".into()
            };
            if title != last_title {
                let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle(&title));
                last_title = title;
            }
        }

        terminal.draw(|f| ui::draw(f, state))?;

        if state.should_quit {
            flush_pending_save(state);
            return Ok(());
        }

        state.clear_stale_status();

        while let Ok(ev) = event_rx.try_recv() {
            handle_app_event(state, ev, cmd_tx, hotkey_config);
        }

        if event::poll(Duration::from_millis(16))?
            && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press {
                    handle_key(state, key.code, key.modifiers, cmd_tx, hotkey_config);
                }
    }
}

fn macro_allowed(state: &AppState, mac: &Macro) -> bool {
    if !state.macros_enabled {
        return false;
    }
    if let Some(ref bound) = mac.bound_process {
        state
            .foreground_process
            .as_ref()
            .is_some_and(|fg| fg == bound)
    } else {
        true
    }
}

/// Stop other macros in the same exclusive group.
fn enforce_exclusive_group(
    state: &mut AppState,
    mac: &Macro,
    cmd_tx: &mpsc::Sender<ExecutorCommand>,
) {
    if let Some(ref group) = mac.exclusive_group {
        let to_stop: Vec<i32> = state
            .macros
            .iter()
            .filter(|m| {
                m.hotkey_vk != mac.hotkey_vk
                    && m.exclusive_group.as_ref() == Some(group)
                    && state.is_macro_active(m.hotkey_vk)
            })
            .map(|m| m.hotkey_vk)
            .collect();
        for vk in to_stop {
            stop_macro(state, vk, cmd_tx, false);
        }
    }
}

/// Refuse to add a macro past the persistence cap: the load clamp would
/// silently drop the excess at next launch.
fn can_add_macro(state: &mut AppState) -> bool {
    if state.macros.len() >= macro_def::MAX_MACROS {
        state.set_status(format!("Macro limit reached ({})", macro_def::MAX_MACROS));
        return false;
    }
    true
}

/// Append a quick-recorded macro and return to the main view.
fn commit_quick_recording(state: &mut AppState, mac: Macro) {
    let count = mac.steps.len();
    let idx = state.macros.len();
    state.macros.push(mac);
    // The prompt named a macro that may no longer be the resolved selection.
    state.confirm_delete = false;
    // selected_macro indexes the filtered list; clear the filter so the raw
    // index below points at the macro just recorded.
    state.search_query.clear();
    state.searching = false;
    state.selected_macro = idx;
    state.view = View::Main;
    state.set_status(format!("Quick recorded {} steps — edit with Space", count));
    autosave(state);
}

/// Mouse-move recording thresholds the F3 key cycles through, in pixels.
const MOUSE_THRESHOLDS: &[u32] = &[1, 3, 5, 10, 20];

fn next_mouse_threshold(current: u32) -> u32 {
    let pos = MOUSE_THRESHOLDS.iter().position(|&t| t == current);
    let next = pos.map_or(0, |p| (p + 1) % MOUSE_THRESHOLDS.len());
    MOUSE_THRESHOLDS[next]
}

/// Send a command to the executor, surfacing a dead executor thread instead
/// of silently losing macro control.
fn send_cmd(state: &mut AppState, cmd_tx: &mpsc::Sender<ExecutorCommand>, cmd: ExecutorCommand) -> bool {
    if cmd_tx.send(cmd).is_err() {
        state.set_status_persistent("Executor unavailable — macros cannot start or stop");
        return false;
    }
    true
}

/// Stop a macro: send the command, untrack it, and optionally beep.
fn stop_macro(
    state: &mut AppState,
    vk: i32,
    cmd_tx: &mpsc::Sender<ExecutorCommand>,
    audible: bool,
) {
    send_cmd(state, cmd_tx, ExecutorCommand::StopMacro(vk));
    state.set_macro_active(vk, false);
    if audible {
        audio::play_deactivate();
    }
}

/// Start a macro: enforce exclusive group, send command, track state.
fn start_macro(
    state: &mut AppState,
    mac: Macro,
    cmd_tx: &mpsc::Sender<ExecutorCommand>,
) {
    if mac.steps.is_empty() {
        state.set_status("Macro has no steps - nothing to start");
        return;
    }
    let vk = mac.hotkey_vk;
    enforce_exclusive_group(state, &mac, cmd_tx);
    if !send_cmd(state, cmd_tx, ExecutorCommand::StartMacro(mac)) {
        // Nothing is running: marking it active would show ACTIVE forever.
        return;
    }
    // Chained macros may be unbound; vk 0 must not enter the active list.
    if vk != 0 {
        state.set_macro_active(vk, true);
    }
    audio::play_activate();
}

fn handle_app_event(
    state: &mut AppState,
    ev: AppEvent,
    cmd_tx: &mpsc::Sender<ExecutorCommand>,
    hotkey_config: &Arc<Mutex<HotkeyConfig>>,
) {
    match ev {
        AppEvent::HotkeyPressed(vk) => {
            if let Some((_, mac)) = state.macro_by_hotkey(vk) {
                let mac = mac.clone();
                match mac.repetition {
                    RepetitionMode::Toggle => {
                        if state.is_macro_active(vk) {
                            stop_macro(state, vk, cmd_tx, true);
                        } else if macro_allowed(state, &mac) {
                            start_macro(state, mac, cmd_tx);
                        }
                    }
                    RepetitionMode::HoldToRepeat | RepetitionMode::SingleShot => {
                        if !state.is_macro_active(vk) && macro_allowed(state, &mac) {
                            start_macro(state, mac, cmd_tx);
                        }
                    }
                }
            }
        }
        AppEvent::HotkeyReleased(vk) => {
            if let Some((_, mac)) = state.macro_by_hotkey(vk)
                && mac.repetition == RepetitionMode::HoldToRepeat && state.is_macro_active(vk) {
                    stop_macro(state, vk, cmd_tx, true);
                }
        }
        AppEvent::HotkeyCaptured(vk) => {
            capture_config_binding(state, vk);
            capture_step_replacement(state, vk);
            capture_key_pair_insert(state, vk);
        }
        AppEvent::MacroProgress(vk, reps, secs) => {
            state.macro_progress.insert(vk, (reps, secs));
        }
        AppEvent::RecordKey(action, instant) => {
            if state.view == View::Recording {
                recorder::process_record_event(state, action, instant);
            }
        }
        AppEvent::MacroFinished(vk) => {
            state.set_macro_active(vk, false);
            audio::play_deactivate();
        }
        AppEvent::ChainMacro(name) => {
            if !state.macros.iter().any(|m| m.name == name) {
                state.set_status(format!("Chain target '{}' not found", name));
            }
            if let Some(mac) = state.macros.iter().find(|m| m.name == name) {
                let mac = mac.clone();
                // Chain starts obey the same gates as hotkey starts; an
                // ungated restart would bypass the kill switch and process
                // binding.
                if macro_allowed(state, &mac) && !state.is_macro_active(mac.hotkey_vk) {
                    start_macro(state, mac, cmd_tx);
                    state.set_status(format!("Chained: {}", name));
                }
            }
        }
        AppEvent::QuickRecordToggle => {
            if state.view == View::Recording {
                // Same transition as RecordStop; QUICK_RECORD routes the
                // commit, so the stop sequence exists exactly once.
                handle_app_event(state, AppEvent::RecordStop, cmd_tx, hotkey_config);
            } else if state.view == View::Main && can_add_macro(state) {
                recorder::start_recording(state);
                QUICK_RECORD.store(true, Ordering::SeqCst);
                state.set_status_persistent("Quick recording... Alt+Z to stop");
            }
        }
        AppEvent::Warning(msg) => {
            state.set_status_persistent(msg);
        }
        AppEvent::RecordStop => {
            if state.view == View::Recording {
                let mac = recorder::stop_recording(state);
                audio::play_record_stop();
                if mac.steps.is_empty() {
                    state.view = View::Main;
                    state.set_status("Recording cancelled — no keys captured");
                } else if QUICK_RECORD.load(Ordering::SeqCst) {
                    commit_quick_recording(state, mac);
                } else {
                    // Normal record: go to config
                    let count = mac.steps.len();
                    let idx = state.macros.len();
                    state.macros.push(mac);
                    state.confirm_delete = false;
                    state.search_query.clear();
                    state.searching = false;
                    state.selected_macro = idx;
                    enter_config(state, idx, ConfigEntry::FreshRecording);
                    update_hotkey_config(state, hotkey_config);
                    state.set_status(format!("Recorded {} steps — configure your macro", count));
                }
                QUICK_RECORD.store(false, Ordering::SeqCst);
            }
        }
        AppEvent::RecordPauseToggle => {
            if state.view == View::Recording {
                recorder::toggle_pause(state);
                if state.recording_paused {
                    state.set_status_persistent("Recording PAUSED — F1 to resume");
                } else {
                    state.set_status("Recording resumed");
                }
            }
        }
        AppEvent::RecordMouseToggle => {
            if state.view == View::Recording {
                state.recording_mouse_moves = !state.recording_mouse_moves;
                RECORD_MOUSE_MOVES.store(state.recording_mouse_moves, Ordering::SeqCst);
                if state.recording_mouse_moves {
                    state.set_status(format!("Mouse tracking ON ({}px)", state.mouse_move_threshold));
                } else {
                    state.set_status("Mouse tracking OFF");
                }
            }
        }
        AppEvent::RecordThresholdCycle => {
            if state.view == View::Recording {
                state.mouse_move_threshold = next_mouse_threshold(state.mouse_move_threshold);
                MOUSE_MOVE_THRESHOLD.store(state.mouse_move_threshold, Ordering::SeqCst);
                state.set_status(format!("Mouse threshold: {}px", state.mouse_move_threshold));
            }
        }
    }
}

/// Bind a captured key as a Config-view hotkey or require-held key.
fn capture_config_binding(state: &mut AppState, vk: i32) {
    if !(state.awaiting_hotkey || state.awaiting_require_held) {
        return;
    }
    let View::Config(idx) = state.view else { return };

    // Check for duplicate hotkey before the mutable borrow.
    let dupe_name = if state.awaiting_hotkey {
        state.macros.iter().enumerate()
            .find(|(i, m)| *i != idx && m.hotkey_vk == vk)
            .map(|(_, m)| m.name.clone())
    } else { None };

    let Some(mac) = state.macros.get_mut(idx) else { return };
    let label = if state.awaiting_hotkey {
        mac.hotkey_vk = vk;
        if let Some(other) = dupe_name {
            format!("Hotkey [{}] — WARNING: also used by '{}'", vk_name(vk), other)
        } else {
            format!("Hotkey set to [{}]", vk_name(vk))
        }
    } else {
        mac.require_held_vk = vk;
        format!("Require held: [{}]", vk_name(vk))
    };
    end_key_capture(state);
    state.set_status(label);
}

/// What a "press a key" capture is for; step captures also accept mouse
/// buttons, hotkey bindings must not (a click on the UI would bind the macro
/// to that button and fire it on every click anywhere).
#[derive(Clone, Copy, PartialEq)]
enum KeyCapture {
    HotkeyBinding,
    RequireHeldBinding,
    StepReplace,
    StepInsert,
}

/// Enter key-capture mode, keeping the AppState flags and the atomics the
/// hotkey thread reads in lockstep — one missed store here silently eats all
/// keyboard input.
fn begin_key_capture(state: &mut AppState, mode: KeyCapture) {
    match mode {
        KeyCapture::HotkeyBinding => state.awaiting_hotkey = true,
        KeyCapture::RequireHeldBinding => state.awaiting_require_held = true,
        KeyCapture::StepReplace => state.editing_step_action = true,
        KeyCapture::StepInsert => state.inserting_key_step = true,
    }
    AWAITING_HOTKEY.store(true, Ordering::SeqCst);
    CAPTURE_MOUSE_BUTTONS.store(capture_accepts_mouse(mode), Ordering::SeqCst);
}

/// Step captures accept mouse buttons; bindings are keyboard-only.
fn capture_accepts_mouse(mode: KeyCapture) -> bool {
    matches!(mode, KeyCapture::StepReplace | KeyCapture::StepInsert)
}

/// Leave key-capture mode. Only one capture is ever active, so clearing all
/// four flags is safe from any exit path.
fn end_key_capture(state: &mut AppState) {
    state.awaiting_hotkey = false;
    state.awaiting_require_held = false;
    state.editing_step_action = false;
    state.inserting_key_step = false;
    AWAITING_HOTKEY.store(false, Ordering::SeqCst);
    CAPTURE_MOUSE_BUTTONS.store(false, Ordering::SeqCst);
}

/// Replace the selected step's action with a captured key, keeping direction.
fn capture_step_replacement(state: &mut AppState, vk: i32) {
    if !state.editing_step_action {
        return;
    }
    let View::StepEditor(idx) = state.view else { return };

    let sc = input::vk_to_scancode(vk);
    if let Some(step) = state.macros[idx].steps.get_mut(state.selected_step) {
        let is_down = matches!(step.action, KeyAction::KeyDown(_) | KeyAction::MouseDown(_));
        if let Some(new_action) = KeyAction::from_vk(vk, sc, is_down) {
            let name = new_action.display_name();
            step.action = new_action;
            state.set_status(format!("Action changed to {}", name));
        }
    }
    end_key_capture(state);
}

/// Insert a captured key as a down+up step pair after the selection.
fn capture_key_pair_insert(state: &mut AppState, vk: i32) {
    if !state.inserting_key_step {
        return;
    }
    let View::StepEditor(idx) = state.view else { return };

    let sc = input::vk_to_scancode(vk);
    if let (Some(action_down), Some(action_up)) = (
        KeyAction::from_vk(vk, sc, true),
        KeyAction::from_vk(vk, sc, false),
    ) {
        let name = action_down.display_name();
        let pos = insert_step_after_selection(state, idx, MacroStep { action: action_down, delay_ms: 0 });
        state.macros[idx].steps.insert(pos + 1, MacroStep { action: action_up, delay_ms: 50 });
        state.selected_step = pos;
        state.set_status(format!("Inserted {} (down+up)", name));
    }
    end_key_capture(state);
}

fn handle_key(
    state: &mut AppState,
    key: KeyCode,
    mods: KeyModifiers,
    cmd_tx: &mpsc::Sender<ExecutorCommand>,
    hotkey_config: &Arc<Mutex<HotkeyConfig>>,
) {
    match &state.view {
        View::Main => handle_key_main(state, key, mods, cmd_tx, hotkey_config),
        // Recording controls (Esc, F1-F3, Alt+Z) arrive as AppEvents from the
        // hotkey thread so they work without TUI focus; keys here are ignored.
        View::Recording => {}
        View::Config(_) => handle_key_config(state, key, mods, hotkey_config),
        View::StepEditor(_) => handle_key_step_editor(state, key, mods),
        View::ProcessPicker(_) => handle_key_process_picker(state, key),
        View::ChainPicker(_) => handle_key_chain_picker(state, key),
        View::Help => handle_key_help(state, key),
    }
}

fn activate_button(
    state: &mut AppState,
    cmd_tx: &mpsc::Sender<ExecutorCommand>,
    hotkey_config: &Arc<Mutex<HotkeyConfig>>,
) {
    match state.selected_button {
        BTN_RECORD => {
            if !can_add_macro(state) {
                return;
            }
            recorder::start_recording(state);
            update_hotkey_config(state, hotkey_config);
        }
        BTN_EDIT => {
            if let Some(real_idx) = resolve_selected_macro(state) {
                enter_config(state, real_idx, ConfigEntry::ExistingMacro);
            } else {
                state.set_status("No macros to edit. Record one first");
            }
        }
        BTN_COPY => {
            if let Some(real_idx) = resolve_selected_macro(state) {
                if !can_add_macro(state) {
                    return;
                }
                let src_name = state.macros[real_idx].name.clone();
                let mut copy = state.macros[real_idx].clone();
                copy.name = format!("{} (copy)", copy.name);
                copy.hotkey_vk = 0;
                state.macros.push(copy);
                state.search_query.clear(); // clear search to see the new macro
                state.selected_macro = state.macros.len() - 1;
                state.set_status(format!("Duplicated '{}'", src_name));
                update_hotkey_config(state, hotkey_config);
                autosave(state);
            } else {
                state.set_status("No macros to copy. Record one first");
            }
        }
        BTN_DELETE => {
            if let Some(real_idx) = resolve_selected_macro(state) {
                state.confirm_delete = true;
                let name = &state.macros[real_idx].name;
                state.set_status_persistent(format!("Delete '{}'? Enter=yes  Esc=no", name));
            } else {
                state.set_status("No macros to delete");
            }
        }
        BTN_SAVE => {
            autosave(state);
            state.set_status(format!("Saved {} macros to {}", state.macros.len(), persistence::FILE_NAME));
        }
        BTN_TOGGLE => {
            state.macros_enabled = !state.macros_enabled;
            MACROS_ENABLED.store(state.macros_enabled, Ordering::SeqCst);
            if !state.macros_enabled {
                for vk in std::mem::take(&mut state.active_macros) {
                    send_cmd(state, cmd_tx, ExecutorCommand::StopMacro(vk));
                }
            }
            if state.macros_enabled {
                state.set_status("Macros enabled — hotkeys will trigger macros");
            } else {
                state.set_status("Macros disabled — all hotkeys paused");
            }
            autosave(state);
        }
        BTN_MUTE => {
            state.audio_enabled = !state.audio_enabled;
            AUDIO_ENABLED.store(state.audio_enabled, Ordering::SeqCst);
            if state.audio_enabled {
                state.set_status("Audio on — sounds will play");
            } else {
                state.set_status("Audio muted");
            }
            autosave(state);
        }
        BTN_QUIT => {
            request_quit(state);
        }
        _ => {}
    }
}

/// Resolve the real macro index from the current selection (accounting for search filter).
fn resolve_selected_macro(state: &AppState) -> Option<usize> {
    let filtered = state.filtered_macro_indices();
    filtered.get(state.selected_macro).copied()
}

/// How the Config view is being opened.
#[derive(Clone, Copy, PartialEq)]
enum ConfigEntry {
    /// Editing an existing macro: undoable, Esc reverts to a backup.
    ExistingMacro,
    /// Configuring a just-recorded macro: nothing to revert to.
    FreshRecording,
}

fn enter_config(state: &mut AppState, idx: usize, entry: ConfigEntry) {
    state.clear_status();
    if entry == ConfigEntry::ExistingMacro {
        state.push_undo();
        state.config_macro_backup = Some(state.macros[idx].clone());
    } else {
        state.config_macro_backup = None;
    }
    state.config_tab = 0;
    state.config_field = ConfigField::Name;
    state.config_input_buf = state.macros[idx].name.clone();
    state.text_cursor = state.config_input_buf.chars().count();
    state.awaiting_hotkey = false;
    state.view = View::Config(idx);
}

fn handle_key_main(
    state: &mut AppState,
    key: KeyCode,
    mods: KeyModifiers,
    cmd_tx: &mpsc::Sender<ExecutorCommand>,
    hotkey_config: &Arc<Mutex<HotkeyConfig>>,
) {
    if state.renaming {
        match handle_modal_input(state, TextField::Rename, key, mods) {
            ModalOutcome::Committed(name) => {
                if let Some(real_idx) = resolve_selected_macro(state)
                    && !name.is_empty() {
                        let old_name = state.macros[real_idx].name.clone();
                        state.macros[real_idx].name = name.clone();
                        state.macros[real_idx].clamp_to_valid_ranges();
                        let new_name = state.macros[real_idx].name.clone();
                        if new_name != old_name {
                            retarget_chains(state, &old_name, &new_name);
                        }
                        state.set_status(format!("Renamed to '{}'", new_name));
                        autosave(state);
                    }
                state.renaming = false;
            }
            ModalOutcome::Cancelled => {
                state.renaming = false;
                state.set_status("Rename cancelled");
            }
            ModalOutcome::Editing => {}
        }
        return;
    }

    if state.searching {
        match key {
            KeyCode::Enter | KeyCode::Esc => {
                state.searching = false;
                if key == KeyCode::Esc {
                    state.search_query.clear();
                }
            }
            _ => {
                let mut buf = state.search_query.clone();
                if handle_text_motion(state, &mut buf, key, mods, true, true) {
                    state.search_query = buf;
                    state.selected_macro = 0;
                }
            }
        }
        return;
    }

    if state.confirm_delete {
        match key {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let Some(real_idx) = resolve_selected_macro(state) {
                    state.push_undo();
                    let name = state.macros[real_idx].name.clone();
                    let vk = state.macros[real_idx].hotkey_vk;
                    if state.is_macro_active(vk) {
                        stop_macro(state, vk, cmd_tx, false);
                    }
                    state.macros.remove(real_idx);
                    let mut dangling = 0;
                    for m in &mut state.macros {
                        if m.chain_macro.as_deref() == Some(name.as_str()) {
                            m.chain_macro = None;
                            dangling += 1;
                        }
                    }
                    state.clamp_selection();
                    if dangling > 0 {
                        state.set_status(format!(
                            "Deleted '{}' — cleared its chain reference in {} macro(s)", name, dangling
                        ));
                    } else {
                        state.set_status(format!("Deleted '{}'", name));
                    }
                    update_hotkey_config(state, hotkey_config);
                    autosave(state);
                }
                state.confirm_delete = false;
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                state.confirm_delete = false;
                state.set_status("Delete cancelled");
            }
            // Ignore other keys — don't cancel on accident
            _ => {}
        }
        return;
    }

    if state.confirm_quit {
        match key {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                state.should_quit = true;
            }
            _ => {
                state.confirm_quit = false;
                state.set_status("Quit cancelled");
            }
        }
        return;
    }

    match key {
        KeyCode::Up => state.move_selection(true),
        KeyCode::Down => state.move_selection(false),

        KeyCode::Left => wrap_nav(&mut state.selected_button, MAIN_BUTTONS.len(), true),
        KeyCode::Right => wrap_nav(&mut state.selected_button, MAIN_BUTTONS.len(), false),

        KeyCode::Enter => {
            activate_button(state, cmd_tx, hotkey_config);
        }

        KeyCode::Char(' ') => {
            if let Some(real_idx) = resolve_selected_macro(state) {
                enter_config(state, real_idx, ConfigEntry::ExistingMacro);
            } else if can_add_macro(state) {
                recorder::start_recording(state);
                update_hotkey_config(state, hotkey_config);
            }
        }

        KeyCode::Char('z') | KeyCode::Char('Z') => {
            if state.pop_undo() {
                update_hotkey_config(state, hotkey_config);
                autosave(state);
                state.set_status("Undone");
            } else {
                state.set_status("Nothing to undo");
            }
        }
        KeyCode::F(2) => {
            if let Some(real_idx) = resolve_selected_macro(state) {
                state.renaming = true;
                state.rename_buf = state.macros[real_idx].name.clone();
                state.text_cursor = state.rename_buf.chars().count();
                state.set_status_persistent("Type new name, Enter to confirm...");
            }
        }
        KeyCode::Char('k') | KeyCode::Char('K') => {
            if let Some(real_idx) = resolve_selected_macro(state)
                && real_idx > 0 {
                    state.push_undo();
                    state.macros.swap(real_idx, real_idx - 1);
                    state.selected_macro = state.selected_macro.saturating_sub(1);
                    autosave(state);
                }
        }
        KeyCode::Char('j') | KeyCode::Char('J') => {
            if let Some(real_idx) = resolve_selected_macro(state)
                && real_idx + 1 < state.macros.len() {
                    state.push_undo();
                    state.macros.swap(real_idx, real_idx + 1);
                    if state.selected_macro + 1 < state.filtered_macro_indices().len() {
                        state.selected_macro += 1;
                    }
                    autosave(state);
                }
        }
        KeyCode::Char('/') => {
            state.searching = true;
            state.search_query.clear();
            state.text_cursor = 0;
            state.set_status_persistent("Type to search...");
        }
        KeyCode::F(3) => {
            state.sort_mode = match state.sort_mode {
                state::SortMode::None => state::SortMode::Name,
                state::SortMode::Name => state::SortMode::Hotkey,
                state::SortMode::Hotkey => state::SortMode::None,
            };
            state.push_undo();
            state.apply_sort();
            autosave(state);
            let label = match state.sort_mode {
                state::SortMode::None => "No sort",
                state::SortMode::Name => "Sorted by name",
                state::SortMode::Hotkey => "Sorted by hotkey",
            };
            state.set_status(label);
            update_hotkey_config(state, hotkey_config);
        }
        KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char('-') | KeyCode::Char('0') => {
            state.speed_multiplier = match key {
                KeyCode::Char('+') | KeyCode::Char('=') => (state.speed_multiplier + SPEED_MULTIPLIER_STEP).min(MAX_SPEED_MULTIPLIER),
                KeyCode::Char('-') => (state.speed_multiplier - SPEED_MULTIPLIER_STEP).max(MIN_SPEED_MULTIPLIER),
                _ => 1.0,
            };
            SPEED_MULTIPLIER.store(state.speed_multiplier.to_bits(), Ordering::SeqCst);
            let label = match state.speed_display() {
                None => "Speed: x1.00 (reset)".to_string(),
                Some(display) => format!("Speed: x{:.2}", display),
            };
            state.set_status(label);
            autosave(state);
        }
        KeyCode::Char('?') => {
            state.help_scroll = 0;
            state.view = View::Help;
        }
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            request_quit(state);
        }
        KeyCode::F(12) => {
            state.console_hidden = !state.console_hidden;
            input::set_console_visible(!state.console_hidden);
            if !state.console_hidden {
                state.set_status("Window restored");
            }
        }
        _ => {}
    }
}

fn save_config(state: &mut AppState, idx: usize, hotkey_config: &Arc<Mutex<HotkeyConfig>>) {
    apply_config_field(state, idx);
    state.config_macro_backup = None;
    state.view = View::Main;
    update_hotkey_config(state, hotkey_config);
    state.set_status(format!("Saved '{}'", state.macros[idx].name));
    autosave(state);
}

/// Handle common text editing motions. Returns true if the key was consumed.
fn handle_text_motion(state: &mut AppState, buf: &mut String, key: KeyCode, mods: KeyModifiers, allow_text: bool, allow_digits: bool) -> bool {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    match key {
        KeyCode::Left if ctrl => { state.buf_cursor_word_left(buf); true }
        KeyCode::Right if ctrl => { state.buf_cursor_word_right(buf); true }
        KeyCode::Left => { state.buf_cursor_left(); true }
        KeyCode::Right => { state.buf_cursor_right(buf.chars().count()); true }
        KeyCode::Home => { state.buf_cursor_home(); true }
        KeyCode::End => { state.buf_cursor_end_pos(buf); true }
        KeyCode::Delete => { state.buf_delete(buf); true }
        KeyCode::Backspace if ctrl => { state.buf_backspace_word(buf); true }
        KeyCode::Backspace => { state.buf_backspace(buf); true }
        KeyCode::Char(c) if allow_text => { state.buf_insert(buf, c); true }
        KeyCode::Char(c) if allow_digits && c.is_ascii_digit() => { state.buf_insert(buf, c); true }
        _ => false,
    }
}

fn request_quit(state: &mut AppState) {
    if state.active_macros.is_empty() {
        state.should_quit = true;
    } else {
        state.confirm_quit = true;
        let count = state.active_macros.len();
        state.set_status_persistent(format!(
            "{} macro(s) running. Quit? Enter=yes  any key=no", count
        ));
    }
}

/// Mark the config dirty so the event loop flushes it shortly.
///
/// Called from every mutating key handler, so it must not touch the disk: the
/// write happens in `flush_pending_save`, at most once per `SAVE_INTERVAL`.
fn autosave(state: &mut AppState) {
    if state.config_read_failed {
        state.set_status_persistent("Not saving: config file unreadable at startup");
        return;
    }
    state.unsaved_changes = true;
}

/// Write the config if it is dirty, reporting failures in the status bar.
fn flush_pending_save(state: &mut AppState) {
    if !state.unsaved_changes || state.config_read_failed {
        return;
    }
    let config = persistence::AppConfig {
        audio_enabled: state.audio_enabled,
        macros_enabled: state.macros_enabled,
        speed_multiplier: state.speed_multiplier,
        macros: state.macros.clone(),
    };
    match persistence::save(&config) {
        Ok(()) => state.unsaved_changes = false,
        // Keep the dirty flag: the failure may be transient (disk full, file
        // locked), and clearing it would silently drop the session's edits.
        Err(e) => state.set_status_persistent(format!("Save failed: {e} — will retry")),
    }
}

fn handle_key_config(
    state: &mut AppState,
    key: KeyCode,
    mods: KeyModifiers,
    hotkey_config: &Arc<Mutex<HotkeyConfig>>,
) {
    let idx = match state.view {
        View::Config(i) => i,
        _ => return,
    };

    // While awaiting hotkey, only Esc cancels — everything else is captured
    // by the hotkey thread (keyboard only: capture_scan drops mouse buttons
    // for bindings).
    // Crossterm might also fire, so just ignore everything except Esc here.
    if state.awaiting_hotkey || state.awaiting_require_held {
        if key == KeyCode::Esc {
            end_key_capture(state);
            state.set_status("Binding cancelled");
        }
        return;
    }

    match key {
        KeyCode::Esc => {
            apply_config_field(state, idx);
            if let Some(backup) = state.config_macro_backup.take() {
                state.macros[idx] = backup;
            } else {
                state.macros.remove(idx);
                if state.selected_macro >= state.macros.len() && state.selected_macro > 0 {
                    state.selected_macro -= 1;
                }
            }
            state.view = View::Main;
            update_hotkey_config(state, hotkey_config);
        }

        KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab => {
            apply_config_field(state, idx);
            match key {
                KeyCode::Up => state.config_field = state.config_field.prev_in_tab(state.config_tab),
                KeyCode::Down => state.config_field = state.config_field.next_in_tab(state.config_tab),
                _ => {
                    let tab_count = state::CONFIG_TABS.len();
                    state.config_tab = if key == KeyCode::Tab {
                        (state.config_tab + 1) % tab_count
                    } else {
                        (state.config_tab + tab_count - 1) % tab_count
                    };
                    state.config_field = state::CONFIG_TABS[state.config_tab].1[0];
                }
            }
            prefill_config_buf(state, idx);
        }

        KeyCode::Left | KeyCode::Right => {
            let forward = key == KeyCode::Right;
            match state.config_field {
                ConfigField::RepetitionMode => {
                    state.macros[idx].repetition = if forward {
                        state.macros[idx].repetition.next()
                    } else {
                        state.macros[idx].repetition.prev()
                    };
                }
                ConfigField::DelayMode => {
                    state.macros[idx].use_recorded_delays = !state.macros[idx].use_recorded_delays;
                }
                ConfigField::StopCondition => {
                    state.macros[idx].stop_condition = match (state.macros[idx].stop_condition, forward) {
                        (StopCondition::None, true) => StopCondition::AfterReps(DEFAULT_STOP_REPS),
                        (StopCondition::None, false) => StopCondition::AfterSecs(DEFAULT_STOP_SECS),
                        (StopCondition::AfterReps(_), true) => StopCondition::AfterSecs(DEFAULT_STOP_SECS),
                        (StopCondition::AfterReps(_), false) => StopCondition::None,
                        (StopCondition::AfterSecs(_), true) => StopCondition::None,
                        (StopCondition::AfterSecs(_), false) => StopCondition::AfterReps(DEFAULT_STOP_REPS),
                    };
                }
                ConfigField::SendMode => {
                    state.macros[idx].send_mode = match state.macros[idx].send_mode {
                        SendMode::Global => SendMode::Window,
                        SendMode::Window => SendMode::Global,
                    };
                    // Without a bound process, Window mode silently degrades
                    // to global SendInput (executor::HeldInputs::new).
                    if state.macros[idx].send_mode == SendMode::Window
                        && state.macros[idx].bound_process.is_none()
                    {
                        state.set_status_persistent(
                            "Window mode needs a bound process — set one or input stays global",
                        );
                    }
                }
                _ => {
                    // Text/numeric fields: same motion semantics as every
                    // other buffer, via the shared handler.
                    if state.config_field.is_text_field() || state.config_field.is_numeric_field() {
                        let mut buf = state.config_input_buf.clone();
                        let key = if forward { KeyCode::Right } else { KeyCode::Left };
                        handle_text_motion(state, &mut buf, key, mods, false, false);
                        state.config_input_buf = buf;
                    }
                }
            }
        }

        KeyCode::Enter => {
            apply_config_field(state, idx);
            match state.config_field {
                ConfigField::Hotkey => {
                    begin_key_capture(state, KeyCapture::HotkeyBinding);
                    state.set_status_persistent("Press any key...");
                }
                ConfigField::RequireHeld => {
                    begin_key_capture(state, KeyCapture::RequireHeldBinding);
                    state.set_status_persistent("Press the key that must be held...");
                }
                ConfigField::ChainMacro => {
                    state.chain_list = state
                        .macros
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != idx)
                        .map(|(_, m)| m.name.clone())
                        .collect();
                    state.selected_chain = 0;
                    state.view = View::ChainPicker(idx);
                }
                ConfigField::BoundProcess => {
                    state.process_list = input::get_window_list();
                    state.selected_process = 0;
                    state.view = View::ProcessPicker(idx);
                }
                _ => {
                    state.config_field = state.config_field.next_in_tab(state.config_tab);
                    prefill_config_buf(state, idx);
                }
            }
        }

        KeyCode::F(5) => {
            save_config(state, idx, hotkey_config);
        }

        KeyCode::F(1) => {
            apply_config_field(state, idx);
            state.selected_step = 0;
            state.selection_anchor = None;
            state.editing_step_delay = false;
            state.step_delay_buf.clear();
            state.view = View::StepEditor(idx);
        }

        _ => {
            if state.config_field.is_text_field() || state.config_field.is_numeric_field() {
                let is_text = state.config_field.is_text_field();
                let mut buf = state.config_input_buf.clone();
                handle_text_motion(state, &mut buf, key, mods, is_text, true);
                state.config_input_buf = buf;
            }
        }
    }
}

/// Point every `chain_macro` referencing `old` at `new`: chains are stored by
/// name, so a rename would otherwise silently break them.
fn retarget_chains(state: &mut AppState, old: &str, new: &str) {
    for m in &mut state.macros {
        if m.chain_macro.as_deref() == Some(old) {
            m.chain_macro = Some(new.to_string());
        }
    }
}

fn apply_config_field(state: &mut AppState, idx: usize) {
    let input = state.config_input_buf.clone();
    let old_name = state.macros[idx].name.clone();
    state.config_field.write(&mut state.macros[idx], &input);
    // Same bounds as the load path: without this, an in-session value past a
    // limit (jitter, delays, name length) is silently changed at next launch,
    // and an oversized jitter overflows the i32 math in the executor.
    state.macros[idx].clamp_to_valid_ranges();
    let new_name = state.macros[idx].name.clone();
    if new_name != old_name {
        retarget_chains(state, &old_name, &new_name);
    }
}

fn prefill_config_buf(state: &mut AppState, idx: usize) {
    state.config_input_buf = state.config_field.read(&state.macros[idx]);
    state.text_cursor = state.config_input_buf.chars().count();
}

/// What a modal text input did with a key press.
enum ModalOutcome {
    /// Enter: the buffer is ready to be applied by the caller.
    Committed(String),
    Cancelled,
    /// The key was consumed as editing input.
    Editing,
}

/// Which buffer on `AppState` a modal input is editing.
#[derive(Clone, Copy)]
enum TextField {
    InsertText,
    ScrollClicks,
    StepDelay,
    Rename,
}

impl TextField {
    fn get(self, state: &AppState) -> &String {
        match self {
            Self::InsertText => &state.insert_text_buf,
            Self::ScrollClicks => &state.scroll_clicks_buf,
            Self::StepDelay => &state.step_delay_buf,
            Self::Rename => &state.rename_buf,
        }
    }

    fn set(self, state: &mut AppState, value: String) {
        match self {
            Self::InsertText => state.insert_text_buf = value,
            Self::ScrollClicks => state.scroll_clicks_buf = value,
            Self::StepDelay => state.step_delay_buf = value,
            Self::Rename => state.rename_buf = value,
        }
    }

    /// Numeric fields reject letters; text fields accept both.
    fn allows_text(self) -> bool {
        matches!(self, Self::InsertText | Self::Rename)
    }
}

/// Drive one key press through a modal text buffer.
///
/// Returns the buffer on Enter and clears it on either Enter or Esc, so callers
/// only decide what a committed value means.
fn handle_modal_input(
    state: &mut AppState,
    field: TextField,
    key: KeyCode,
    mods: KeyModifiers,
) -> ModalOutcome {
    match key {
        KeyCode::Enter => {
            let value = field.get(state).clone();
            field.set(state, String::new());
            ModalOutcome::Committed(value)
        }
        KeyCode::Esc => {
            field.set(state, String::new());
            ModalOutcome::Cancelled
        }
        _ => {
            let mut buf = field.get(state).clone();
            handle_text_motion(state, &mut buf, key, mods, field.allows_text(), true);
            field.set(state, buf);
            ModalOutcome::Editing
        }
    }
}

/// Handle the modal input layers of the step editor.
///
/// Returns true when the key was consumed by a modal, so the command match is
/// skipped.
fn handle_step_editor_modal(
    state: &mut AppState,
    idx: usize,
    key: KeyCode,
    mods: KeyModifiers,
) -> bool {
    // Awaiting key capture (insert or replace) — only Esc cancels
    if state.inserting_key_step || state.editing_step_action {
        if key == KeyCode::Esc {
            let msg = if state.inserting_key_step { "Insert key cancelled" } else { "Action edit cancelled" };
            end_key_capture(state);
            state.set_status(msg);
        }
        return true;
    }

    if state.inserting_text {
        match handle_modal_input(state, TextField::InsertText, key, mods) {
            ModalOutcome::Committed(text) => {
                if !text.is_empty() {
                    let step = MacroStep {
                        action: KeyAction::TypeText(text.clone()),
                        delay_ms: 0,
                    };
                    let mut text = text;
                    macro_def::truncate_on_char_boundary(&mut text, macro_def::MAX_TYPE_TEXT_LEN);
                    let pos = insert_step_after_selection(state, idx, step);
                    state.selected_step = pos;
                    state.set_status(format!("Inserted text: \"{}\"", text));
                }
                state.inserting_text = false;
            }
            ModalOutcome::Cancelled => {
                state.inserting_text = false;
                state.set_status("Insert cancelled");
            }
            ModalOutcome::Editing => {}
        }
        return true;
    }

    if state.editing_scroll_clicks {
        match handle_modal_input(state, TextField::ScrollClicks, key, mods) {
            ModalOutcome::Committed(buf) => {
                if let Ok(clicks) = buf.parse::<u32>()
                    && clicks > 0
                        && let Some(step) = state.macros[idx].steps.get_mut(state.selected_step)
                            && let KeyAction::MouseScroll(dir, ref mut c) = step.action {
                                let clamped = clicks.min(macro_def::MAX_SCROLL_CLICKS);
                                *c = clamped;
                                let label = if dir == ScrollDirection::Up { "up" } else { "down" };
                                state.set_status(format!("Scroll {} x{}", label, clamped));
                            }
                state.editing_scroll_clicks = false;
            }
            ModalOutcome::Cancelled => state.editing_scroll_clicks = false,
            ModalOutcome::Editing => {}
        }
        return true;
    }

    if state.editing_step_delay {
        match handle_modal_input(state, TextField::StepDelay, key, mods) {
            ModalOutcome::Committed(buf) => {
                if let Ok(ms) = buf.parse::<u64>()
                    && let Some(step) = state.macros[idx].steps.get_mut(state.selected_step) {
                        step.delay_ms = ms.min(macro_def::MAX_DELAY_MS);
                    }
                state.editing_step_delay = false;
            }
            ModalOutcome::Cancelled => state.editing_step_delay = false,
            ModalOutcome::Editing => {}
        }
        return true;
    }

    false
}

/// Insert a step directly after the current selection, returning its position.
fn insert_step_after_selection(state: &mut AppState, idx: usize, step: MacroStep) -> usize {
    let pos = (state.selected_step + 1).min(state.macros[idx].steps.len());
    state.macros[idx].steps.insert(pos, step);
    pos
}

fn handle_key_step_editor(state: &mut AppState, key: KeyCode, mods: KeyModifiers) {
    let idx = match state.view {
        View::StepEditor(i) => i,
        _ => return,
    };

    let step_count = state.macros[idx].steps.len();

    if handle_step_editor_modal(state, idx, key, mods) {
        return;
    }

    match key {
        KeyCode::Esc => {
            state.view = View::Config(idx);
        }
        KeyCode::Up => wrap_nav(&mut state.selected_step, step_count, true),
        KeyCode::Down => wrap_nav(&mut state.selected_step, step_count, false),
        KeyCode::Char('d') | KeyCode::Char('D')
            if step_count > 0 => {
                let (start, end) = state.step_selection_range(step_count);
                let count = end - start + 1;
                state.macros[idx].steps.drain(start..=end);
                let new_len = state.macros[idx].steps.len();
                state.selected_step = if new_len == 0 { 0 } else { start.min(new_len - 1) };
                state.selection_anchor = None;
                state.set_status(format!("Deleted {} step(s)", count));
            }
        KeyCode::Char('t') | KeyCode::Char('T') | KeyCode::Enter => {
            if let Some(step) = state.macros[idx].steps.get(state.selected_step) {
                if let KeyAction::MouseScroll(_, clicks) = &step.action {
                    state.editing_scroll_clicks = true;
                    state.scroll_clicks_buf = clicks.to_string();
                    state.text_cursor = state.scroll_clicks_buf.chars().count();
                } else {
                    state.editing_step_delay = true;
                    state.step_delay_buf = step.delay_ms.to_string();
                    state.text_cursor = state.step_delay_buf.chars().count();
                }
            }
        }
        KeyCode::Char('k') | KeyCode::Char('K')
            if state.selected_step > 0 => {
                state.macros[idx]
                    .steps
                    .swap(state.selected_step, state.selected_step - 1);
                state.selected_step -= 1;
            }
        KeyCode::Char('j') | KeyCode::Char('J')
            if state.selected_step + 1 < step_count => {
                state.macros[idx]
                    .steps
                    .swap(state.selected_step, state.selected_step + 1);
                state.selected_step += 1;
            }
        KeyCode::Char('a') | KeyCode::Char('A')
            if step_count > 0 => {
                begin_key_capture(state, KeyCapture::StepReplace);
                state.set_status_persistent("Press a key or mouse button to replace action...");
            }
        KeyCode::Char('s') | KeyCode::Char('S') => {
            if let Some(step) = state.macros[idx].steps.get_mut(state.selected_step) {
                step.action = match step.action.clone() {
                    KeyAction::KeyDown(sc) => KeyAction::KeyUp(sc),
                    KeyAction::KeyUp(sc) => KeyAction::KeyDown(sc),
                    KeyAction::MouseDown(b) => KeyAction::MouseUp(b),
                    KeyAction::MouseUp(b) => KeyAction::MouseDown(b),
                    // Steps without a direction (TypeText, scroll, move, wait)
                    other => other,
                };
                let name = step.action.display_name();
                state.set_status(format!("Swapped to {}", name));
            }
        }
        KeyCode::Char('w') | KeyCode::Char('W') => {
            let step = MacroStep {
                action: KeyAction::MouseScroll(ScrollDirection::Up, 1),
                delay_ms: 0,
            };
            state.selected_step = insert_step_after_selection(state, idx, step);
            state.set_status("Inserted Scroll Up");
        }
        KeyCode::Char('x') | KeyCode::Char('X') => {
            let step = MacroStep {
                action: KeyAction::MouseScroll(ScrollDirection::Down, 1),
                delay_ms: 0,
            };
            state.selected_step = insert_step_after_selection(state, idx, step);
            state.set_status("Inserted Scroll Down");
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            let (cx, cy) = input::get_cursor_pos().unwrap_or((0, 0));
            let step = MacroStep {
                action: KeyAction::MouseMove(cx, cy),
                delay_ms: 0,
            };
            state.selected_step = insert_step_after_selection(state, idx, step);
            state.set_status(format!("Inserted Move ({},{})", cx, cy));
        }
        KeyCode::Char('f') | KeyCode::Char('F') => {
            if let Some(fg) = state.foreground_process.clone() {
                let step = MacroStep {
                    action: KeyAction::WaitForWindow(fg.clone()),
                    delay_ms: 0,
                };
                state.selected_step = insert_step_after_selection(state, idx, step);
                state.set_status(format!("Inserted Wait for '{}'", fg));
            } else {
                state.set_status("No foreground process detected");
            }
        }
        KeyCode::Char('i') | KeyCode::Char('I') => {
            state.inserting_text = true;
            state.insert_text_buf.clear();
            state.text_cursor = 0;
            state.set_status_persistent("Type text and press Enter to insert...");
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            begin_key_capture(state, KeyCapture::StepInsert);
            state.set_status_persistent("Press a key to insert (down+up pair)...");
        }
        KeyCode::Char('c') | KeyCode::Char('C')
            if step_count > 0 => {
                let (start, end) = state.step_selection_range(step_count);
                let duped = state.macros[idx].steps[start..=end].to_vec();
                let count = duped.len();
                for (i, step) in duped.into_iter().enumerate() {
                    state.macros[idx].steps.insert(end + 1 + i, step);
                }
                state.selected_step = end + 1;
                state.selection_anchor = None;
                state.set_status(format!("Duplicated {} step(s)", count));
            }
        KeyCode::Char('y') | KeyCode::Char('Y')
            if step_count > 0 => {
                let (start, end) = state.step_selection_range(step_count);
                state.clipboard_steps = state.macros[idx].steps[start..=end].to_vec();
                let count = state.clipboard_steps.len();
                state.set_status(format!("Copied {} step(s)", count));
            }
        KeyCode::Char('p') | KeyCode::Char('P') => {
            if !state.clipboard_steps.is_empty() {
                let count = state.clipboard_steps.len();
                let mut steps = state.clipboard_steps.clone().into_iter();
                let pos = insert_step_after_selection(state, idx, steps.next().expect("non-empty"));
                for (i, step) in steps.enumerate() {
                    state.macros[idx].steps.insert(pos + 1 + i, step);
                }
                state.selected_step = pos;
                state.selection_anchor = None;
                state.set_status(format!("Pasted {} step(s)", count));
            } else {
                state.set_status("Nothing to paste — copy first with y");
            }
        }
        KeyCode::Char('v') | KeyCode::Char('V')
            if step_count > 0 => {
                if state.selection_anchor.is_some() {
                    state.selection_anchor = None;
                    state.set_status("Selection cleared");
                } else {
                    state.selection_anchor = Some(state.selected_step);
                    state.set_status("Selection started — move with Up/Down, then y/c/d");
                }
            }
        _ => {}
    }
}

fn handle_key_process_picker(state: &mut AppState, key: KeyCode) {
    let idx = match state.view {
        View::ProcessPicker(i) => i,
        _ => return,
    };

    match key {
        KeyCode::Esc => {
            state.view = View::Config(idx);
        }
        KeyCode::Up => wrap_nav(&mut state.selected_process, state.process_list.len(), true),
        KeyCode::Down => wrap_nav(&mut state.selected_process, state.process_list.len(), false),
        KeyCode::Enter => {
            if let Some((exe, _)) = state.process_list.get(state.selected_process) {
                state.macros[idx].bound_process = Some(exe.clone());
                state.config_input_buf = exe.clone();
                state.set_status(format!(
                    "Macro will only run when '{}' has focus",
                    exe
                ));
            }
            state.view = View::Config(idx);
        }
        KeyCode::F(5) => {
            state.process_list = input::get_window_list();
            state.selected_process = 0;
            state.set_status(format!("Refreshed — {} processes found", state.process_list.len()));
        }
        KeyCode::Backspace => {
            state.macros[idx].bound_process = None;
            state.config_input_buf.clear();
            state.set_status("Process binding cleared — macro will work with any window");
            state.view = View::Config(idx);
        }
        _ => {}
    }
}

fn handle_key_chain_picker(state: &mut AppState, key: KeyCode) {
    let idx = match state.view {
        View::ChainPicker(i) => i,
        _ => return,
    };

    match key {
        KeyCode::Esc => {
            state.view = View::Config(idx);
        }
        KeyCode::Up => wrap_nav(&mut state.selected_chain, state.chain_list.len(), true),
        KeyCode::Down => wrap_nav(&mut state.selected_chain, state.chain_list.len(), false),
        KeyCode::Enter => {
            if let Some(name) = state.chain_list.get(state.selected_chain) {
                state.macros[idx].chain_macro = Some(name.clone());
                state.config_input_buf = name.clone();
                state.set_status(format!("Will chain to '{}'", name));
            }
            state.view = View::Config(idx);
        }
        KeyCode::Backspace => {
            state.macros[idx].chain_macro = None;
            state.config_input_buf.clear();
            state.set_status("Chain cleared");
            state.view = View::Config(idx);
        }
        _ => {}
    }
}

fn handle_key_help(state: &mut AppState, key: KeyCode) {
    match key {
        KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
            state.view = View::Main;
        }
        KeyCode::Up => {
            state.help_scroll = state.help_scroll.saturating_sub(1);
        }
        KeyCode::Down => {
            state.help_scroll += 1;
        }
        _ => {}
    }
}

/// Publish the current macros' hotkey bindings to the scan thread.
fn update_hotkey_config(state: &AppState, config: &Arc<Mutex<HotkeyConfig>>) {
    let entries: Vec<(i32, RepetitionMode)> = state
        .macros
        .iter()
        .filter(|m| m.hotkey_vk != 0)
        .map(|m| (m.hotkey_vk, m.repetition))
        .collect();
    // See normal_scan: recovering from a poisoned lock beats silently never
    // applying hotkey changes again.
    config
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .bound_keys = entries;
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn autosave_marks_dirty_without_touching_the_disk() {
        let mut s = AppState::new();
        assert!(!s.unsaved_changes);
        autosave(&mut s);
        assert!(s.unsaved_changes, "the event loop flushes it later");
    }

    #[test]
    fn autosave_is_inert_when_the_config_failed_to_load() {
        let mut s = AppState::new();
        s.config_read_failed = true;
        autosave(&mut s);
        assert!(!s.unsaved_changes, "never overwrite a config we could not read");
    }

    #[test]
    fn flush_is_blocked_by_an_unreadable_config() {
        let mut s = AppState::new();
        s.config_read_failed = true;
        s.unsaved_changes = true;
        flush_pending_save(&mut s);
        assert!(s.unsaved_changes, "blocked config keeps the flag, writes nothing");
    }

    /// Both flush paths in one test: env vars are process-global, so two
    /// parallel tests redirecting HOME would race each other.
    #[test]
    #[cfg(unix)]
    fn flush_writes_on_success_and_reports_on_failure() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("tapmatic-flush-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp home");
        unsafe { std::env::set_var("HOME", &dir) };

        let mut s = AppState::new();
        s.macros.push(Macro::new("flushed".into()));
        s.unsaved_changes = true;
        flush_pending_save(&mut s);
        assert!(!s.unsaved_changes, "successful flush clears the dirty flag");
        assert!(dir.join(".tapmatic.json").exists(), "config written to HOME");

        // Err path: a read-only home makes the temp-file creation fail.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).expect("chmod");
        s.unsaved_changes = true;
        flush_pending_save(&mut s);
        assert!(
            s.status_message.as_ref().is_some_and(|(m, _, _)| m.contains("Save failed")),
            "a failed save must be reported, not silently dropped"
        );
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
    }

    //
    // Characterisation tests: these pin the behaviour of handle_key_step_editor
    // so the modal guards and the command match can be refactored safely.

    fn step_editor_state(step_count: usize) -> AppState {
        let mut s = AppState::new();
        let mut mac = Macro::new("m".into());
        for i in 0..step_count {
            mac.steps.push(MacroStep {
                action: KeyAction::KeyDown(i as u16 + 1),
                delay_ms: 10,
            });
        }
        s.macros.push(mac);
        s.view = View::StepEditor(0);
        s
    }

    fn press(state: &mut AppState, key: KeyCode) {
        handle_key_step_editor(state, key, KeyModifiers::NONE);
    }

    fn main_harness() -> (
        mpsc::Sender<ExecutorCommand>,
        mpsc::Receiver<ExecutorCommand>,
        Arc<Mutex<HotkeyConfig>>,
    ) {
        // On Windows the audio beeps are real rodio threads opening the
        // device; detached ones racing process exit crash the test binary
        // (STATUS_ACCESS_VIOLATION) on the headless CI runner.
        AUDIO_ENABLED.store(false, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        (tx, rx, Arc::new(Mutex::new(HotkeyConfig::new())))
    }

    fn named_macros(names: &[&str]) -> AppState {
        let mut s = AppState::new();
        for (i, name) in names.iter().enumerate() {
            let mut mac = Macro::new((*name).into());
            mac.hotkey_vk = 0x41 + i as i32;
            // Startable fixtures need at least one step: empty macros are refused.
            mac.steps.push(MacroStep { action: KeyAction::KeyDown(0x41), delay_ms: 0 });
            s.macros.push(mac);
        }
        s
    }

    #[test]
    fn delete_under_filter_removes_the_highlighted_macro() {
        let (tx, _rx, cfg) = main_harness();
        let mut s = named_macros(&["alpha", "beta", "gamma"]);
        s.search_query = "gamma".into();
        s.selected_macro = 0;
        s.confirm_delete = true;

        handle_key_main(&mut s, KeyCode::Enter, KeyModifiers::NONE, &tx, &cfg);

        let names: Vec<&str> = s.macros.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["alpha", "beta"], "must delete the filtered-to macro");
    }

    #[test]
    fn delete_prompt_names_the_filtered_macro() {
        let (tx, _rx, cfg) = main_harness();
        let mut s = named_macros(&["alpha", "beta", "gamma"]);
        s.search_query = "beta".into();
        s.selected_macro = 0;

        s.selected_button = BTN_DELETE;
        activate_button(&mut s, &tx, &cfg);

        assert!(s.confirm_delete);
        assert!(
            s.status_message.as_ref().is_some_and(|(m, _, _)| m.contains("beta")),
            "prompt must name the macro that will actually be deleted"
        );
    }

    #[test]
    fn f3_sort_snapshots_undo_and_marks_dirty() {
        let (tx, _rx, cfg) = main_harness();
        let mut s = named_macros(&["zeta", "alpha"]);

        handle_key_main(&mut s, KeyCode::F(3), KeyModifiers::NONE, &tx, &cfg);

        assert_eq!(s.sort_mode, state::SortMode::Name);
        assert_eq!(s.macros[0].name, "alpha");
        assert_eq!(s.undo_stack.len(), 1, "manual order must be undoable");
        assert!(s.unsaved_changes, "the sorted order must persist");
    }

    #[test]
    fn key_capture_modes_map_to_the_right_flags() {
        // The atomics are process-global and other tests store them in
        // parallel; assert the per-state flags and the pure mode mapping.
        let mut s = named_macros(&["m"]);

        begin_key_capture(&mut s, KeyCapture::StepInsert);
        assert!(s.inserting_key_step);
        end_key_capture(&mut s);
        assert!(!s.inserting_key_step);

        begin_key_capture(&mut s, KeyCapture::HotkeyBinding);
        assert!(s.awaiting_hotkey);
        end_key_capture(&mut s);
        assert!(!s.awaiting_hotkey);

        assert!(capture_accepts_mouse(KeyCapture::StepInsert));
        assert!(capture_accepts_mouse(KeyCapture::StepReplace));
        assert!(!capture_accepts_mouse(KeyCapture::HotkeyBinding));
        assert!(!capture_accepts_mouse(KeyCapture::RequireHeldBinding));
    }

    #[test]
    fn captured_mouse_button_inserts_a_click_step_pair() {
        let mut s = named_macros(&["m"]);
        s.view = View::StepEditor(0);
        s.selected_step = 0;
        s.inserting_key_step = true;

        capture_key_pair_insert(&mut s, 0x01);

        let steps = &s.macros[0].steps;
        assert!(matches!(steps[1].action, KeyAction::MouseDown(macro_def::MouseButton::Left)));
        assert!(matches!(steps[2].action, KeyAction::MouseUp(macro_def::MouseButton::Left)));
    }

    #[test]
    fn delete_clears_chain_references_to_the_victim() {
        let (tx, _rx, cfg) = main_harness();
        let mut s = named_macros(&["target", "chainer"]);
        s.macros[1].chain_macro = Some("target".into());
        s.selected_macro = 0;
        s.confirm_delete = true;

        handle_key_main(&mut s, KeyCode::Enter, KeyModifiers::NONE, &tx, &cfg);

        assert_eq!(s.macros.len(), 1);
        assert_eq!(s.macros[0].chain_macro, None, "dangling chain must be cleared");
    }

    #[test]
    fn config_rename_retargets_chain_references() {
        let mut s = named_macros(&["old-name", "chainer"]);
        s.macros[1].chain_macro = Some("old-name".into());
        s.config_field = state::ConfigField::Name;
        s.config_input_buf = "new-name".into();

        apply_config_field(&mut s, 0);

        assert_eq!(s.macros[0].name, "new-name");
        assert_eq!(s.macros[1].chain_macro.as_deref(), Some("new-name"));
    }

    #[test]
    fn chain_to_a_missing_target_reports_it() {
        let (tx, rx, cfg) = main_harness();
        let mut s = named_macros(&["m"]);

        handle_app_event(&mut s, AppEvent::ChainMacro("gone".into()), &tx, &cfg);

        assert!(rx.try_recv().is_err());
        assert!(
            s.status_message.as_ref().is_some_and(|(m, _, _)| m.contains("not found")),
            "a dangling chain must be reported, not silently dropped"
        );
    }

    #[test]
    fn quick_record_commit_invalidates_filter_and_prompt() {
        let mut s = named_macros(&["alpha", "beta"]);
        s.search_query = "alpha".into();
        s.searching = true;
        s.confirm_delete = true;
        let mut mac = Macro::new("recorded".into());
        mac.steps.push(MacroStep { action: KeyAction::KeyDown(0x41), delay_ms: 0 });

        commit_quick_recording(&mut s, mac);

        assert!(s.search_query.is_empty(), "filter must not survive the commit");
        assert!(!s.searching);
        assert!(!s.confirm_delete, "a pending delete prompt now names the wrong macro");
        assert_eq!(s.macros[s.selected_macro].name, "recorded");
    }

    #[test]
    fn config_commit_applies_the_load_clamp() {
        let mut s = named_macros(&["m"]);
        s.config_field = state::ConfigField::MouseJitter;
        s.config_input_buf = format!("{}", u32::MAX);

        apply_config_field(&mut s, 0);

        // Unclamped, this value overflows the i32 jitter math in the executor.
        assert!(s.macros[0].mouse_jitter <= 1000);
    }

    #[test]
    fn copy_refuses_past_the_macro_cap() {
        let (tx, _rx, cfg) = main_harness();
        let mut s = named_macros(&["m"]);
        // Simulate a full macro list without allocating 1000 real entries.
        let filler = s.macros[0].clone();
        s.macros.resize(macro_def::MAX_MACROS, filler);
        s.selected_macro = 0;
        s.selected_button = BTN_COPY;

        activate_button(&mut s, &tx, &cfg);

        assert_eq!(s.macros.len(), macro_def::MAX_MACROS, "no macro may be added past the cap");
    }

    #[test]
    fn empty_macro_is_refused_at_start() {
        let (tx, rx, _cfg) = main_harness();
        let mut s = named_macros(&["m"]);
        s.macros[0].steps.clear();
        let vk = s.macros[0].hotkey_vk;

        let mac = s.macros[0].clone();
        start_macro(&mut s, mac, &tx);

        assert!(rx.try_recv().is_err(), "no command may reach the executor");
        assert!(!s.is_macro_active(vk));
    }

    #[test]
    fn enter_config_resets_the_text_cursor() {
        let mut s = named_macros(&["ab"]);
        s.text_cursor = 10; // stale from a longer buffer (e.g. the search field)

        enter_config(&mut s, 0, ConfigEntry::ExistingMacro);

        assert_eq!(s.text_cursor, 2);
    }

    #[test]
    fn hotkey_toggle_starts_then_stops() {
        let (tx, rx, cfg) = main_harness();
        let mut s = named_macros(&["m"]);
        let vk = s.macros[0].hotkey_vk;

        handle_app_event(&mut s, AppEvent::HotkeyPressed(vk), &tx, &cfg);
        assert!(s.is_macro_active(vk));
        assert!(matches!(rx.try_recv(), Ok(ExecutorCommand::StartMacro(_))));

        handle_app_event(&mut s, AppEvent::HotkeyPressed(vk), &tx, &cfg);
        assert!(!s.is_macro_active(vk));
        assert!(matches!(rx.try_recv(), Ok(ExecutorCommand::StopMacro(v)) if v == vk));
    }

    #[test]
    fn exclusive_group_stops_the_running_peer() {
        let (tx, rx, cfg) = main_harness();
        let mut s = named_macros(&["a", "b"]);
        s.macros[0].exclusive_group = Some("g".into());
        s.macros[1].exclusive_group = Some("g".into());
        let (vk_a, vk_b) = (s.macros[0].hotkey_vk, s.macros[1].hotkey_vk);

        handle_app_event(&mut s, AppEvent::HotkeyPressed(vk_a), &tx, &cfg);
        handle_app_event(&mut s, AppEvent::HotkeyPressed(vk_b), &tx, &cfg);

        assert!(!s.is_macro_active(vk_a), "peer in the group must stop");
        assert!(s.is_macro_active(vk_b));
        let cmds: Vec<ExecutorCommand> = rx.try_iter().collect();
        assert!(
            cmds.iter().any(|c| matches!(c, ExecutorCommand::StopMacro(v) if *v == vk_a)),
            "expected a StopMacro for the peer, got {cmds:?}"
        );
    }

    #[test]
    fn chain_start_respects_the_kill_switch() {
        let (tx, rx, cfg) = main_harness();
        let mut s = named_macros(&["target"]);
        s.macros_enabled = false;

        handle_app_event(&mut s, AppEvent::ChainMacro("target".into()), &tx, &cfg);

        assert!(rx.try_recv().is_err(), "disabled macros must not chain-start");
    }

    #[test]
    fn chain_start_launches_an_allowed_macro() {
        let (tx, rx, cfg) = main_harness();
        let mut s = named_macros(&["target"]);

        handle_app_event(&mut s, AppEvent::ChainMacro("target".into()), &tx, &cfg);

        assert!(matches!(rx.try_recv(), Ok(ExecutorCommand::StartMacro(m)) if m.name == "target"));
    }

    #[test]
    fn record_key_outside_recording_view_is_ignored() {
        let (tx, _rx, cfg) = main_harness();
        let mut s = AppState::new();
        s.view = View::Main;

        handle_app_event(
            &mut s,
            AppEvent::RecordKey(KeyAction::KeyDown(30), std::time::Instant::now()),
            &tx,
            &cfg,
        );

        assert!(
            s.recording_steps.is_empty(),
            "keys typed outside recording mode must not be captured"
        );
    }

    #[test]
    fn step_editor_ignores_keys_outside_its_view() {
        let mut s = step_editor_state(3);
        s.view = View::Main;
        press(&mut s, KeyCode::Down);
        assert_eq!(s.selected_step, 0, "no effect outside StepEditor");
    }

    #[test]
    fn step_editor_navigation_wraps() {
        let mut s = step_editor_state(3);
        press(&mut s, KeyCode::Down);
        assert_eq!(s.selected_step, 1);
        press(&mut s, KeyCode::Down);
        press(&mut s, KeyCode::Down);
        assert_eq!(s.selected_step, 0, "wraps past the last step");
        press(&mut s, KeyCode::Up);
        assert_eq!(s.selected_step, 2, "wraps backwards");
    }

    #[test]
    fn step_editor_esc_returns_to_config() {
        let mut s = step_editor_state(1);
        press(&mut s, KeyCode::Esc);
        assert_eq!(s.view, View::Config(0));
    }

    #[test]
    fn key_capture_mode_only_accepts_esc() {
        let mut s = step_editor_state(2);
        s.inserting_key_step = true;

        press(&mut s, KeyCode::Down);
        assert!(s.inserting_key_step, "other keys are swallowed");
        assert_eq!(s.selected_step, 0, "navigation does not leak through");

        press(&mut s, KeyCode::Esc);
        assert!(!s.inserting_key_step, "Esc cancels capture");
    }

    #[test]
    fn insert_text_commits_after_the_selection() {
        let mut s = step_editor_state(3);
        s.selected_step = 0;
        s.inserting_text = true;
        s.insert_text_buf = "hi".into();

        press(&mut s, KeyCode::Enter);

        assert!(!s.inserting_text);
        assert!(s.insert_text_buf.is_empty(), "buffer cleared on commit");
        assert_eq!(s.macros[0].steps.len(), 4);
        assert_eq!(s.selected_step, 1, "selection follows the inserted step");
        match &s.macros[0].steps[1].action {
            KeyAction::TypeText(t) => assert_eq!(t, "hi"),
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn insert_text_esc_discards_the_buffer() {
        let mut s = step_editor_state(2);
        s.inserting_text = true;
        s.insert_text_buf = "hi".into();

        press(&mut s, KeyCode::Esc);

        assert!(!s.inserting_text);
        assert!(s.insert_text_buf.is_empty());
        assert_eq!(s.macros[0].steps.len(), 2, "no step added");
    }

    #[test]
    fn insert_text_empty_buffer_adds_nothing() {
        let mut s = step_editor_state(2);
        s.inserting_text = true;

        press(&mut s, KeyCode::Enter);

        assert_eq!(s.macros[0].steps.len(), 2);
        assert!(!s.inserting_text);
    }

    #[test]
    fn insert_text_typing_accumulates_into_the_buffer() {
        let mut s = step_editor_state(1);
        s.inserting_text = true;

        press(&mut s, KeyCode::Char('a'));
        press(&mut s, KeyCode::Char('b'));

        assert_eq!(s.insert_text_buf, "ab", "the clone-mutate-write-back path");
    }

    #[test]
    fn step_delay_edit_commits_a_parsed_value() {
        let mut s = step_editor_state(2);
        s.selected_step = 1;
        s.editing_step_delay = true;
        s.step_delay_buf = "250".into();

        press(&mut s, KeyCode::Enter);

        assert_eq!(s.macros[0].steps[1].delay_ms, 250);
        assert!(!s.editing_step_delay);
        assert!(s.step_delay_buf.is_empty());
    }

    #[test]
    fn step_delay_edit_ignores_an_unparseable_value() {
        let mut s = step_editor_state(1);
        s.editing_step_delay = true;
        s.step_delay_buf = "abc".into();

        press(&mut s, KeyCode::Enter);

        assert_eq!(s.macros[0].steps[0].delay_ms, 10, "original delay kept");
        assert!(!s.editing_step_delay, "edit mode still exits");
    }

    #[test]
    fn step_delay_edit_esc_leaves_the_step_alone() {
        let mut s = step_editor_state(1);
        s.editing_step_delay = true;
        s.step_delay_buf = "999".into();

        press(&mut s, KeyCode::Esc);

        assert_eq!(s.macros[0].steps[0].delay_ms, 10);
        assert!(s.step_delay_buf.is_empty());
    }

    #[test]
    fn step_delay_edit_rejects_letters() {
        let mut s = step_editor_state(1);
        s.editing_step_delay = true;

        press(&mut s, KeyCode::Char('7'));
        press(&mut s, KeyCode::Char('x'));

        assert_eq!(s.step_delay_buf, "7", "numeric field takes digits only");
    }

    #[test]
    fn scroll_clicks_edit_updates_the_selected_scroll_step() {
        let mut s = step_editor_state(0);
        s.macros[0].steps.push(MacroStep {
            action: KeyAction::MouseScroll(ScrollDirection::Up, 1),
            delay_ms: 0,
        });
        s.editing_scroll_clicks = true;
        s.scroll_clicks_buf = "5".into();

        press(&mut s, KeyCode::Enter);

        match s.macros[0].steps[0].action {
            KeyAction::MouseScroll(_, clicks) => assert_eq!(clicks, 5),
            ref other => panic!("unexpected action: {other:?}"),
        }
        assert!(!s.editing_scroll_clicks);
    }

    #[test]
    fn scroll_clicks_edit_rejects_zero() {
        let mut s = step_editor_state(0);
        s.macros[0].steps.push(MacroStep {
            action: KeyAction::MouseScroll(ScrollDirection::Up, 3),
            delay_ms: 0,
        });
        s.editing_scroll_clicks = true;
        s.scroll_clicks_buf = "0".into();

        press(&mut s, KeyCode::Enter);

        match s.macros[0].steps[0].action {
            KeyAction::MouseScroll(_, clicks) => assert_eq!(clicks, 3, "zero is ignored"),
            ref other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn modal_guards_take_priority_over_commands() {
        // 'd' deletes a step in command mode, but must be swallowed as text
        // while a modal input is open.
        let mut s = step_editor_state(3);
        s.inserting_text = true;

        press(&mut s, KeyCode::Char('d'));

        assert_eq!(s.macros[0].steps.len(), 3, "no step deleted");
        assert_eq!(s.insert_text_buf, "d", "captured as text instead");
    }

    #[test]
    fn wrap_nav_down() {
        let mut cur = 0;
        wrap_nav(&mut cur, 3, false);
        assert_eq!(cur, 1);
        wrap_nav(&mut cur, 3, false);
        assert_eq!(cur, 2);
        wrap_nav(&mut cur, 3, false);
        assert_eq!(cur, 0); // wraps
    }

    #[test]
    fn wrap_nav_up() {
        let mut cur = 0;
        wrap_nav(&mut cur, 3, true);
        assert_eq!(cur, 2); // wraps
        wrap_nav(&mut cur, 3, true);
        assert_eq!(cur, 1);
    }

    #[test]
    fn wrap_nav_empty() {
        let mut cur = 0;
        wrap_nav(&mut cur, 0, true);
        assert_eq!(cur, 0);
        wrap_nav(&mut cur, 0, false);
        assert_eq!(cur, 0);
    }

    #[test]
    fn wrap_nav_single_element() {
        let mut cur = 0;
        wrap_nav(&mut cur, 1, false);
        assert_eq!(cur, 0);
        wrap_nav(&mut cur, 1, true);
        assert_eq!(cur, 0);
    }
}

