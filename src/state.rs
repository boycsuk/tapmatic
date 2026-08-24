use std::collections::HashSet;
use std::time::Instant;

use std::sync::mpsc;

use crate::macro_def::{KeyAction, Macro, MacroStep, MouseButton};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortMode {
    None,
    Name,
    Hotkey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum View {
    Main,
    Recording,
    Config(usize),        // index into macros vec
    StepEditor(usize),    // index into macros vec — edit individual steps
    ProcessPicker(usize), // picking process for macro at index
    ChainPicker(usize),   // picking chain macro for macro at index
    Help,                 // help/info screen
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigField {
    Name,
    Hotkey,
    RepetitionMode,
    DelayMode,
    FixedInterval,
    RandomDelayMin,
    RandomDelayMax,
    StopCondition,
    StopValue,
    CycleDelay,
    StartDelay,
    RequireHeld,
    ExclusiveGroup,
    ChainMacro,
    SendMode,
    BoundProcess,
    MouseJitter,
    HumanizeMs,
}

impl ConfigField {
    /// Current value of this field on `mac`, as the text an edit buffer
    /// should start from. Fields without a text form yield an empty string.
    pub fn read(self, mac: &Macro) -> String {
        match self {
            Self::Name => mac.name.clone(),
            Self::FixedInterval => mac.fixed_interval_ms.to_string(),
            Self::RandomDelayMin => {
                mac.random_delay.map_or(String::new(), |(min, _)| min.to_string())
            }
            Self::RandomDelayMax => {
                mac.random_delay.map_or(String::new(), |(_, max)| max.to_string())
            }
            Self::StopValue => mac.stop_condition.value().map_or(String::new(), |n| n.to_string()),
            Self::CycleDelay => mac.cycle_delay_ms.to_string(),
            Self::StartDelay => mac.start_delay_ms.to_string(),
            Self::MouseJitter => mac.mouse_jitter.to_string(),
            Self::HumanizeMs => mac.humanize_ms.to_string(),
            Self::ExclusiveGroup => mac.exclusive_group.clone().unwrap_or_default(),
            Self::ChainMacro => mac.chain_macro.clone().unwrap_or_default(),
            Self::BoundProcess => mac.bound_process.clone().unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// Apply edited text to this field on `mac`.
    ///
    /// Unparseable input leaves the field unchanged (revert-on-invalid); an
    /// emptied optional field clears it.
    pub fn write(self, mac: &mut Macro, input: &str) {
        match self {
            Self::Name if !input.is_empty() => mac.name = input.to_string(),
            Self::FixedInterval => {
                if let Ok(ms) = input.parse::<u64>()
                    && ms > 0 {
                        mac.fixed_interval_ms = ms;
                    }
            }
            Self::RandomDelayMin => {
                if let Ok(ms) = input.parse::<u64>() {
                    let max = mac.random_delay.map_or(ms, |(_, mx)| mx);
                    mac.random_delay = Some((ms, max.max(ms)));
                } else if input.is_empty() {
                    mac.random_delay = None;
                }
            }
            Self::RandomDelayMax => {
                if let Ok(ms) = input.parse::<u64>() {
                    let min = mac.random_delay.map_or(0, |(mn, _)| mn);
                    mac.random_delay = Some((min, ms.max(min)));
                }
            }
            Self::StopValue => {
                if let Ok(val) = input.parse::<u32>()
                    && val > 0 {
                        mac.stop_condition = mac.stop_condition.with_value(val);
                    }
            }
            Self::CycleDelay => {
                if let Ok(ms) = input.parse::<u64>() {
                    mac.cycle_delay_ms = ms;
                }
            }
            Self::StartDelay => {
                if let Ok(ms) = input.parse::<u64>() {
                    mac.start_delay_ms = ms;
                }
            }
            Self::MouseJitter => {
                if let Ok(px) = input.parse::<u32>() {
                    mac.mouse_jitter = px;
                }
            }
            Self::HumanizeMs => {
                if let Ok(ms) = input.parse::<u64>() {
                    mac.humanize_ms = ms;
                }
            }
            Self::ExclusiveGroup | Self::ChainMacro | Self::BoundProcess => {
                let val = input.trim().to_string();
                let opt = if val.is_empty() {
                    None
                } else if self == Self::BoundProcess {
                    Some(val.to_lowercase())
                } else {
                    Some(val)
                };
                match self {
                    Self::ExclusiveGroup => mac.exclusive_group = opt,
                    Self::ChainMacro => mac.chain_macro = opt,
                    Self::BoundProcess => mac.bound_process = opt,
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

const CONFIG_TAB_BASIC: &[ConfigField] = &[
    ConfigField::Name,
    ConfigField::Hotkey,
    ConfigField::RepetitionMode,
    ConfigField::StopCondition,
    ConfigField::StopValue,
];

const CONFIG_TAB_TIMING: &[ConfigField] = &[
    ConfigField::DelayMode,
    ConfigField::FixedInterval,
    ConfigField::RandomDelayMin,
    ConfigField::RandomDelayMax,
    ConfigField::CycleDelay,
    ConfigField::StartDelay,
    ConfigField::HumanizeMs,
];

const CONFIG_TAB_ADVANCED: &[ConfigField] = &[
    ConfigField::RequireHeld,
    ConfigField::ExclusiveGroup,
    ConfigField::ChainMacro,
    ConfigField::SendMode,
    ConfigField::BoundProcess,
    ConfigField::MouseJitter,
];

pub const CONFIG_TABS: &[(&str, &[ConfigField])] = &[
    ("Basic", CONFIG_TAB_BASIC),
    ("Timing", CONFIG_TAB_TIMING),
    ("Advanced", CONFIG_TAB_ADVANCED),
];

impl ConfigField {
    pub fn next_in_tab(self, tab: usize) -> Self {
        let fields = CONFIG_TABS[tab].1;
        let pos = fields.iter().position(|&f| f == self).unwrap_or(0);
        fields[(pos + 1) % fields.len()]
    }

    pub fn prev_in_tab(self, tab: usize) -> Self {
        let fields = CONFIG_TABS[tab].1;
        let pos = fields.iter().position(|&f| f == self).unwrap_or(0);
        fields[(pos + fields.len() - 1) % fields.len()]
    }

    /// Fields that accept free-text input (letters, digits, etc.)
    pub fn is_text_field(self) -> bool {
        matches!(self, Self::Name | Self::BoundProcess | Self::ExclusiveGroup)
    }

    /// Fields that accept only numeric input
    pub fn is_numeric_field(self) -> bool {
        matches!(
            self,
            Self::FixedInterval | Self::RandomDelayMin | Self::RandomDelayMax
            | Self::StopValue | Self::CycleDelay | Self::StartDelay | Self::MouseJitter
            | Self::HumanizeMs
        )
    }
}

#[derive(Debug)]
/// Messages from the background threads (hotkey, executor) to the UI loop.
pub enum AppEvent {
    HotkeyPressed(i32),
    HotkeyReleased(i32),
    RecordKey(KeyAction, Instant),
    HotkeyCaptured(i32),              // vk captured during awaiting_hotkey mode
    MacroProgress(i32, u32, u32),     // vk, current_rep, elapsed_secs
    MacroFinished(i32),               // vk of the macro that finished
    ChainMacro(String),               // name of macro to chain-start
    QuickRecordToggle,                // global quick-record hotkey pressed
    RecordPauseToggle,                // F1 during recording (works without TUI focus)
    RecordMouseToggle,                // F2 during recording
    RecordThresholdCycle,             // F3 during recording
    RecordStop,                       // Esc during recording (works without TUI focus)
    Warning(String),                  // background-thread failure surfaced in the status bar
}

#[derive(Debug)]
pub enum ExecutorCommand {
    StartMacro(Macro),
    StopMacro(i32),
    StopAll,
}

/// The whole mutable UI-thread state: macros, view, selections, edit buffers
/// and status reporting.
pub struct AppState {
    // Macro list
    pub macros: Vec<Macro>,
    pub selected_macro: usize,
    pub active_macros: Vec<i32>,                         // vk codes of currently running macros
    pub macro_progress: std::collections::HashMap<i32, (u32, u32)>, // vk -> (reps, elapsed_secs)

    // View
    pub view: View,
    pub should_quit: bool,

    // Global toggle
    pub macros_enabled: bool,
    /// Delay multiplier applied to every step: values above 1.0 slow playback
    /// down, below 1.0 speed it up. The UI displays its reciprocal, so a stored
    /// 0.25 is shown as "x4.00".
    pub speed_multiplier: f64,

    // Audio
    pub audio_enabled: bool,

    // Foreground process (updated each frame)
    pub foreground_process: Option<String>,

    /// Set when the config file could not be read at startup; blocks autosave
    /// so a failed read is never overwritten by a save.
    pub config_read_failed: bool,

    /// Set by `autosave`, cleared once the event loop writes to disk.
    pub unsaved_changes: bool,

    // Recording
    pub recording_steps: Vec<MacroStep>,
    pub recording_last_instant: Option<Instant>,
    pub recording_name: String,
    pub recording_pressed_keys: HashSet<u16>,
    pub recording_pressed_mouse: HashSet<MouseButton>,
    pub recording_paused: bool,
    pub recording_mouse_moves: bool,
    pub mouse_move_threshold: u32, // min pixels to record a move (default 5)

    // Config
    pub config_field: ConfigField,
    pub config_tab: usize,
    pub config_input_buf: String,
    pub awaiting_hotkey: bool,
    pub awaiting_require_held: bool,
    pub config_macro_backup: Option<Macro>,

    // Step editor
    pub selected_step: usize,
    pub editing_step_delay: bool,
    pub step_delay_buf: String,
    pub editing_step_action: bool,
    pub inserting_text: bool,
    pub insert_text_buf: String,
    pub inserting_key_step: bool, // waiting for key capture to insert a new step
    pub editing_scroll_clicks: bool,
    pub scroll_clicks_buf: String,
    pub selection_anchor: Option<usize>, // start of multi-select range
    pub clipboard_steps: Vec<MacroStep>,

    // Main view
    pub selected_button: usize,
    pub search_query: String,
    pub searching: bool,
    pub sort_mode: SortMode,

    // Process picker
    pub process_list: Vec<(String, String)>, // (exe_name, window_title)
    pub selected_process: usize,

    // Chain picker
    pub chain_list: Vec<String>, // names of macros available to chain
    pub selected_chain: usize,

    // Status message: (text, timestamp, persistent)
    pub status_message: Option<(String, Instant, bool)>,

    // Help
    pub help_scroll: u16,

    // Undo
    pub undo_stack: Vec<Vec<Macro>>, // snapshots of macros list

    pub renaming: bool,
    pub rename_buf: String,

    // Text cursor position for editable fields
    pub text_cursor: usize,

    pub console_hidden: bool,

    pub confirm_delete: bool,

    // System stats
    pub cpu_usage: u32,
    pub ram_used_mb: u32,
    pub ram_total_mb: u32,
    pub uptime_secs: u64,
    pub confirm_quit: bool,

    // Event channel (for starting scroll hook etc.)
    pub event_tx: Option<mpsc::Sender<AppEvent>>,

}

/// Non-persistent status messages disappear after this many seconds.
const STATUS_MESSAGE_TTL_SECS: u64 = 3;
/// Snapshots kept for F-key undo; each one clones the whole macro list.
const UNDO_STACK_CAP: usize = 20;

impl AppState {
    pub fn new() -> Self {
        Self {
            macros: Vec::new(),
            selected_macro: 0,
            active_macros: Vec::new(),
            macro_progress: std::collections::HashMap::new(),
            view: View::Main,
            should_quit: false,
            macros_enabled: true,
            speed_multiplier: 1.0,
            audio_enabled: true,
            foreground_process: None,
            config_read_failed: false,
            unsaved_changes: false,
            recording_steps: Vec::new(),
            recording_last_instant: None,
            recording_name: String::new(),
            recording_pressed_keys: HashSet::new(),
            recording_pressed_mouse: HashSet::new(),
            recording_paused: false,
            recording_mouse_moves: false,
            mouse_move_threshold: crate::DEFAULT_MOUSE_MOVE_THRESHOLD,
            config_field: ConfigField::Name,
            config_tab: 0,
            config_input_buf: String::new(),
            awaiting_hotkey: false,
            awaiting_require_held: false,
            config_macro_backup: None,
            selected_step: 0,
            editing_step_delay: false,
            step_delay_buf: String::new(),
            editing_step_action: false,
            inserting_text: false,
            insert_text_buf: String::new(),
            inserting_key_step: false,
            editing_scroll_clicks: false,
            scroll_clicks_buf: String::new(),
            selection_anchor: None,
            clipboard_steps: Vec::new(),
            selected_button: 0,
            search_query: String::new(),
            searching: false,
            sort_mode: SortMode::None,
            process_list: Vec::new(),
            selected_process: 0,
            chain_list: Vec::new(),
            selected_chain: 0,
            help_scroll: 0,
            undo_stack: Vec::new(),
            renaming: false,
            rename_buf: String::new(),
            text_cursor: 0,
            console_hidden: false,
            status_message: None,
            confirm_delete: false,
            confirm_quit: false,
            cpu_usage: 0,
            ram_used_mb: 0,
            ram_total_mb: 0,
            uptime_secs: 0,
            event_tx: None,
        }
    }

    /// Show a temporary status message (auto-clears after 3s).
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), Instant::now(), false));
    }

    /// Show a persistent status message (stays until manually cleared).
    pub fn set_status_persistent(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), Instant::now(), true));
    }

    pub fn clear_status(&mut self) {
        self.status_message = None;
    }

    pub fn clear_stale_status(&mut self) {
        if let Some((_, t, persistent)) = &self.status_message
            && !persistent && t.elapsed().as_secs() >= STATUS_MESSAGE_TTL_SECS {
                self.status_message = None;
            }
    }

    /// The user-facing speed factor, or `None` at normal speed.
    ///
    /// `speed_multiplier` scales delays (0.5 = half the delay), so the number
    /// people think of as "speed" is its reciprocal.
    pub fn speed_display(&self) -> Option<f64> {
        ((self.speed_multiplier - 1.0).abs() > 0.01).then(|| 1.0 / self.speed_multiplier)
    }

    pub fn is_macro_active(&self, vk: i32) -> bool {
        self.active_macros.contains(&vk)
    }

    pub fn set_macro_active(&mut self, vk: i32, active: bool) {
        if active {
            if !self.active_macros.contains(&vk) {
                self.active_macros.push(vk);
            }
        } else {
            self.active_macros.retain(|&v| v != vk);
            self.macro_progress.remove(&vk);
        }
    }

    pub fn macro_by_hotkey(&self, vk: i32) -> Option<(usize, &Macro)> {
        self.macros
            .iter()
            .enumerate()
            .find(|(_, m)| m.hotkey_vk == vk)
    }

    pub fn next_macro_name(&self) -> String {
        format!("Macro {}", self.macros.len() + 1)
    }

    /// Move the selection cursor one row up or down, wrapping at the ends.
    ///
    /// Wraps against the visible (filtered) row count, since `selected_macro`
    /// indexes the filtered list rather than `macros`.
    pub fn move_selection(&mut self, up: bool) {
        let count = self.filtered_macro_indices().len();
        wrap_nav(&mut self.selected_macro, count, up);
    }

    /// Insert a character at the cursor position.
    pub fn buf_insert(&mut self, buf: &mut String, c: char) {
        buf.insert(byte_index(buf, self.text_cursor), c);
        self.text_cursor += 1;
    }

    /// Delete the character before the cursor (Backspace).
    pub fn buf_backspace(&mut self, buf: &mut String) {
        if self.text_cursor > 0 {
            buf.remove(byte_index(buf, self.text_cursor - 1));
            self.text_cursor -= 1;
        }
    }

    /// Delete the character at the cursor (Delete key).
    pub fn buf_delete(&mut self, buf: &mut String) {
        if self.text_cursor < buf.chars().count() {
            buf.remove(byte_index(buf, self.text_cursor));
        }
    }

    /// Delete the word before the cursor (Ctrl+Backspace).
    pub fn buf_backspace_word(&mut self, buf: &mut String) {
        if self.text_cursor == 0 { return; }
        let pos = word_left_pos(buf, self.text_cursor);
        let byte_start = byte_index(buf, pos);
        let byte_end = byte_index(buf, self.text_cursor);
        buf.drain(byte_start..byte_end);
        self.text_cursor = pos;
    }

    /// Move cursor left by one character.
    pub fn buf_cursor_left(&mut self) {
        if self.text_cursor > 0 {
            self.text_cursor -= 1;
        }
    }

    /// Move cursor right by one character.
    pub fn buf_cursor_right(&mut self, buf_len: usize) {
        if self.text_cursor < buf_len {
            self.text_cursor += 1;
        }
    }

    /// Move cursor to the start of the previous word (Ctrl+Left).
    pub fn buf_cursor_word_left(&mut self, buf: &str) {
        self.text_cursor = word_left_pos(buf, self.text_cursor);
    }

    /// Move cursor to the end of the next word (Ctrl+Right).
    pub fn buf_cursor_word_right(&mut self, buf: &str) {
        let chars: Vec<char> = buf.chars().collect();
        let len = chars.len();
        let mut pos = self.text_cursor;
        while pos < len && chars[pos] != ' ' { pos += 1; }
        while pos < len && chars[pos] == ' ' { pos += 1; }
        self.text_cursor = pos;
    }

    /// Move cursor to start (Home).
    pub fn buf_cursor_home(&mut self) {
        self.text_cursor = 0;
    }

    /// Move cursor to end (End).
    pub fn buf_cursor_end_pos(&mut self, buf: &str) {
        self.text_cursor = buf.chars().count();
    }

    /// Format a buffer with cursor indicator for display.
    pub fn buf_display(buf: &str, cursor: usize) -> String {
        let left: String = buf.chars().take(cursor).collect();
        let right: String = buf.chars().skip(cursor).collect();
        format!("{}|{}", left, right)
    }

    /// Clamp the selection cursor into the visible (filtered) list.
    ///
    /// `selected_macro` indexes the filtered list, not `macros`, so the bound
    /// is the number of visible rows.
    pub fn clamp_selection(&mut self) {
        let visible = self.filtered_macro_indices().len();
        self.selected_macro = self.selected_macro.min(visible.saturating_sub(1));
    }

    pub fn push_undo(&mut self) {
        if self.undo_stack.len() >= UNDO_STACK_CAP {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(self.macros.clone());
    }

    pub fn pop_undo(&mut self) -> bool {
        if let Some(prev) = self.undo_stack.pop() {
            self.macros = prev;
            self.selected_macro = self.selected_macro.min(self.macros.len().saturating_sub(1));
            true
        } else {
            false
        }
    }

    pub fn step_selection_range(&self, step_count: usize) -> (usize, usize) {
        let last = step_count.saturating_sub(1);
        if let Some(anchor) = self.selection_anchor {
            (
                anchor.min(self.selected_step).min(last),
                anchor.max(self.selected_step).min(last),
            )
        } else {
            (self.selected_step.min(last), self.selected_step.min(last))
        }
    }

    pub fn filtered_macro_indices(&self) -> Vec<usize> {
        if self.search_query.is_empty() {
            (0..self.macros.len()).collect()
        } else {
            let q = self.search_query.to_lowercase();
            self.macros
                .iter()
                .enumerate()
                .filter(|(_, m)| m.name.to_lowercase().contains(&q))
                .map(|(i, _)| i)
                .collect()
        }
    }

    pub fn apply_sort(&mut self) {
        match self.sort_mode {
            SortMode::None => {}
            SortMode::Name => self.macros.sort_by_key(|a| a.name.to_lowercase()),
            SortMode::Hotkey => self.macros.sort_by_key(|m| m.hotkey_vk),
        }
        self.selected_macro = 0;
    }
}

/// Move `current` one step through `count` items, wrapping at both ends.
pub fn wrap_nav(current: &mut usize, count: usize, up: bool) {
    if count == 0 {
        return;
    }
    if up {
        *current = if *current > 0 { *current - 1 } else { count - 1 };
    } else {
        *current = if *current + 1 < count { *current + 1 } else { 0 };
    }
}

/// Byte offset of the `char_pos`-th character, or the buffer's end.
fn byte_index(buf: &str, char_pos: usize) -> usize {
    buf.char_indices().nth(char_pos).map_or(buf.len(), |(i, _)| i)
}

/// Character position of the start of the word before `from`: skips trailing
/// spaces, then the word itself.
fn word_left_pos(buf: &str, from: usize) -> usize {
    let chars: Vec<char> = buf.chars().collect();
    let mut pos = from;
    while pos > 0 && chars[pos - 1] == ' ' { pos -= 1; }
    while pos > 0 && chars[pos - 1] != ' ' { pos -= 1; }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macro_def::Macro;

    fn state_with_macros(names: &[&str]) -> AppState {
        let mut s = AppState::new();
        for (i, name) in names.iter().enumerate() {
            let mut m = Macro::new(name.to_string());
            m.hotkey_vk = (0x41 + i as i32).min(0x5A); // A, B, C, ...
            s.macros.push(m);
        }
        s
    }

    #[test]
    fn move_selection_wraps() {
        let mut s = state_with_macros(&["A", "B", "C"]);
        s.selected_macro = 0;
        s.move_selection(true); // up from 0 wraps to 2
        assert_eq!(s.selected_macro, 2);
        s.move_selection(false); // down from 2 wraps to 0
        assert_eq!(s.selected_macro, 0);
    }

    #[test]
    fn move_selection_empty() {
        let mut s = AppState::new();
        s.move_selection(true);
        assert_eq!(s.selected_macro, 0);
        s.move_selection(false);
        assert_eq!(s.selected_macro, 0);
    }

    #[test]
    fn move_selection_sequential() {
        let mut s = state_with_macros(&["A", "B", "C"]);
        s.selected_macro = 0;
        s.move_selection(false);
        assert_eq!(s.selected_macro, 1);
        s.move_selection(false);
        assert_eq!(s.selected_macro, 2);
        s.move_selection(true);
        assert_eq!(s.selected_macro, 1);
    }

    #[test]
    fn clamp_selection_adjusts() {
        let mut s = state_with_macros(&["A", "B"]);
        s.selected_macro = 5;
        s.clamp_selection();
        assert_eq!(s.selected_macro, 1);

        s.selected_macro = 2;
        s.clamp_selection();
        assert_eq!(s.selected_macro, 1);
    }

    #[test]
    fn filtered_selection_resolves_to_real_index() {
        let mut s = state_with_macros(&["alpha", "beta", "gamma"]);
        s.search_query = "gam".into();
        s.selected_macro = 0; // first (and only) visible row

        let filtered = s.filtered_macro_indices();
        let real = filtered[s.selected_macro];
        assert_eq!(real, 2, "display row 0 under this filter is macros[2]");
        assert_eq!(s.macros[real].name, "gamma");
    }

    #[test]
    fn clamp_selection_bounds_to_filtered_list() {
        let mut s = state_with_macros(&["alpha", "beta", "gamma"]);
        s.search_query = "a".into(); // matches alpha, beta, gamma
        s.selected_macro = 2;
        s.clamp_selection();
        assert_eq!(s.selected_macro, 2);

        s.search_query = "alph".into(); // matches alpha only
        s.clamp_selection();
        assert_eq!(s.selected_macro, 0);
    }

    #[test]
    fn move_selection_wraps_within_filtered_list() {
        let mut s = state_with_macros(&["alpha", "beta", "gamma"]);
        s.search_query = "a".into();
        s.selected_macro = 2;
        s.move_selection(false);
        assert_eq!(s.selected_macro, 0, "wraps at the end of the visible list");

        s.search_query = "et".into(); // matches beta only
        s.selected_macro = 0;
        s.move_selection(false);
        assert_eq!(s.selected_macro, 0, "single visible row stays put");
    }

    #[test]
    fn clamp_selection_already_valid() {
        let mut s = state_with_macros(&["A", "B"]);
        s.selected_macro = 1;
        s.clamp_selection();
        assert_eq!(s.selected_macro, 1);
    }

    #[test]
    fn macro_active_tracking() {
        let mut s = AppState::new();
        assert!(!s.is_macro_active(0x41));

        s.set_macro_active(0x41, true);
        assert!(s.is_macro_active(0x41));

        s.set_macro_active(0x41, true); // duplicate, should not double-add
        assert_eq!(s.active_macros.len(), 1);

        s.set_macro_active(0x41, false);
        assert!(!s.is_macro_active(0x41));
        assert!(s.active_macros.is_empty());
    }

    #[test]
    fn macro_by_hotkey_found() {
        let s = state_with_macros(&["Alpha", "Beta"]);
        let result = s.macro_by_hotkey(0x42); // B
        assert!(result.is_some());
        let (idx, mac) = result.unwrap();
        assert_eq!(idx, 1);
        assert_eq!(mac.name, "Beta");
    }

    #[test]
    fn macro_by_hotkey_not_found() {
        let s = state_with_macros(&["Alpha"]);
        assert!(s.macro_by_hotkey(0xFF).is_none());
    }

    #[test]
    fn next_macro_name() {
        let s = AppState::new();
        assert_eq!(s.next_macro_name(), "Macro 1");
        let s = state_with_macros(&["A", "B", "C"]);
        assert_eq!(s.next_macro_name(), "Macro 4");
    }

    #[test]
    fn filtered_macro_indices_no_query() {
        let s = state_with_macros(&["Alpha", "Beta", "Gamma"]);
        assert_eq!(s.filtered_macro_indices(), vec![0, 1, 2]);
    }

    #[test]
    fn filtered_macro_indices_with_query() {
        let mut s = state_with_macros(&["Alpha", "Beta", "Gamma"]);
        s.search_query = "lph".into(); // matches only Alpha
        let filtered = s.filtered_macro_indices();
        assert_eq!(filtered, vec![0]);
    }

    #[test]
    fn filtered_macro_indices_no_match() {
        let mut s = state_with_macros(&["Alpha", "Beta"]);
        s.search_query = "zzz".into();
        assert!(s.filtered_macro_indices().is_empty());
    }

    #[test]
    fn apply_sort_by_name() {
        let mut s = state_with_macros(&["Charlie", "Alpha", "Bravo"]);
        s.sort_mode = SortMode::Name;
        s.apply_sort();
        let names: Vec<&str> = s.macros.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "Bravo", "Charlie"]);
        assert_eq!(s.selected_macro, 0);
    }

    #[test]
    fn apply_sort_by_hotkey() {
        let mut s = AppState::new();
        let mut m1 = Macro::new("Z".into());
        m1.hotkey_vk = 0x50;
        let mut m2 = Macro::new("A".into());
        m2.hotkey_vk = 0x10;
        s.macros.push(m1);
        s.macros.push(m2);
        s.sort_mode = SortMode::Hotkey;
        s.apply_sort();
        assert_eq!(s.macros[0].hotkey_vk, 0x10);
        assert_eq!(s.macros[1].hotkey_vk, 0x50);
    }

    #[test]
    fn undo_push_pop() {
        let mut s = state_with_macros(&["A", "B"]);
        s.push_undo();
        s.macros.remove(0);
        assert_eq!(s.macros.len(), 1);

        assert!(s.pop_undo());
        assert_eq!(s.macros.len(), 2);
        assert_eq!(s.macros[0].name, "A");
    }

    #[test]
    fn undo_empty_stack() {
        let mut s = AppState::new();
        assert!(!s.pop_undo());
    }

    #[test]
    fn undo_stack_limit() {
        let mut s = AppState::new();
        for _ in 0..25 {
            s.push_undo();
        }
        assert_eq!(s.undo_stack.len(), 20); // capped at 20
    }

    #[test]
    fn step_selection_range_no_anchor() {
        let mut s = AppState::new();
        s.selected_step = 5;
        assert_eq!(s.step_selection_range(10), (5, 5));
    }

    #[test]
    fn step_selection_range_with_anchor() {
        let mut s = AppState::new();
        s.selected_step = 7;
        s.selection_anchor = Some(3);
        assert_eq!(s.step_selection_range(10), (3, 7));

        s.selected_step = 1;
        s.selection_anchor = Some(5);
        assert_eq!(s.step_selection_range(10), (1, 5));
    }

    #[test]
    fn step_selection_range_clamps_stale_anchor() {
        // Anchor left over from a longer macro must not index past the list.
        let mut s = AppState::new();
        s.selected_step = 7;
        s.selection_anchor = Some(3);
        assert_eq!(s.step_selection_range(2), (1, 1));
        s.selection_anchor = None;
        s.selected_step = 9;
        assert_eq!(s.step_selection_range(2), (1, 1));
        assert_eq!(s.step_selection_range(0), (0, 0));
    }

    #[test]
    fn config_field_next_prev_in_tab() {
        let first = CONFIG_TAB_BASIC[0]; // Name
        let second = CONFIG_TAB_BASIC[1]; // Hotkey
        let last = *CONFIG_TAB_BASIC.last().unwrap();

        assert_eq!(first.next_in_tab(0), second);
        assert_eq!(last.next_in_tab(0), first); // wraps
        assert_eq!(first.prev_in_tab(0), last); // wraps back
    }

    #[test]
    fn config_field_text_vs_numeric() {
        assert!(ConfigField::Name.is_text_field());
        assert!(ConfigField::BoundProcess.is_text_field());
        assert!(ConfigField::ExclusiveGroup.is_text_field());
        assert!(!ConfigField::FixedInterval.is_text_field());

        assert!(ConfigField::FixedInterval.is_numeric_field());
        assert!(ConfigField::MouseJitter.is_numeric_field());
        assert!(ConfigField::CycleDelay.is_numeric_field());
        assert!(!ConfigField::Name.is_numeric_field());
        assert!(!ConfigField::Hotkey.is_numeric_field());
    }

    #[test]
    fn status_message_lifecycle() {
        let mut s = AppState::new();
        assert!(s.status_message.is_none());

        s.set_status("Hello");
        assert!(s.status_message.is_some());
        assert_eq!(s.status_message.as_ref().unwrap().0, "Hello");
        assert!(!s.status_message.as_ref().unwrap().2); // not persistent

        s.set_status_persistent("Stay");
        assert_eq!(s.status_message.as_ref().unwrap().0, "Stay");
        assert!(s.status_message.as_ref().unwrap().2); // persistent

        s.clear_status();
        assert!(s.status_message.is_none());
    }
}
