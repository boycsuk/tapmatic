use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepetitionMode {
    Toggle,
    HoldToRepeat,
    SingleShot,
}

impl RepetitionMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Toggle => "Toggle",
            Self::HoldToRepeat => "Hold",
            Self::SingleShot => "Single",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            Self::Toggle => Self::HoldToRepeat,
            Self::HoldToRepeat => Self::SingleShot,
            Self::SingleShot => Self::Toggle,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Self::Toggle => Self::SingleShot,
            Self::HoldToRepeat => Self::Toggle,
            Self::SingleShot => Self::HoldToRepeat,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScrollDirection {
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyAction {
    KeyDown(u16),
    KeyUp(u16),
    MouseDown(MouseButton),
    MouseUp(MouseButton),
    /// Scroll mouse wheel (direction, clicks — 1 click = WHEEL_DELTA = 120)
    MouseScroll(ScrollDirection, u32),
    /// Move mouse to absolute screen coordinates
    MouseMove(i32, i32),
    /// Type a string of text (sends key events for each character)
    TypeText(String),
    /// Wait until a window with this process name is in the foreground
    WaitForWindow(String),
}

impl KeyAction {
    /// Create a KeyAction from a VK code and scancode.
    /// `down` selects KeyDown/MouseDown vs KeyUp/MouseUp.
    /// Returns None for unrecognized VKs with scancode 0.
    pub fn from_vk(vk: i32, scancode: u16, down: bool) -> Option<Self> {
        match vk {
            0x01 => Some(if down { Self::MouseDown(MouseButton::Left) } else { Self::MouseUp(MouseButton::Left) }),
            0x02 => Some(if down { Self::MouseDown(MouseButton::Right) } else { Self::MouseUp(MouseButton::Right) }),
            0x04 => Some(if down { Self::MouseDown(MouseButton::Middle) } else { Self::MouseUp(MouseButton::Middle) }),
            _ if scancode != 0 => Some(if down { Self::KeyDown(scancode) } else { Self::KeyUp(scancode) }),
            _ => None,
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::KeyDown(sc) => format!("{} \u{2193}", scancode_name(*sc)),
            Self::KeyUp(sc) => format!("{} \u{2191}", scancode_name(*sc)),
            Self::MouseDown(b) => format!("{} \u{2193}", mouse_button_name(*b)),
            Self::MouseUp(b) => format!("{} \u{2191}", mouse_button_name(*b)),
            Self::MouseScroll(dir, clicks) => {
                let arrow = match dir { ScrollDirection::Up => "\u{2191}", ScrollDirection::Down => "\u{2193}" };
                if *clicks == 1 { format!("Scroll {}", arrow) } else { format!("Scroll {}x{}", arrow, clicks) }
            }
            Self::MouseMove(x, y) => format!("Move ({},{})", x, y),
            Self::TypeText(s) => {
                let preview: String = s.chars().take(20).collect();
                if s.len() > 20 {
                    format!("\"{}...\"", preview)
                } else {
                    format!("\"{}\"", preview)
                }
            }
            Self::WaitForWindow(name) => format!("Wait: {}", name),
        }
    }
}

fn mouse_button_name(b: MouseButton) -> &'static str {
    match b {
        MouseButton::Left => "LMB",
        MouseButton::Right => "RMB",
        MouseButton::Middle => "MMB",
    }
}

fn scancode_name(sc: u16) -> String {
    // Common scancodes -> readable names
    match sc {
        0x01 => "Esc".into(),
        0x02..=0x0B => {
            let digits = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'];
            digits[(sc - 0x02) as usize].to_string()
        }
        0x0E => "Backspace".into(),
        0x0F => "Tab".into(),
        0x10 => "Q".into(),
        0x11 => "W".into(),
        0x12 => "E".into(),
        0x13 => "R".into(),
        0x14 => "T".into(),
        0x15 => "Y".into(),
        0x16 => "U".into(),
        0x17 => "I".into(),
        0x18 => "O".into(),
        0x19 => "P".into(),
        0x1C => "Enter".into(),
        0x1D => "LCtrl".into(),
        0x1E => "A".into(),
        0x1F => "S".into(),
        0x20 => "D".into(),
        0x21 => "F".into(),
        0x22 => "G".into(),
        0x23 => "H".into(),
        0x24 => "J".into(),
        0x25 => "K".into(),
        0x26 => "L".into(),
        0x2A => "LShift".into(),
        0x2C => "Z".into(),
        0x2D => "X".into(),
        0x2E => "C".into(),
        0x2F => "V".into(),
        0x30 => "B".into(),
        0x31 => "N".into(),
        0x32 => "M".into(),
        0x36 => "RShift".into(),
        0x38 => "LAlt".into(),
        0x39 => "Space".into(),
        0x3A => "CapsLock".into(),
        0x3B => "F1".into(),
        0x3C => "F2".into(),
        0x3D => "F3".into(),
        0x3E => "F4".into(),
        0x3F => "F5".into(),
        0x40 => "F6".into(),
        0x41 => "F7".into(),
        0x42 => "F8".into(),
        0x43 => "F9".into(),
        0x44 => "F10".into(),
        0x57 => "F11".into(),
        0x58 => "F12".into(),
        _ => format!("SC:{:#04X}", sc),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroStep {
    pub action: KeyAction,
    pub delay_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SendMode {
    /// SendInput — goes to focused window (works with games)
    Global,
    /// PostMessage — targets a specific window even without focus (apps only)
    Window,
}

impl SendMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::Window => "Window",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopCondition {
    /// Run forever (until manually stopped)
    None,
    /// Stop after N repetitions of the full sequence
    AfterReps(u32),
    /// Stop after X seconds
    AfterSecs(u32),
}

impl StopCondition {
    /// Display the value for UI (e.g. "10 reps", "30 secs", "---")
    pub fn display_value(&self) -> String {
        match self {
            Self::None => "---".into(),
            Self::AfterReps(n) => format!("{} reps", n),
            Self::AfterSecs(n) => format!("{} secs", n),
        }
    }

    /// Get the inner numeric value (reps or secs), if any.
    pub fn value(&self) -> Option<u32> {
        match self {
            Self::AfterReps(n) | Self::AfterSecs(n) => Some(*n),
            Self::None => None,
        }
    }

    /// Replace the inner value while keeping the same variant.
    pub fn with_value(self, val: u32) -> Self {
        match self {
            Self::AfterReps(_) => Self::AfterReps(val),
            Self::AfterSecs(_) => Self::AfterSecs(val),
            Self::None => Self::None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Macro {
    pub name: String,
    pub steps: Vec<MacroStep>,
    pub hotkey_vk: i32,
    pub repetition: RepetitionMode,
    pub use_recorded_delays: bool,
    pub fixed_interval_ms: u64,
    /// Random delay range: if set, each step uses a random delay in [min, max] ms
    #[serde(default)]
    pub random_delay: Option<(u64, u64)>,
    /// Automatic stop condition
    #[serde(default = "default_stop_condition")]
    pub stop_condition: StopCondition,
    /// Delay between full repetition cycles (ms)
    #[serde(default)]
    pub cycle_delay_ms: u64,
    /// Delay before first execution after activation (ms)
    #[serde(default)]
    pub start_delay_ms: u64,
    /// Only execute steps while this VK is held down (0 = no requirement)
    #[serde(default)]
    pub require_held_vk: i32,
    /// Exclusive group: activating a macro in this group deactivates others in the same group
    #[serde(default)]
    pub exclusive_group: Option<String>,
    /// Name of macro to chain (auto-start) when this macro finishes
    #[serde(default)]
    pub chain_macro: Option<String>,
    /// Input delivery mode
    #[serde(default = "default_send_mode")]
    pub send_mode: SendMode,
    /// If set, macro only activates when this process has focus (Global)
    /// or targets this process's window (Window mode)
    #[serde(default)]
    pub bound_process: Option<String>,
    /// Random pixel offset applied to MouseMove coordinates (0 = exact)
    #[serde(default)]
    pub mouse_jitter: u32,
    /// Random timing jitter added to each step delay (±ms). 0 = exact timing.
    #[serde(default)]
    pub humanize_ms: u64,
}

fn default_stop_condition() -> StopCondition {
    StopCondition::None
}

fn default_send_mode() -> SendMode {
    SendMode::Global
}

impl Macro {
    pub fn new(name: String) -> Self {
        Self {
            name,
            steps: Vec::new(),
            hotkey_vk: 0,
            repetition: RepetitionMode::Toggle,
            use_recorded_delays: true,
            fixed_interval_ms: 50,
            random_delay: None,
            stop_condition: StopCondition::None,
            cycle_delay_ms: 0,
            start_delay_ms: 0,
            require_held_vk: 0,
            exclusive_group: None,
            chain_macro: None,
            send_mode: SendMode::Global,
            bound_process: None,
            mouse_jitter: 0,
            humanize_ms: 0,
        }
    }

    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    pub fn total_duration_ms(&self) -> u64 {
        self.steps.iter().map(|s| s.delay_ms).sum()
    }
}

/// Convert a Windows virtual-key code to a human-readable name.
pub fn vk_name(vk: i32) -> &'static str {
    match vk {
        0x01 => "LMB",
        0x02 => "RMB",
        0x04 => "MMB",
        0x05 => "X1",
        0x06 => "X2",
        0x08 => "Backspace",
        0x09 => "Tab",
        0x0D => "Enter",
        0x10 => "Shift",
        0x11 => "Ctrl",
        0x12 => "Alt",
        0x13 => "Pause",
        0x14 => "CapsLock",
        0x1B => "Esc",
        0x20 => "Space",
        0x21 => "PgUp",
        0x22 => "PgDn",
        0x23 => "End",
        0x24 => "Home",
        0x25 => "Left",
        0x26 => "Up",
        0x27 => "Right",
        0x28 => "Down",
        0x2D => "Insert",
        0x2E => "Delete",
        0x30 => "0",
        0x31 => "1",
        0x32 => "2",
        0x33 => "3",
        0x34 => "4",
        0x35 => "5",
        0x36 => "6",
        0x37 => "7",
        0x38 => "8",
        0x39 => "9",
        0x41 => "A",
        0x42 => "B",
        0x43 => "C",
        0x44 => "D",
        0x45 => "E",
        0x46 => "F",
        0x47 => "G",
        0x48 => "H",
        0x49 => "I",
        0x4A => "J",
        0x4B => "K",
        0x4C => "L",
        0x4D => "M",
        0x4E => "N",
        0x4F => "O",
        0x50 => "P",
        0x51 => "Q",
        0x52 => "R",
        0x53 => "S",
        0x54 => "T",
        0x55 => "U",
        0x56 => "V",
        0x57 => "W",
        0x58 => "X",
        0x59 => "Y",
        0x5A => "Z",
        0x5B => "LWin",
        0x5C => "RWin",
        0x60 => "Num0",
        0x61 => "Num1",
        0x62 => "Num2",
        0x63 => "Num3",
        0x64 => "Num4",
        0x65 => "Num5",
        0x66 => "Num6",
        0x67 => "Num7",
        0x68 => "Num8",
        0x69 => "Num9",
        0x6A => "Num*",
        0x6B => "Num+",
        0x6D => "Num-",
        0x6E => "Num.",
        0x6F => "Num/",
        0x70 => "F1",
        0x71 => "F2",
        0x72 => "F3",
        0x73 => "F4",
        0x74 => "F5",
        0x75 => "F6",
        0x76 => "F7",
        0x77 => "F8",
        0x78 => "F9",
        0x79 => "F10",
        0x7A => "F11",
        0x7B => "F12",
        0x7C => "F13",
        0x7D => "F14",
        0x7E => "F15",
        0x7F => "F16",
        0x80 => "F17",
        0x81 => "F18",
        0x82 => "F19",
        0x83 => "F20",
        0x84 => "F21",
        0x85 => "F22",
        0x86 => "F23",
        0x87 => "F24",
        0x90 => "NumLock",
        0x91 => "ScrLock",
        0xA0 => "LShift",
        0xA1 => "RShift",
        0xA2 => "LCtrl",
        0xA3 => "RCtrl",
        0xA4 => "LAlt",
        0xA5 => "RAlt",
        // Browser keys
        0xA6 => "BrowserBack",
        0xA7 => "BrowserFwd",
        0xA8 => "BrowserRefresh",
        0xA9 => "BrowserStop",
        0xAA => "BrowserSearch",
        0xAB => "BrowserFav",
        0xAC => "BrowserHome",
        // Media keys
        0xAD => "VolMute",
        0xAE => "VolDown",
        0xAF => "VolUp",
        0xB0 => "NextTrack",
        0xB1 => "PrevTrack",
        0xB2 => "MediaStop",
        0xB3 => "PlayPause",
        0xB4 => "LaunchMail",
        0xB5 => "LaunchMedia",
        0xB6 => "LaunchApp1",
        0xB7 => "LaunchApp2",
        // Punctuation / OEM keys
        0xBA => ";",
        0xBB => "=",
        0xBC => ",",
        0xBD => "-",
        0xBE => ".",
        0xBF => "/",
        0xC0 => "`",
        0xDB => "[",
        0xDC => "\\",
        0xDD => "]",
        0xDE => "'",
        0xDF => "OEM8",
        0xE2 => "OEM102",
        // Misc
        0x03 => "Cancel",
        0x0C => "Clear",
        0x15 => "IME",
        0x1C => "Convert",
        0x1D => "NonConvert",
        0x2C => "PrtSc",
        0x5D => "Menu",
        0x5F => "Sleep",
        0xF6 => "Attn",
        0xFA => "Play",
        0xFE => "Clear",
        _ => "???",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── RepetitionMode ──

    #[test]
    fn repetition_mode_cycle() {
        assert_eq!(RepetitionMode::Toggle.next(), RepetitionMode::HoldToRepeat);
        assert_eq!(RepetitionMode::HoldToRepeat.next(), RepetitionMode::SingleShot);
        assert_eq!(RepetitionMode::SingleShot.next(), RepetitionMode::Toggle);

        assert_eq!(RepetitionMode::Toggle.prev(), RepetitionMode::SingleShot);
        assert_eq!(RepetitionMode::SingleShot.prev(), RepetitionMode::HoldToRepeat);
        assert_eq!(RepetitionMode::HoldToRepeat.prev(), RepetitionMode::Toggle);
    }

    #[test]
    fn repetition_mode_labels() {
        assert_eq!(RepetitionMode::Toggle.label(), "Toggle");
        assert_eq!(RepetitionMode::HoldToRepeat.label(), "Hold");
        assert_eq!(RepetitionMode::SingleShot.label(), "Single");
    }

    // ── StopCondition ──

    #[test]
    fn stop_condition_display() {
        assert_eq!(StopCondition::None.display_value(), "---");
        assert_eq!(StopCondition::AfterReps(10).display_value(), "10 reps");
        assert_eq!(StopCondition::AfterSecs(30).display_value(), "30 secs");
    }

    #[test]
    fn stop_condition_value() {
        assert_eq!(StopCondition::None.value(), None);
        assert_eq!(StopCondition::AfterReps(5).value(), Some(5));
        assert_eq!(StopCondition::AfterSecs(60).value(), Some(60));
    }

    #[test]
    fn stop_condition_with_value() {
        assert_eq!(StopCondition::AfterReps(1).with_value(99), StopCondition::AfterReps(99));
        assert_eq!(StopCondition::AfterSecs(1).with_value(42), StopCondition::AfterSecs(42));
        assert_eq!(StopCondition::None.with_value(100), StopCondition::None);
    }

    // ── KeyAction::from_vk ──

    #[test]
    fn from_vk_mouse_buttons() {
        assert_eq!(KeyAction::from_vk(0x01, 0, true), Some(KeyAction::MouseDown(MouseButton::Left)));
        assert_eq!(KeyAction::from_vk(0x01, 0, false), Some(KeyAction::MouseUp(MouseButton::Left)));
        assert_eq!(KeyAction::from_vk(0x02, 0, true), Some(KeyAction::MouseDown(MouseButton::Right)));
        assert_eq!(KeyAction::from_vk(0x04, 0, false), Some(KeyAction::MouseUp(MouseButton::Middle)));
    }

    #[test]
    fn from_vk_keyboard() {
        assert_eq!(KeyAction::from_vk(0x41, 0x1E, true), Some(KeyAction::KeyDown(0x1E)));
        assert_eq!(KeyAction::from_vk(0x41, 0x1E, false), Some(KeyAction::KeyUp(0x1E)));
    }

    #[test]
    fn from_vk_unknown_returns_none() {
        assert_eq!(KeyAction::from_vk(0xFF, 0, true), None);
        assert_eq!(KeyAction::from_vk(0x03, 0, false), None);
    }

    // ── KeyAction::display_name ──

    #[test]
    fn display_name_keys() {
        assert!(KeyAction::KeyDown(0x1E).display_name().contains("A"));
        assert!(KeyAction::KeyDown(0x1E).display_name().contains("\u{2193}"));
        assert!(KeyAction::KeyUp(0x1E).display_name().contains("\u{2191}"));
    }

    #[test]
    fn display_name_mouse() {
        assert!(KeyAction::MouseDown(MouseButton::Left).display_name().contains("LMB"));
        assert!(KeyAction::MouseUp(MouseButton::Right).display_name().contains("RMB"));
    }

    #[test]
    fn display_name_scroll() {
        assert_eq!(KeyAction::MouseScroll(ScrollDirection::Up, 1).display_name(), "Scroll \u{2191}");
        assert_eq!(KeyAction::MouseScroll(ScrollDirection::Down, 3).display_name(), "Scroll \u{2193}x3");
    }

    #[test]
    fn display_name_move() {
        assert_eq!(KeyAction::MouseMove(100, 200).display_name(), "Move (100,200)");
    }

    #[test]
    fn display_name_text() {
        assert_eq!(KeyAction::TypeText("hello".into()).display_name(), "\"hello\"");
        let long = "a".repeat(25);
        let display = KeyAction::TypeText(long).display_name();
        assert!(display.ends_with("...\""));
    }

    #[test]
    fn display_name_wait() {
        assert_eq!(KeyAction::WaitForWindow("notepad.exe".into()).display_name(), "Wait: notepad.exe");
    }

    // ── Macro ──

    #[test]
    fn macro_new_defaults() {
        let m = Macro::new("Test".into());
        assert_eq!(m.name, "Test");
        assert_eq!(m.steps.len(), 0);
        assert_eq!(m.hotkey_vk, 0);
        assert_eq!(m.repetition, RepetitionMode::Toggle);
        assert!(m.use_recorded_delays);
        assert_eq!(m.fixed_interval_ms, 50);
        assert_eq!(m.random_delay, None);
        assert_eq!(m.stop_condition, StopCondition::None);
        assert_eq!(m.cycle_delay_ms, 0);
        assert_eq!(m.start_delay_ms, 0);
        assert_eq!(m.require_held_vk, 0);
        assert_eq!(m.exclusive_group, None);
        assert_eq!(m.chain_macro, None);
        assert_eq!(m.send_mode, SendMode::Global);
        assert_eq!(m.bound_process, None);
        assert_eq!(m.mouse_jitter, 0);
    }

    #[test]
    fn macro_step_count_and_duration() {
        let mut m = Macro::new("Test".into());
        assert_eq!(m.step_count(), 0);
        assert_eq!(m.total_duration_ms(), 0);

        m.steps.push(MacroStep { action: KeyAction::KeyDown(0x1E), delay_ms: 100 });
        m.steps.push(MacroStep { action: KeyAction::KeyUp(0x1E), delay_ms: 50 });
        assert_eq!(m.step_count(), 2);
        assert_eq!(m.total_duration_ms(), 150);
    }

    // ── Serde round-trip ──

    #[test]
    fn serde_all_action_variants() {
        let actions = vec![
            MacroStep { action: KeyAction::KeyDown(0x1E), delay_ms: 10 },
            MacroStep { action: KeyAction::KeyUp(0x1E), delay_ms: 20 },
            MacroStep { action: KeyAction::MouseDown(MouseButton::Left), delay_ms: 0 },
            MacroStep { action: KeyAction::MouseUp(MouseButton::Right), delay_ms: 5 },
            MacroStep { action: KeyAction::MouseScroll(ScrollDirection::Up, 3), delay_ms: 0 },
            MacroStep { action: KeyAction::MouseScroll(ScrollDirection::Down, 1), delay_ms: 0 },
            MacroStep { action: KeyAction::MouseMove(1920, 1080), delay_ms: 0 },
            MacroStep { action: KeyAction::MouseMove(-10, -20), delay_ms: 0 },
            MacroStep { action: KeyAction::TypeText("hello world".into()), delay_ms: 0 },
            MacroStep { action: KeyAction::WaitForWindow("notepad.exe".into()), delay_ms: 0 },
        ];

        let json = serde_json::to_string(&actions).expect("serialize");
        let parsed: Vec<MacroStep> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(actions, parsed);
    }

    #[test]
    fn serde_full_macro_round_trip() {
        let mut m = Macro::new("Serde Test".into());
        m.hotkey_vk = 0x41;
        m.repetition = RepetitionMode::HoldToRepeat;
        m.use_recorded_delays = false;
        m.fixed_interval_ms = 100;
        m.random_delay = Some((50, 200));
        m.stop_condition = StopCondition::AfterReps(10);
        m.cycle_delay_ms = 500;
        m.start_delay_ms = 1000;
        m.require_held_vk = 0x10;
        m.exclusive_group = Some("group1".into());
        m.chain_macro = Some("Other Macro".into());
        m.send_mode = SendMode::Window;
        m.bound_process = Some("game.exe".into());
        m.mouse_jitter = 5;
        m.steps.push(MacroStep { action: KeyAction::KeyDown(0x1E), delay_ms: 50 });
        m.steps.push(MacroStep { action: KeyAction::MouseMove(100, 200), delay_ms: 0 });

        let macros = vec![m];
        let json = serde_json::to_string_pretty(&macros).expect("serialize");
        let parsed: Vec<Macro> = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(parsed.len(), 1);
        let p = &parsed[0];
        assert_eq!(p.name, "Serde Test");
        assert_eq!(p.hotkey_vk, 0x41);
        assert_eq!(p.repetition, RepetitionMode::HoldToRepeat);
        assert!(!p.use_recorded_delays);
        assert_eq!(p.random_delay, Some((50, 200)));
        assert_eq!(p.stop_condition, StopCondition::AfterReps(10));
        assert_eq!(p.mouse_jitter, 5);
        assert_eq!(p.send_mode, SendMode::Window);
        assert_eq!(p.steps.len(), 2);
        assert_eq!(p.steps[1].action, KeyAction::MouseMove(100, 200));
    }

    #[test]
    fn serde_backward_compat_missing_new_fields() {
        // Simulates loading a macro saved BEFORE mouse_jitter, mouse_scroll, etc. existed
        let old_json = r#"[{
            "name": "Old Macro",
            "steps": [{"action": {"KeyDown": 30}, "delay_ms": 50}],
            "hotkey_vk": 65,
            "repetition": "Toggle",
            "use_recorded_delays": true,
            "fixed_interval_ms": 50
        }]"#;

        let macros: Vec<Macro> = serde_json::from_str(old_json).expect("should parse old format");
        assert_eq!(macros.len(), 1);
        let m = &macros[0];
        assert_eq!(m.name, "Old Macro");
        assert_eq!(m.mouse_jitter, 0);
        assert_eq!(m.stop_condition, StopCondition::None);
        assert_eq!(m.send_mode, SendMode::Global);
        assert_eq!(m.random_delay, None);
        assert_eq!(m.cycle_delay_ms, 0);
        assert_eq!(m.start_delay_ms, 0);
        assert_eq!(m.require_held_vk, 0);
        assert_eq!(m.exclusive_group, None);
        assert_eq!(m.chain_macro, None);
        assert_eq!(m.bound_process, None);
        assert_eq!(m.steps.len(), 1);
    }

    // ── vk_name ──

    #[test]
    fn vk_name_known_keys() {
        assert_eq!(vk_name(0x01), "LMB");
        assert_eq!(vk_name(0x41), "A");
        assert_eq!(vk_name(0x20), "Space");
        assert_eq!(vk_name(0x0D), "Enter");
        assert_eq!(vk_name(0x1B), "Esc");
    }

    #[test]
    fn vk_name_unknown() {
        assert_eq!(vk_name(0x07), "???");
    }

    // ── SendMode ──

    #[test]
    fn send_mode_labels() {
        assert_eq!(SendMode::Global.label(), "Global");
        assert_eq!(SendMode::Window.label(), "Window");
    }
}
