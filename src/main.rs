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
use state::{AppEvent, AppState, ConfigField, ExecutorCommand, View};

pub static RUNNING: AtomicBool = AtomicBool::new(true);
pub static RECORDING: AtomicBool = AtomicBool::new(false);
pub static AUDIO_ENABLED: AtomicBool = AtomicBool::new(true);
pub static MACROS_ENABLED: AtomicBool = AtomicBool::new(true);
pub static AWAITING_HOTKEY: AtomicBool = AtomicBool::new(false);
pub static SPEED_MULTIPLIER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0x3FF0000000000000); // 1.0f64 as bits
pub static QUICK_RECORD: AtomicBool = AtomicBool::new(false);
pub static RECORD_MOUSE_MOVES: AtomicBool = AtomicBool::new(false);
pub static MOUSE_MOVE_THRESHOLD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(5);

pub const MAIN_BUTTONS: &[&str] = &[
    "Record", "Edit", "Copy", "Delete", "Save", "Toggle", "Mute", "Quit",
];

fn main() {
    // Panic handler: restore terminal before printing panic info
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        default_hook(info);
    }));

    input::begin_high_res_timer();

    let config = persistence::load();

    let (event_tx, event_rx) = mpsc::channel::<AppEvent>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<ExecutorCommand>();

    let hotkey_config = Arc::new(Mutex::new(HotkeyConfig::new()));

    let mut state = AppState::new();
    state.macros = config.macros;
    state.audio_enabled = config.audio_enabled;
    state.macros_enabled = config.macros_enabled;
    state.speed_multiplier = config.speed_multiplier;
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
    std::thread::Builder::new()
        .name("executor".into())
        .spawn(move || executor::executor_loop(cmd_rx, exec_tx))
        .expect("failed to spawn executor thread");

    // Set terminal window title
    crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle("tapmatic")).ok();

    let mut terminal = ratatui::init();
    let result = run_app(&mut terminal, &mut state, &event_rx, &cmd_tx, &hotkey_config);
    ratatui::restore();

    RUNNING.store(false, Ordering::SeqCst);
    let _ = cmd_tx.send(ExecutorCommand::StopAll);
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

    loop {
        state.foreground_process = input::get_foreground_process_name();

        // Update system stats every ~1 second (60 frames)
        frame_count += 1;
        if frame_count % 60 == 0 {
            let new_snapshot = input::get_cpu_usage_snapshot();
            state.cpu_usage = input::calc_cpu_percent(cpu_snapshot, new_snapshot);
            cpu_snapshot = new_snapshot;
            let (used, total) = input::get_ram_usage();
            state.ram_used_mb = used;
            state.ram_total_mb = total;
            state.uptime_secs = app_start.elapsed().as_secs();
        }

        // Update console title (visible in taskbar when minimized)
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
        crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle(&title)).ok();

        terminal.draw(|f| ui::draw(f, state))?;

        if state.should_quit {
            return Ok(());
        }

        state.clear_stale_status();

        while let Ok(ev) = event_rx.try_recv() {
            handle_app_event(state, ev, cmd_tx, hotkey_config);
        }

        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(state, key.code, key.modifiers, cmd_tx, hotkey_config);
                }
            }
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
            let _ = cmd_tx.send(ExecutorCommand::StopMacro(vk));
            state.set_macro_active(vk, false);
        }
    }
}

/// Start a macro: enforce exclusive group, send command, track state.
fn start_macro(
    state: &mut AppState,
    mac: Macro,
    cmd_tx: &mpsc::Sender<ExecutorCommand>,
) {
    let vk = mac.hotkey_vk;
    enforce_exclusive_group(state, &mac, cmd_tx);
    let _ = cmd_tx.send(ExecutorCommand::StartMacro(mac));
    state.set_macro_active(vk, true);
    *state.macro_activations.entry(vk).or_insert(0) += 1;
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
                            let _ = cmd_tx.send(ExecutorCommand::StopMacro(vk));
                            state.set_macro_active(vk, false);
                            audio::play_deactivate();
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
            if let Some((_, mac)) = state.macro_by_hotkey(vk) {
                if mac.repetition == RepetitionMode::HoldToRepeat && state.is_macro_active(vk) {
                    let _ = cmd_tx.send(ExecutorCommand::StopMacro(vk));
                    state.set_macro_active(vk, false);
                    audio::play_deactivate();
                }
            }
        }
        AppEvent::HotkeyCaptured(vk) => {
            if state.awaiting_hotkey || state.awaiting_require_held {
                if let View::Config(idx) = state.view {
                    // Check for duplicate hotkey before mutable borrow
                    let dupe_name = if state.awaiting_hotkey {
                        state.macros.iter().enumerate()
                            .find(|(i, m)| *i != idx && m.hotkey_vk == vk)
                            .map(|(_, m)| m.name.clone())
                    } else { None };

                    if let Some(mac) = state.macros.get_mut(idx) {
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
                        state.awaiting_hotkey = false;
                        state.awaiting_require_held = false;
                        AWAITING_HOTKEY.store(false, Ordering::SeqCst);
                        state.set_status(label);
                    }
                }
            }
            if state.editing_step_action {
                if let View::StepEditor(idx) = state.view {
                    let sc = input::vk_to_scancode(vk);
                    if let Some(step) = state.macros[idx].steps.get_mut(state.selected_step) {
                        let is_down = matches!(
                            step.action,
                            KeyAction::KeyDown(_) | KeyAction::MouseDown(_)
                        );
                        if let Some(new_action) = KeyAction::from_vk(vk, sc, is_down) {
                            let name = new_action.display_name();
                            step.action = new_action;
                            state.set_status(format!("Action changed to {}", name));
                        }
                    }
                    state.editing_step_action = false;
                    AWAITING_HOTKEY.store(false, Ordering::SeqCst);
                }
            }
            if state.inserting_key_step {
                if let View::StepEditor(idx) = state.view {
                    let sc = input::vk_to_scancode(vk);
                    if let (Some(action_down), Some(action_up)) = (
                        KeyAction::from_vk(vk, sc, true),
                        KeyAction::from_vk(vk, sc, false),
                    ) {
                        let pos = (state.selected_step + 1).min(state.macros[idx].steps.len());
                        let name = action_down.display_name();
                        state.macros[idx].steps.insert(pos, MacroStep { action: action_down, delay_ms: 0 });
                        state.macros[idx].steps.insert(pos + 1, MacroStep { action: action_up, delay_ms: 50 });
                        state.selected_step = pos;
                        state.set_status(format!("Inserted {} (down+up)", name));
                    }
                    state.inserting_key_step = false;
                    AWAITING_HOTKEY.store(false, Ordering::SeqCst);
                }
            }
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
            if let Some((_, mac)) = state
                .macros
                .iter()
                .enumerate()
                .find(|(_, m)| m.name == name)
            {
                let mac = mac.clone();
                let vk = mac.hotkey_vk;
                let _ = cmd_tx.send(ExecutorCommand::StartMacro(mac));
                if vk != 0 {
                    state.set_macro_active(vk, true);
                }
                state.set_status(format!("Chained: {}", name));
            }
        }
        AppEvent::QuickRecordToggle => {
            if state.view == View::Recording {
                // Stop quick-recording
                let mac = recorder::stop_recording(state);
                audio::play_record_stop();
                if mac.steps.is_empty() {
                    state.view = View::Main;
                    state.set_status("Quick record cancelled — no keys captured");
                } else {
                    let count = mac.steps.len();
                    let idx = state.macros.len();
                    state.macros.push(mac);
                    state.selected_macro = idx;
                    state.view = View::Main;
                    state.set_status(format!("Quick recorded {} steps — edit with Space", count));
                    autosave(state);
                }
                QUICK_RECORD.store(false, Ordering::SeqCst);
            } else if state.view == View::Main {
                // Start quick-recording
                recorder::start_recording(state);
                QUICK_RECORD.store(true, Ordering::SeqCst);
                state.set_status_persistent("Quick recording... Alt+Z to stop");
            }
        }
        AppEvent::RecordStop => {
            if state.view == View::Recording {
                let mac = recorder::stop_recording(state);
                audio::play_record_stop();
                if mac.steps.is_empty() {
                    state.view = View::Main;
                    state.set_status("Recording cancelled — no keys captured");
                } else if QUICK_RECORD.load(Ordering::SeqCst) {
                    // Quick record: save directly to main
                    let count = mac.steps.len();
                    let idx = state.macros.len();
                    state.macros.push(mac);
                    state.selected_macro = idx;
                    state.view = View::Main;
                    state.set_status(format!("Quick recorded {} steps — edit with Space", count));
                    autosave(state);
                } else {
                    // Normal record: go to config
                    let count = mac.steps.len();
                    let idx = state.macros.len();
                    state.macros.push(mac);
                    state.config_macro_backup = None;
                    state.config_field = ConfigField::Name;
                    state.config_input_buf = state.macros[idx].name.clone();
                    state.text_cursor = state.config_input_buf.chars().count();
                    state.awaiting_hotkey = false;
                    state.selected_macro = idx;
                    state.view = View::Config(idx);
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
                state.mouse_move_threshold = match state.mouse_move_threshold {
                    1 => 3, 3 => 5, 5 => 10, 10 => 20, _ => 1,
                };
                MOUSE_MOVE_THRESHOLD.store(state.mouse_move_threshold, Ordering::SeqCst);
                state.set_status(format!("Mouse threshold: {}px", state.mouse_move_threshold));
            }
        }
    }
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
        View::Recording => handle_key_recording(state, key, hotkey_config),
        View::Config(_) => handle_key_config(state, key, mods, hotkey_config),
        View::StepEditor(_) => handle_key_step_editor(state, key, mods),
        View::ProcessPicker(_) => handle_key_process_picker(state, key),
        View::ChainPicker(_) => handle_key_chain_picker(state, key),
        View::Help => handle_key_help(state, key),
    }
}

// ── Main View ──
// Up/Down = navigate macro list, Left/Right = navigate buttons, Enter = activate button

fn activate_button(
    state: &mut AppState,
    cmd_tx: &mpsc::Sender<ExecutorCommand>,
    hotkey_config: &Arc<Mutex<HotkeyConfig>>,
) {
    match state.selected_button {
        0 => {
            // Record
            recorder::start_recording(state);
            update_hotkey_config(state, hotkey_config);
        }
        1 => {
            // Edit
            if let Some(real_idx) = resolve_selected_macro(state) {
                enter_config(state, real_idx);
            } else {
                state.set_status("No macros to edit. Record one first");
            }
        }
        2 => {
            // Copy
            if let Some(real_idx) = resolve_selected_macro(state) {
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
        3 => {
            // Delete
            if !state.macros.is_empty() {
                state.confirm_delete = true;
                let name = &state.macros[state.selected_macro].name;
                state.set_status_persistent(format!("Delete '{}'? Enter=yes  Esc=no", name));
            } else {
                state.set_status("No macros to delete");
            }
        }
        4 => {
            // Save to file
            autosave(state);
            state.set_status(format!("Saved {} macros to .tapmatic.json", state.macros.len()));
        }
        5 => {
            // Toggle
            state.macros_enabled = !state.macros_enabled;
            MACROS_ENABLED.store(state.macros_enabled, Ordering::SeqCst);
            if !state.macros_enabled {
                for vk in std::mem::take(&mut state.active_macros) {
                    let _ = cmd_tx.send(ExecutorCommand::StopMacro(vk));
                }
            }
            if state.macros_enabled {
                state.set_status("Macros enabled — hotkeys will trigger macros");
            } else {
                state.set_status("Macros disabled — all hotkeys paused");
            }
            autosave(state);
        }
        6 => {
            // Mute
            state.audio_enabled = !state.audio_enabled;
            AUDIO_ENABLED.store(state.audio_enabled, Ordering::SeqCst);
            if state.audio_enabled {
                state.set_status("Audio on — sounds will play");
            } else {
                state.set_status("Audio muted");
            }
            autosave(state);
        }
        7 => {
            // Quit
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

fn enter_config(state: &mut AppState, idx: usize) {
    state.clear_status();
    state.push_undo();
    state.config_macro_backup = Some(state.macros[idx].clone());
    state.config_tab = 0;
    state.config_field = ConfigField::Name;
    state.config_input_buf = state.macros[idx].name.clone();
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
    // Renaming mode
    if state.renaming {
        match key {
            KeyCode::Enter => {
                if let Some(real_idx) = resolve_selected_macro(state) {
                    if !state.rename_buf.is_empty() {
                        state.macros[real_idx].name = state.rename_buf.clone();
                        state.set_status(format!("Renamed to '{}'", state.rename_buf));
                        autosave(state);
                    }
                }
                state.renaming = false;
                state.rename_buf.clear();
            }
            KeyCode::Esc => {
                state.renaming = false;
                state.rename_buf.clear();
                state.set_status("Rename cancelled");
            }
            _ => {
                let mut buf = state.rename_buf.clone();
                handle_text_motion(state, &mut buf, key, mods, true, true);
                state.rename_buf = buf;
            }
        }
        return;
    }

    // Search mode
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
                if state.selected_macro < state.macros.len() {
                    state.push_undo();
                    let name = state.macros[state.selected_macro].name.clone();
                    let vk = state.macros[state.selected_macro].hotkey_vk;
                    if state.is_macro_active(vk) {
                        let _ = cmd_tx.send(ExecutorCommand::StopMacro(vk));
                        state.set_macro_active(vk, false);
                    }
                    state.macros.remove(state.selected_macro);
                    state.clamp_selection();
                    state.set_status(format!("Deleted '{}'", name));
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
        // List navigation
        KeyCode::Up => state.move_selection(true),
        KeyCode::Down => state.move_selection(false),

        // Button bar navigation
        KeyCode::Left => wrap_nav(&mut state.selected_button, MAIN_BUTTONS.len(), true),
        KeyCode::Right => wrap_nav(&mut state.selected_button, MAIN_BUTTONS.len(), false),

        // Enter = activate selected button
        KeyCode::Enter => {
            activate_button(state, cmd_tx, hotkey_config);
        }

        KeyCode::Char(' ') => {
            if let Some(real_idx) = resolve_selected_macro(state) {
                enter_config(state, real_idx);
            } else {
                // No macros? Start recording
                recorder::start_recording(state);
                update_hotkey_config(state, hotkey_config);
            }
        }

        KeyCode::Char('z') | KeyCode::Char('Z') => {
            // Undo
            if state.pop_undo() {
                update_hotkey_config(state, hotkey_config);
                autosave(state);
                state.set_status("Undone");
            } else {
                state.set_status("Nothing to undo");
            }
        }
        KeyCode::F(2) => {
            // Rename selected macro inline
            if let Some(real_idx) = resolve_selected_macro(state) {
                state.renaming = true;
                state.rename_buf = state.macros[real_idx].name.clone();
                state.text_cursor = state.rename_buf.chars().count();
                state.set_status_persistent("Type new name, Enter to confirm...");
            }
        }
        KeyCode::Char('k') | KeyCode::Char('K') => {
            // Move macro up
            if let Some(real_idx) = resolve_selected_macro(state) {
                if real_idx > 0 {
                    state.push_undo();
                    state.macros.swap(real_idx, real_idx - 1);
                    state.selected_macro = state.selected_macro.saturating_sub(1);
                    autosave(state);
                }
            }
        }
        KeyCode::Char('j') | KeyCode::Char('J') => {
            // Move macro down
            if let Some(real_idx) = resolve_selected_macro(state) {
                if real_idx + 1 < state.macros.len() {
                    state.push_undo();
                    state.macros.swap(real_idx, real_idx + 1);
                    if state.selected_macro + 1 < state.filtered_macro_indices().len() {
                        state.selected_macro += 1;
                    }
                    autosave(state);
                }
            }
        }
        KeyCode::Char('/') => {
            // Search
            state.searching = true;
            state.search_query.clear();
            state.text_cursor = 0;
            state.set_status_persistent("Type to search...");
        }
        KeyCode::F(3) => {
            // Cycle sort mode
            state.sort_mode = match state.sort_mode {
                state::SortMode::None => state::SortMode::Name,
                state::SortMode::Name => state::SortMode::Hotkey,
                state::SortMode::Hotkey => state::SortMode::None,
            };
            state.apply_sort();
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
                KeyCode::Char('+') | KeyCode::Char('=') => (state.speed_multiplier + 0.25).min(4.0),
                KeyCode::Char('-') => (state.speed_multiplier - 0.25).max(0.25),
                _ => 1.0,
            };
            SPEED_MULTIPLIER.store(state.speed_multiplier.to_bits(), Ordering::SeqCst);
            let label = if (state.speed_multiplier - 1.0).abs() < 0.01 {
                "Speed: x1.00 (reset)".to_string()
            } else {
                format!("Speed: x{:.2}", 1.0 / state.speed_multiplier)
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

// ── Recording View ──

fn handle_key_recording(
    _state: &mut AppState,
    _key: KeyCode,
    _hotkey_config: &Arc<Mutex<HotkeyConfig>>,
) {
    // All recording controls (Esc, F1, F2, F3, Alt+Z) are handled via AppEvent
    // from the hotkey thread so they work without TUI focus.
}

// ── Config View ──

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

fn autosave(state: &AppState) {
    let config = persistence::AppConfig {
        audio_enabled: state.audio_enabled,
        macros_enabled: state.macros_enabled,
        speed_multiplier: state.speed_multiplier,
        macros: state.macros.clone(),
    };
    let _ = persistence::save(&config);
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
    // by the hotkey thread (all keys + mouse buttons via GetAsyncKeyState).
    // Crossterm might also fire, so just ignore everything except Esc here.
    if state.awaiting_hotkey || state.awaiting_require_held {
        if key == KeyCode::Esc {
            state.awaiting_hotkey = false;
            state.awaiting_require_held = false;
            AWAITING_HOTKEY.store(false, Ordering::SeqCst);
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

        // Navigate fields / tabs
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

        // Cycle values with Left/Right
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
                        (StopCondition::None, true) => StopCondition::AfterReps(10),
                        (StopCondition::None, false) => StopCondition::AfterSecs(30),
                        (StopCondition::AfterReps(_), true) => StopCondition::AfterSecs(30),
                        (StopCondition::AfterReps(_), false) => StopCondition::None,
                        (StopCondition::AfterSecs(_), true) => StopCondition::None,
                        (StopCondition::AfterSecs(_), false) => StopCondition::AfterReps(10),
                    };
                }
                ConfigField::SendMode => {
                    state.macros[idx].send_mode = match state.macros[idx].send_mode {
                        SendMode::Global => SendMode::Window,
                        SendMode::Window => SendMode::Global,
                    };
                }
                _ => {
                    // Text/numeric fields: cursor movement (including Ctrl+Arrow word jump)
                    if state.config_field.is_text_field() || state.config_field.is_numeric_field() {
                        let ctrl = mods.contains(KeyModifiers::CONTROL);
                        if ctrl && !forward { state.buf_cursor_word_left(&state.config_input_buf.clone()); }
                        else if ctrl && forward { state.buf_cursor_word_right(&state.config_input_buf.clone()); }
                        else if forward { state.buf_cursor_right(state.config_input_buf.chars().count()); }
                        else { state.buf_cursor_left(); }
                    }
                }
            }
        }

        // Enter = field-specific action
        KeyCode::Enter => {
            apply_config_field(state, idx);
            match state.config_field {
                ConfigField::Hotkey => {
                    state.awaiting_hotkey = true;
                    AWAITING_HOTKEY.store(true, Ordering::SeqCst);
                    state.set_status_persistent("Press any key or mouse button...");
                }
                ConfigField::RequireHeld => {
                    state.awaiting_require_held = true;
                    AWAITING_HOTKEY.store(true, Ordering::SeqCst);
                    state.set_status_persistent("Press the key that must be held...");
                }
                ConfigField::ChainMacro => {
                    // Open chain macro picker
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
                    // Open process picker
                    state.process_list = input::get_window_list();
                    state.selected_process = 0;
                    state.view = View::ProcessPicker(idx);
                }
                _ => {
                    // Move to next field
                    state.config_field = state.config_field.next_in_tab(state.config_tab);
                    prefill_config_buf(state, idx);
                }
            }
        }

        // F5 = Save & close
        KeyCode::F(5) => {
            save_config(state, idx, hotkey_config);
        }

        // F1 = Step editor
        KeyCode::F(1) => {
            apply_config_field(state, idx);
            state.selected_step = 0;
            state.editing_step_delay = false;
            state.step_delay_buf.clear();
            state.view = View::StepEditor(idx);
        }

        // Text input for editable fields (Char, Backspace, Delete, Home, End, Ctrl+Backspace)
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

fn apply_config_field(state: &mut AppState, idx: usize) {
    match state.config_field {
        ConfigField::Name => {
            if !state.config_input_buf.is_empty() {
                state.macros[idx].name = state.config_input_buf.clone();
            }
        }
        ConfigField::FixedInterval => {
            if let Ok(ms) = state.config_input_buf.parse::<u64>() {
                if ms > 0 {
                    state.macros[idx].fixed_interval_ms = ms;
                }
            }
        }
        ConfigField::RandomDelayMin => {
            if let Ok(ms) = state.config_input_buf.parse::<u64>() {
                let max = state.macros[idx].random_delay.map_or(ms, |(_, mx)| mx);
                state.macros[idx].random_delay = Some((ms, max.max(ms)));
            } else if state.config_input_buf.is_empty() {
                state.macros[idx].random_delay = None;
            }
        }
        ConfigField::RandomDelayMax => {
            if let Ok(ms) = state.config_input_buf.parse::<u64>() {
                let min = state.macros[idx].random_delay.map_or(0, |(mn, _)| mn);
                state.macros[idx].random_delay = Some((min, ms.max(min)));
            }
        }
        ConfigField::StopValue => {
            if let Ok(val) = state.config_input_buf.parse::<u32>() {
                if val > 0 {
                    state.macros[idx].stop_condition = state.macros[idx].stop_condition.with_value(val);
                }
            }
        }
        ConfigField::CycleDelay => {
            if let Ok(ms) = state.config_input_buf.parse::<u64>() {
                state.macros[idx].cycle_delay_ms = ms;
            }
        }
        ConfigField::StartDelay => {
            if let Ok(ms) = state.config_input_buf.parse::<u64>() {
                state.macros[idx].start_delay_ms = ms;
            }
        }
        ConfigField::MouseJitter => {
            if let Ok(px) = state.config_input_buf.parse::<u32>() {
                state.macros[idx].mouse_jitter = px;
            }
        }
        ConfigField::HumanizeMs => {
            if let Ok(ms) = state.config_input_buf.parse::<u64>() {
                state.macros[idx].humanize_ms = ms;
            }
        }
        ConfigField::ExclusiveGroup | ConfigField::ChainMacro | ConfigField::BoundProcess => {
            let val = state.config_input_buf.trim().to_string();
            let opt = if val.is_empty() { None } else {
                Some(if state.config_field == ConfigField::BoundProcess { val.to_lowercase() } else { val })
            };
            match state.config_field {
                ConfigField::ExclusiveGroup => state.macros[idx].exclusive_group = opt,
                ConfigField::ChainMacro => state.macros[idx].chain_macro = opt,
                ConfigField::BoundProcess => state.macros[idx].bound_process = opt,
                _ => {}
            }
        }
        _ => {}
    }
}

fn prefill_config_buf(state: &mut AppState, idx: usize) {
    match state.config_field {
        ConfigField::Name => {
            state.config_input_buf = state.macros[idx].name.clone();
        }
        ConfigField::FixedInterval => {
            state.config_input_buf = state.macros[idx].fixed_interval_ms.to_string();
        }
        ConfigField::RandomDelayMin => {
            state.config_input_buf = state.macros[idx]
                .random_delay
                .map_or(String::new(), |(min, _)| min.to_string());
        }
        ConfigField::RandomDelayMax => {
            state.config_input_buf = state.macros[idx]
                .random_delay
                .map_or(String::new(), |(_, max)| max.to_string());
        }
        ConfigField::StopValue => {
            state.config_input_buf = state.macros[idx].stop_condition
                .value()
                .map_or(String::new(), |n| n.to_string());
        }
        ConfigField::CycleDelay => {
            state.config_input_buf = state.macros[idx].cycle_delay_ms.to_string();
        }
        ConfigField::StartDelay => {
            state.config_input_buf = state.macros[idx].start_delay_ms.to_string();
        }
        ConfigField::RequireHeld => {
            state.config_input_buf.clear();
        }
        ConfigField::MouseJitter => {
            state.config_input_buf = state.macros[idx].mouse_jitter.to_string();
        }
        ConfigField::HumanizeMs => {
            state.config_input_buf = state.macros[idx].humanize_ms.to_string();
        }
        ConfigField::ExclusiveGroup | ConfigField::ChainMacro | ConfigField::BoundProcess => {
            let mac = &state.macros[idx];
            state.config_input_buf = match state.config_field {
                ConfigField::ExclusiveGroup => mac.exclusive_group.clone(),
                ConfigField::ChainMacro => mac.chain_macro.clone(),
                ConfigField::BoundProcess => mac.bound_process.clone(),
                _ => None,
            }.unwrap_or_default();
        }
        _ => {
            state.config_input_buf.clear();
        }
    }
    state.text_cursor = state.config_input_buf.chars().count();
}

// ── Step Editor ──

fn handle_key_step_editor(state: &mut AppState, key: KeyCode, mods: KeyModifiers) {
    let idx = match state.view {
        View::StepEditor(i) => i,
        _ => return,
    };

    let step_count = state.macros[idx].steps.len();

    // Awaiting key capture (insert or replace) — only Esc cancels
    if state.inserting_key_step || state.editing_step_action {
        if key == KeyCode::Esc {
            let msg = if state.inserting_key_step { "Insert key cancelled" } else { "Action edit cancelled" };
            state.inserting_key_step = false;
            state.editing_step_action = false;
            AWAITING_HOTKEY.store(false, Ordering::SeqCst);
            state.set_status(msg);
        }
        return;
    }

    // Inserting a TypeText step
    if state.inserting_text {
        match key {
            KeyCode::Enter => {
                if !state.insert_text_buf.is_empty() {
                    let text = state.insert_text_buf.clone();
                    let step = MacroStep {
                        action: KeyAction::TypeText(text.clone()),
                        delay_ms: 0,
                    };
                    // Insert after current selection
                    let insert_pos = (state.selected_step + 1).min(state.macros[idx].steps.len());
                    state.macros[idx].steps.insert(insert_pos, step);
                    state.selected_step = insert_pos;
                    state.set_status(format!("Inserted text: \"{}\"", text));
                }
                state.inserting_text = false;
                state.insert_text_buf.clear();
            }
            KeyCode::Esc => {
                state.inserting_text = false;
                state.insert_text_buf.clear();
                state.set_status("Insert cancelled");
            }
            _ => {
                let mut buf = state.insert_text_buf.clone();
                handle_text_motion(state, &mut buf, key, mods, true, true);
                state.insert_text_buf = buf;
            }
        }
        return;
    }

    if state.editing_scroll_clicks {
        match key {
            KeyCode::Enter => {
                if let Ok(clicks) = state.scroll_clicks_buf.parse::<u32>() {
                    if clicks > 0 {
                        if let Some(step) = state.macros[idx].steps.get_mut(state.selected_step) {
                            if let KeyAction::MouseScroll(dir, ref mut c) = step.action {
                                *c = clicks;
                                state.set_status(format!("Scroll {} x{}", if dir == ScrollDirection::Up { "up" } else { "down" }, clicks));
                            }
                        }
                    }
                }
                state.editing_scroll_clicks = false;
                state.scroll_clicks_buf.clear();
            }
            KeyCode::Esc => {
                state.editing_scroll_clicks = false;
                state.scroll_clicks_buf.clear();
            }
            _ => {
                let mut buf = state.scroll_clicks_buf.clone();
                handle_text_motion(state, &mut buf, key, mods, false, true);
                state.scroll_clicks_buf = buf;
            }
        }
        return;
    }

    if state.editing_step_delay {
        match key {
            KeyCode::Enter => {
                if let Ok(ms) = state.step_delay_buf.parse::<u64>() {
                    if let Some(step) = state.macros[idx].steps.get_mut(state.selected_step) {
                        step.delay_ms = ms;
                    }
                }
                state.editing_step_delay = false;
                state.step_delay_buf.clear();
            }
            KeyCode::Esc => {
                state.editing_step_delay = false;
                state.step_delay_buf.clear();
            }
            _ => {
                let mut buf = state.step_delay_buf.clone();
                handle_text_motion(state, &mut buf, key, mods, false, true);
                state.step_delay_buf = buf;
            }
        }
        return;
    }

    match key {
        KeyCode::Esc => {
            state.view = View::Config(idx);
        }
        KeyCode::Up => wrap_nav(&mut state.selected_step, step_count, true),
        KeyCode::Down => wrap_nav(&mut state.selected_step, step_count, false),
        KeyCode::Char('d') | KeyCode::Char('D') => {
            if step_count > 0 {
                let (start, end) = state.step_selection_range();
                if end < step_count {
                    let count = end - start + 1;
                    state.macros[idx].steps.drain(start..=end);
                    let new_len = state.macros[idx].steps.len();
                    state.selected_step = if new_len == 0 { 0 } else { start.min(new_len - 1) };
                    state.selection_anchor = None;
                    state.set_status(format!("Deleted {} step(s)", count));
                }
            }
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
        KeyCode::Char('k') | KeyCode::Char('K') => {
            if state.selected_step > 0 {
                state.macros[idx]
                    .steps
                    .swap(state.selected_step, state.selected_step - 1);
                state.selected_step -= 1;
            }
        }
        KeyCode::Char('j') | KeyCode::Char('J') => {
            if state.selected_step + 1 < step_count {
                state.macros[idx]
                    .steps
                    .swap(state.selected_step, state.selected_step + 1);
                state.selected_step += 1;
            }
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            // Replace key/button (keeps Down/Up direction)
            if step_count > 0 {
                state.editing_step_action = true;
                AWAITING_HOTKEY.store(true, Ordering::SeqCst);
                state.set_status_persistent("Press a key or mouse button to replace action...");
            }
        }
        KeyCode::Char('s') | KeyCode::Char('S') => {
            // Swap Down <-> Up
            if let Some(step) = state.macros[idx].steps.get_mut(state.selected_step) {
                step.action = match step.action.clone() {
                    KeyAction::KeyDown(sc) => KeyAction::KeyUp(sc),
                    KeyAction::KeyUp(sc) => KeyAction::KeyDown(sc),
                    KeyAction::MouseDown(b) => KeyAction::MouseUp(b),
                    KeyAction::MouseUp(b) => KeyAction::MouseDown(b),
                    other => other, // TypeText stays as-is
                };
                let name = step.action.display_name();
                state.set_status(format!("Swapped to {}", name));
            }
        }
        KeyCode::Char('w') | KeyCode::Char('W') => {
            // Insert scroll up step
            let pos = (state.selected_step + 1).min(state.macros[idx].steps.len());
            state.macros[idx].steps.insert(pos, MacroStep {
                action: KeyAction::MouseScroll(ScrollDirection::Up, 1),
                delay_ms: 0,
            });
            state.selected_step = pos;
            state.set_status("Inserted Scroll Up");
        }
        KeyCode::Char('x') | KeyCode::Char('X') => {
            // Insert scroll down step
            let pos = (state.selected_step + 1).min(state.macros[idx].steps.len());
            state.macros[idx].steps.insert(pos, MacroStep {
                action: KeyAction::MouseScroll(ScrollDirection::Down, 1),
                delay_ms: 0,
            });
            state.selected_step = pos;
            state.set_status("Inserted Scroll Down");
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            // Insert MouseMove step at current cursor position
            let (cx, cy) = input::get_cursor_pos();
            let pos = (state.selected_step + 1).min(state.macros[idx].steps.len());
            state.macros[idx].steps.insert(pos, MacroStep {
                action: KeyAction::MouseMove(cx, cy),
                delay_ms: 0,
            });
            state.selected_step = pos;
            state.set_status(format!("Inserted Move ({},{})", cx, cy));
        }
        KeyCode::Char('f') | KeyCode::Char('F') => {
            // Insert WaitForWindow step using current foreground process
            if let Some(ref fg) = state.foreground_process {
                let pos = (state.selected_step + 1).min(state.macros[idx].steps.len());
                state.macros[idx].steps.insert(pos, MacroStep {
                    action: KeyAction::WaitForWindow(fg.clone()),
                    delay_ms: 0,
                });
                state.selected_step = pos;
                state.set_status(format!("Inserted Wait for '{}'", fg));
            } else {
                state.set_status("No foreground process detected");
            }
        }
        KeyCode::Char('i') | KeyCode::Char('I') => {
            // Insert TypeText step
            state.inserting_text = true;
            state.insert_text_buf.clear();
            state.text_cursor = 0;
            state.set_status_persistent("Type text and press Enter to insert...");
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            // Insert key step via capture
            state.inserting_key_step = true;
            AWAITING_HOTKEY.store(true, Ordering::SeqCst);
            state.set_status_persistent("Press a key to insert (down+up pair)...");
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            // Duplicate selected step(s)
            if step_count > 0 {
                let (start, end) = state.step_selection_range();
                let duped = state.macros[idx].steps[start..=end].to_vec();
                let count = duped.len();
                for (i, step) in duped.into_iter().enumerate() {
                    state.macros[idx].steps.insert(end + 1 + i, step);
                }
                state.selected_step = end + 1;
                state.selection_anchor = None;
                state.set_status(format!("Duplicated {} step(s)", count));
            }
        }
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            // Copy selected step(s) to clipboard
            if step_count > 0 {
                let (start, end) = state.step_selection_range();
                state.clipboard_steps = state.macros[idx].steps[start..=end].to_vec();
                let count = state.clipboard_steps.len();
                state.set_status(format!("Copied {} step(s)", count));
            }
        }
        KeyCode::Char('p') | KeyCode::Char('P') => {
            // Paste clipboard steps after selection
            if !state.clipboard_steps.is_empty() {
                let pos = (state.selected_step + 1).min(state.macros[idx].steps.len());
                let count = state.clipboard_steps.len();
                for (i, step) in state.clipboard_steps.clone().into_iter().enumerate() {
                    state.macros[idx].steps.insert(pos + i, step);
                }
                state.selected_step = pos;
                state.selection_anchor = None;
                state.set_status(format!("Pasted {} step(s)", count));
            } else {
                state.set_status("Nothing to paste — copy first with y");
            }
        }
        KeyCode::Char('v') | KeyCode::Char('V') => {
            // Toggle selection anchor
            if step_count > 0 {
                if state.selection_anchor.is_some() {
                    state.selection_anchor = None;
                    state.set_status("Selection cleared");
                } else {
                    state.selection_anchor = Some(state.selected_step);
                    state.set_status("Selection started — move with Up/Down, then y/c/d");
                }
            }
        }
        _ => {}
    }
}

// ── Process Picker ──

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

// ── Chain Picker ──

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

// ── Help ──

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

// ── Helpers ──

/// Wrapping navigation: move cursor up/down in a list of `count` items.
fn wrap_nav(current: &mut usize, count: usize, up: bool) {
    if count == 0 {
        return;
    }
    if up {
        *current = if *current > 0 { *current - 1 } else { count - 1 };
    } else {
        *current = if *current + 1 < count { *current + 1 } else { 0 };
    }
}

fn update_hotkey_config(state: &AppState, config: &Arc<Mutex<HotkeyConfig>>) {
    let entries: Vec<(i32, RepetitionMode)> = state
        .macros
        .iter()
        .filter(|m| m.hotkey_vk != 0)
        .map(|m| (m.hotkey_vk, m.repetition))
        .collect();
    if let Ok(mut cfg) = config.lock() {
        cfg.bound_keys = entries;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

