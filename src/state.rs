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

// ── Views ──

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

pub const CONFIG_TAB_BASIC: &[ConfigField] = &[
    ConfigField::Name,
    ConfigField::Hotkey,
    ConfigField::RepetitionMode,
    ConfigField::StopCondition,
    ConfigField::StopValue,
];

pub const CONFIG_TAB_TIMING: &[ConfigField] = &[
    ConfigField::DelayMode,
    ConfigField::FixedInterval,
    ConfigField::RandomDelayMin,
    ConfigField::RandomDelayMax,
    ConfigField::CycleDelay,
    ConfigField::StartDelay,
    ConfigField::HumanizeMs,
];

pub const CONFIG_TAB_ADVANCED: &[ConfigField] = &[
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

// ── Events between threads ──

#[derive(Debug)]
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
}

#[derive(Debug)]
pub enum ExecutorCommand {
    StartMacro(Macro),
    StopMacro(i32),
    StopAll,
}

// ── Application state ──

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
    /// Global speed multiplier (1.0 = normal, 0.5 = 2x fast, 2.0 = 2x slow)
    pub speed_multiplier: f64,

    // Audio
    pub audio_enabled: bool,

    // Foreground process (updated each frame)
    pub foreground_process: Option<String>,

    // Recording
    pub recording_steps: Vec<MacroStep>,
    pub recording_last_instant: Option<Instant>,
    pub recording_start: Option<Instant>,
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
    pub clipboard_steps: Vec<MacroStep>, // copied steps

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

    // Stats
    pub macro_activations: std::collections::HashMap<i32, u32>, // vk -> activation count this session

    // Renaming
    pub renaming: bool,
    pub rename_buf: String,

    // Text cursor position for editable fields
    pub text_cursor: usize,

    // Console visibility
    pub console_hidden: bool,

    // Delete confirmation
    pub confirm_delete: bool,

    // System stats
    pub cpu_usage: u32,
    pub ram_used_mb: u32,
    pub ram_total_mb: u32,
    pub uptime_secs: u64,
    // Quit confirmation
    pub confirm_quit: bool,

    // Event channel (for starting scroll hook etc.)
    pub event_tx: Option<mpsc::Sender<AppEvent>>,

}

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
            recording_steps: Vec::new(),
            recording_last_instant: None,
            recording_start: None,
            recording_name: String::new(),
            recording_pressed_keys: HashSet::new(),
            recording_pressed_mouse: HashSet::new(),
            recording_paused: false,
            recording_mouse_moves: false,
            mouse_move_threshold: 5,
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
            macro_activations: std::collections::HashMap::new(),
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
        if let Some((_, t, persistent)) = &self.status_message {
            if !persistent && t.elapsed().as_secs() >= 3 {
                self.status_message = None;
            }
        }
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

    pub fn move_selection(&mut self, up: bool) {
        let count = self.macros.len();
        if count == 0 {
            return;
        }
        if up {
            self.selected_macro = if self.selected_macro > 0 { self.selected_macro - 1 } else { count - 1 };
        } else {
            self.selected_macro = if self.selected_macro + 1 < count { self.selected_macro + 1 } else { 0 };
        }
    }

    // ── Text buffer editing helpers ──

    /// Insert a character at the cursor position.
    pub fn buf_insert(&mut self, buf: &mut String, c: char) {
        let byte_pos = buf.char_indices().nth(self.text_cursor).map_or(buf.len(), |(i, _)| i);
        buf.insert(byte_pos, c);
        self.text_cursor += 1;
    }

    /// Delete the character before the cursor (Backspace).
    pub fn buf_backspace(&mut self, buf: &mut String) {
        if self.text_cursor > 0 {
            let byte_pos = buf.char_indices().nth(self.text_cursor - 1).map(|(i, _)| i).unwrap_or(0);
            buf.remove(byte_pos);
            self.text_cursor -= 1;
        }
    }

    /// Delete the character at the cursor (Delete key).
    pub fn buf_delete(&mut self, buf: &mut String) {
        let len = buf.chars().count();
        if self.text_cursor < len {
            let byte_pos = buf.char_indices().nth(self.text_cursor).map(|(i, _)| i).unwrap_or(buf.len());
            buf.remove(byte_pos);
        }
    }

    /// Delete the word before the cursor (Ctrl+Backspace).
    pub fn buf_backspace_word(&mut self, buf: &mut String) {
        if self.text_cursor == 0 { return; }
        let chars: Vec<char> = buf.chars().collect();
        let mut pos = self.text_cursor;
        // Skip whitespace before cursor
        while pos > 0 && chars[pos - 1] == ' ' { pos -= 1; }
        // Skip word characters
        while pos > 0 && chars[pos - 1] != ' ' { pos -= 1; }
        // Remove from pos to cursor
        let byte_start = buf.char_indices().nth(pos).map_or(buf.len(), |(i, _)| i);
        let byte_end = buf.char_indices().nth(self.text_cursor).map_or(buf.len(), |(i, _)| i);
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
        if self.text_cursor == 0 { return; }
        let chars: Vec<char> = buf.chars().collect();
        let mut pos = self.text_cursor;
        // Skip whitespace
        while pos > 0 && chars[pos - 1] == ' ' { pos -= 1; }
        // Skip word
        while pos > 0 && chars[pos - 1] != ' ' { pos -= 1; }
        self.text_cursor = pos;
    }

    /// Move cursor to the end of the next word (Ctrl+Right).
    pub fn buf_cursor_word_right(&mut self, buf: &str) {
        let chars: Vec<char> = buf.chars().collect();
        let len = chars.len();
        let mut pos = self.text_cursor;
        // Skip current word
        while pos < len && chars[pos] != ' ' { pos += 1; }
        // Skip whitespace
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

    pub fn clamp_selection(&mut self) {
        if self.selected_macro >= self.macros.len() && self.selected_macro > 0 {
            self.selected_macro -= 1;
        }
    }

    pub fn push_undo(&mut self) {
        if self.undo_stack.len() >= 20 {
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

    pub fn step_selection_range(&self) -> (usize, usize) {
        if let Some(anchor) = self.selection_anchor {
            (anchor.min(self.selected_step), anchor.max(self.selected_step))
        } else {
            (self.selected_step, self.selected_step)
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
            SortMode::Name => self.macros.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            SortMode::Hotkey => self.macros.sort_by_key(|m| m.hotkey_vk),
        }
        self.selected_macro = 0;
    }
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

    // ── Navigation ──

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

    // ── Clamp selection ──

    #[test]
    fn clamp_selection_adjusts() {
        let mut s = state_with_macros(&["A", "B"]);
        s.selected_macro = 5; // out of bounds
        s.clamp_selection();
        assert_eq!(s.selected_macro, 4); // decrements by 1 (still out, but that's the contract)

        s.selected_macro = 2;
        s.clamp_selection();
        assert_eq!(s.selected_macro, 1);
    }

    #[test]
    fn clamp_selection_already_valid() {
        let mut s = state_with_macros(&["A", "B"]);
        s.selected_macro = 1;
        s.clamp_selection();
        assert_eq!(s.selected_macro, 1);
    }

    // ── Macro active tracking ──

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

    // ── Macro by hotkey ──

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

    // ── Next macro name ──

    #[test]
    fn next_macro_name() {
        let s = AppState::new();
        assert_eq!(s.next_macro_name(), "Macro 1");
        let s = state_with_macros(&["A", "B", "C"]);
        assert_eq!(s.next_macro_name(), "Macro 4");
    }

    // ── Search / filter ──

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

    // ── Sort ──

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

    // ── Undo ──

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

    // ── Step selection range ──

    #[test]
    fn step_selection_range_no_anchor() {
        let mut s = AppState::new();
        s.selected_step = 5;
        assert_eq!(s.step_selection_range(), (5, 5));
    }

    #[test]
    fn step_selection_range_with_anchor() {
        let mut s = AppState::new();
        s.selected_step = 7;
        s.selection_anchor = Some(3);
        assert_eq!(s.step_selection_range(), (3, 7));

        s.selected_step = 1;
        s.selection_anchor = Some(5);
        assert_eq!(s.step_selection_range(), (1, 5));
    }

    // ── ConfigField helpers ──

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

    // ── Status messages ──

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
