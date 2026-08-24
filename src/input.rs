use crate::macro_def::KeyAction;

/// One mouse-wheel notch, as defined by the Windows WHEEL_DELTA constant.
#[cfg(windows)]
const WHEEL_DELTA: i32 = 120;

/// The `0xE0` prefix byte that marks an extended scancode, stored in the high
/// byte of a recorded scancode.
pub const EXTENDED_PREFIX: u16 = 0xE000;

/// Split a stored scancode into its 8-bit hardware code and whether it is
/// extended.
///
/// Extended keys (the arrow cluster, Insert/Delete/Home/End/PageUp/PageDown,
/// right Ctrl and Alt, numpad Enter and Divide) share their low byte with a
/// numpad key, so the prefix is what tells the two apart. Recorded scancodes
/// carry it in the high byte; anything below `0xE000` is a plain key.
pub fn split_scancode(scancode: u16) -> (u16, bool) {
    if scancode & 0xFF00 == EXTENDED_PREFIX {
        (scancode & 0x00FF, true)
    } else {
        (scancode, false)
    }
}

// Only compiled on Windows. The non-Windows stubs below are no-ops that let
// the crate build and the suite run off-Windows; no test exercising them
// proves anything about real input behaviour.

#[cfg(windows)]
mod platform {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, MapVirtualKeyA, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE,
        KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
        KEYEVENTF_SCANCODE, MAPVK_VK_TO_VSC_EX, MAP_VIRTUAL_KEY_TYPE, MOUSEEVENTF_ABSOLUTE,
        MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
        MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL,
        MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
    };

    use crate::macro_def::MouseButton;

    /// Whether the key is physically down right now (`GetAsyncKeyState`).
    pub fn is_key_pressed(vk: i32) -> bool {
        // Bit 15 of the i16 return is the "currently down" flag, i.e. the
        // sign bit. Testing it directly avoids depending on an i16 -> u16
        // cast preserving the pattern.
        unsafe { GetAsyncKeyState(vk) < 0 }
    }

    /// Map a virtual-key code to its scancode; 0 when the VK has none.
    ///
    /// Uses the `_EX` mapping, which keeps the `0xE0` prefix in the high byte.
    /// Without it the arrow cluster and the numpad collapse onto the same
    /// scancodes (Up and numpad-8 are both `0x48`), and replay picks whichever
    /// one the bare scancode happens to mean — a recorded Up arrow types an 8.
    pub fn vk_to_scancode(vk: i32) -> u16 {
        unsafe { MapVirtualKeyA(vk as u32, MAP_VIRTUAL_KEY_TYPE(MAPVK_VK_TO_VSC_EX.0)) as u16 }
    }

    pub(super) fn keyboard_input(scancode: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: scancode,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn mouse_input(dx: i32, dy: i32, mouse_data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: mouse_data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    /// Inject events, reporting whether all of them were accepted.
    ///
    /// SendInput returns the injected count: UIPI or a secure desktop can
    /// block injection entirely, and ignoring that leaves a macro "running"
    /// while sending nothing.
    pub(super) fn send(inputs: &[INPUT]) -> bool {
        let injected = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
        injected as usize == inputs.len()
    }

    /// Inject a key event globally; false when the OS refused the injection
    /// (UIPI, secure desktop).
    pub fn send_key(scancode: u16, up: bool) -> bool {
        let (sc, extended) = super::split_scancode(scancode);
        let mut flags = KEYEVENTF_SCANCODE;
        if extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if up {
            flags |= KEYEVENTF_KEYUP;
        }
        send(&[keyboard_input(sc, flags)])
    }

    /// Inject a mouse-button event globally; false when the OS refused it.
    pub fn send_mouse(button: MouseButton, up: bool) -> bool {
        let flags = match (button, up) {
            (MouseButton::Left, false) => MOUSEEVENTF_LEFTDOWN,
            (MouseButton::Left, true) => MOUSEEVENTF_LEFTUP,
            (MouseButton::Right, false) => MOUSEEVENTF_RIGHTDOWN,
            (MouseButton::Right, true) => MOUSEEVENTF_RIGHTUP,
            (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEDOWN,
            (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEUP,
        };
        send(&[mouse_input(0, 0, 0, flags)])
    }

    /// Inject a wheel scroll globally; false when the OS refused it.
    pub fn send_mouse_scroll(up: bool, clicks: u32) -> bool {
        // Positive delta = scroll up, negative = scroll down. i64 math with a
        // final clamp: an unclamped clicks value must saturate, not overflow.
        let magnitude = i64::from(crate::input::WHEEL_DELTA) * i64::from(clicks);
        let delta = if up { magnitude } else { -magnitude }
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        send(&[mouse_input(0, 0, delta as u32, MOUSEEVENTF_WHEEL)])
    }

    /// Move the cursor to absolute pixel coordinates; false when refused or
    /// when the screen size cannot be queried.
    pub fn send_mouse_move(x: i32, y: i32) -> bool {
        // Coordinates come from GetCursorPos, which reports virtual-desktop
        // space: a monitor left of the primary yields negative x. Normalizing
        // those against SM_CXSCREEN (the primary alone) clamps every such
        // click to the primary's edge, so use the virtual desktop's own origin
        // and size, and tell SendInput to interpret them that way with
        // MOUSEEVENTF_VIRTUALDESK. All three parts are required together.
        let (ox, oy, sx, sy) = unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{
                GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
                SM_YVIRTUALSCREEN,
            };
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        if sx <= 1 || sy <= 1 { return false; }
        // Microsoft's documented normalization is 65535 / (extent - 1); using
        // 65536 / extent leaves the last row and column unreachable.
        let abs_x = (((x - ox) as i64 * 65535) / (sx - 1) as i64).clamp(0, 65535) as i32;
        let abs_y = (((y - oy) as i64 * 65535) / (sy - 1) as i64).clamp(0, 65535) as i32;
        send(&[mouse_input(
            abs_x,
            abs_y,
            0,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        )])
    }

    /// The cursor's screen position, or `None` when the query fails (e.g.
    /// secure desktop) — callers must not mistake failure for (0,0).
    pub fn get_cursor_pos() -> Option<(i32, i32)> {
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        use windows::Win32::Foundation::POINT;
        let mut pt = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut pt).ok()? };
        Some((pt.x, pt.y))
    }

    /// Get the exe name (lowercase) of a process by PID.
    pub(super) fn exe_name_from_pid(pid: u32) -> Option<String> {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION,
        };
        use windows::core::PWSTR;

        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 260];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            );
            let _ = CloseHandle(handle);
            if ok.is_err() {
                return None;
            }
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            path.rsplit('\\').next().map(|s| s.to_lowercase())
        }
    }

    /// Request 1 ms scheduler resolution so short step delays sleep
    /// accurately; paired with `end_high_res_timer` at shutdown.
    pub fn begin_high_res_timer() {
        unsafe {
            windows::Win32::Media::timeBeginPeriod(1);
        }
    }

    /// Release the resolution requested by `begin_high_res_timer`.
    pub fn end_high_res_timer() {
        unsafe {
            windows::Win32::Media::timeEndPeriod(1);
        }
    }

    /// Show or hide the app's own console window (F12 minimize-to-nothing).
    pub fn set_console_visible(visible: bool) {
        use windows::Win32::System::Console::GetConsoleWindow;
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOW};
        unsafe {
            let hwnd = GetConsoleWindow();
            if !hwnd.is_invalid() {
                let _ = ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
            }
        }
    }

    /// Visit every visible top-level window with its owning exe name
    /// (lowercase); return false from `f` to stop enumerating.
    ///
    /// EnumWindows invokes the callback synchronously on the calling thread,
    /// so a plain closure reference through LPARAM is sound — no locking
    /// needed.
    fn for_each_visible_window(f: &mut dyn FnMut(windows::Win32::Foundation::HWND, &str) -> bool) {
        use windows::Win32::Foundation::{HWND, LPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
        };

        type Visitor<'a> = &'a mut dyn FnMut(HWND, &str) -> bool;

        unsafe extern "system" fn callback(
            hwnd: HWND,
            lparam: LPARAM,
        ) -> windows::core::BOOL {
            unsafe {
                use windows::core::BOOL;
                let cont = BOOL(1);

                if !IsWindowVisible(hwnd).as_bool() {
                    return cont;
                }

                let mut pid: u32 = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                if pid == 0 {
                    return cont;
                }

                let Some(name) = super::platform::exe_name_from_pid(pid) else {
                    return cont;
                };

                let f = &mut *(lparam.0 as *mut Visitor);
                BOOL(f(hwnd, &name) as i32)
            }
        }

        let mut visitor: Visitor = f;
        unsafe {
            let ptr = &mut visitor as *mut Visitor;
            let _ = EnumWindows(Some(callback), LPARAM(ptr as isize));
        }
    }

    /// Find the main window HWND of a process by its exe name.
    /// Returns the first visible window belonging to that process.
    pub fn find_window_by_process(exe_name: &str) -> Option<windows::Win32::Foundation::HWND> {
        let target = exe_name.to_lowercase();
        let mut found = None;
        for_each_visible_window(&mut |hwnd, name| {
            if name == target {
                found = Some(hwnd);
                false
            } else {
                true
            }
        });
        found
    }

    /// Send a key event to a specific window via PostMessage (no focus needed).
    pub fn post_key(hwnd: windows::Win32::Foundation::HWND, scancode: u16, up: bool) -> bool {
        use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyA, MAP_VIRTUAL_KEY_TYPE};
        use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
        use windows::Win32::Foundation::{WPARAM, LPARAM};

        use windows::Win32::UI::Input::KeyboardAndMouse::MAPVK_VSC_TO_VK_EX;
        use windows::Win32::UI::WindowsAndMessaging::{WM_KEYDOWN, WM_KEYUP};

        let (sc, extended) = super::split_scancode(scancode);
        unsafe {
            // MAPVK_VSC_TO_VK_EX resolves the prefixed form, so an extended
            // scancode maps to VK_UP rather than to its numpad twin.
            let lookup = if extended { super::EXTENDED_PREFIX | sc } else { sc };
            let vk =
                MapVirtualKeyA(lookup as u32, MAP_VIRTUAL_KEY_TYPE(MAPVK_VSC_TO_VK_EX.0)) as usize;
            // lParam: repeat count 1, scancode, bit 24 for an extended key,
            // and for key-up the previous-state and transition bits (30, 31).
            let extended_bit = if extended { 1u32 << 24 } else { 0 };
            let base = 1u32 | ((sc as u32) << 16) | extended_bit;
            let (msg, lparam) = if up {
                (WM_KEYUP, base | 0xC000_0000)
            } else {
                (WM_KEYDOWN, base)
            };
            PostMessageW(Some(hwnd), msg, WPARAM(vk), LPARAM(lparam as isize)).is_ok()
        }
    }

    /// Send a mouse button event to a specific window via PostMessage.
    pub fn post_mouse(hwnd: windows::Win32::Foundation::HWND, button: MouseButton, up: bool) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
        use windows::Win32::Foundation::{WPARAM, LPARAM};

        use windows::Win32::UI::WindowsAndMessaging::{
            WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_RBUTTONDOWN,
            WM_RBUTTONUP,
        };

        let msg = match (button, up) {
            (MouseButton::Left, false) => WM_LBUTTONDOWN,
            (MouseButton::Left, true) => WM_LBUTTONUP,
            (MouseButton::Right, false) => WM_RBUTTONDOWN,
            (MouseButton::Right, true) => WM_RBUTTONUP,
            (MouseButton::Middle, false) => WM_MBUTTONDOWN,
            (MouseButton::Middle, true) => WM_MBUTTONUP,
        };
        unsafe { PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0)).is_ok() }
    }

    /// Returns (total_time, idle_time) as raw ticks for CPU usage calculation.
    pub fn get_cpu_usage_snapshot() -> (u64, u64) {
        use windows::Win32::System::Threading::GetSystemTimes;
        use windows::Win32::Foundation::FILETIME;

        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        unsafe {
            if GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_err() {
                return (0, 0);
            }
        }
        let ft_to_u64 = |ft: FILETIME| (ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64;
        let total = ft_to_u64(kernel) + ft_to_u64(user);
        let idle_val = ft_to_u64(idle);
        (total, idle_val)
    }

    /// Returns (used_mb, total_mb) of physical RAM.
    pub fn get_ram_usage() -> (u32, u32) {
        use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

        let mut mem = MEMORYSTATUSEX::default();
        mem.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        unsafe {
            if GlobalMemoryStatusEx(&mut mem).is_err() {
                return (0, 0);
            }
        }
        let total_mb = (mem.ullTotalPhys / (1024 * 1024)) as u32;
        let avail_mb = (mem.ullAvailPhys / (1024 * 1024)) as u32;
        (total_mb.saturating_sub(avail_mb), total_mb)
    }

    /// Returns the executable name (e.g. "notepad.exe") of the foreground
    /// window, or None if it can't be determined.
    pub fn get_foreground_process_name() -> Option<String> {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId,
        };

        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_invalid() {
                return None;
            }
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return None;
            }
            exe_name_from_pid(pid)
        }
    }

    /// Returns a list of (exe_name, window_title) for all visible windows with titles.
    pub fn get_window_list() -> Vec<(String, String)> {
        use std::collections::HashSet;
        use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};

        let mut list: Vec<(String, String)> = Vec::new();
        for_each_visible_window(&mut |hwnd, exe_name| {
            let len = unsafe { GetWindowTextLengthW(hwnd) };
            if len == 0 {
                return true;
            }
            let mut title_buf = vec![0u16; (len + 1) as usize];
            // Cut with what GetWindowTextW actually wrote, not the earlier
            // length: a title that changes between the two calls otherwise
            // leaves padding zeros inside the string.
            let written = unsafe { GetWindowTextW(hwnd, &mut title_buf) };
            let title = String::from_utf16_lossy(&title_buf[..written.max(0) as usize]);
            if !title.is_empty() {
                list.push((exe_name.to_string(), title));
            }
            true
        });

        let mut seen = HashSet::new();
        list.retain(|(exe, _)| seen.insert(exe.clone()));
        list.sort_by(|a, b| a.0.cmp(&b.0));
        list
    }

}

#[cfg(not(windows))]
mod platform {
    use crate::macro_def::MouseButton;

    pub fn is_key_pressed(_vk: i32) -> bool {
        false
    }
    pub fn vk_to_scancode(_vk: i32) -> u16 {
        0
    }
    pub fn send_key(_scancode: u16, _up: bool) -> bool { true }
    pub fn send_mouse(_button: MouseButton, _up: bool) -> bool { true }
    pub fn send_mouse_scroll(_up: bool, _clicks: u32) -> bool { true }
    pub fn send_mouse_move(_x: i32, _y: i32) -> bool { true }
    pub fn get_cursor_pos() -> Option<(i32, i32)> { None }
    pub fn begin_high_res_timer() {}
    pub fn end_high_res_timer() {}
    pub fn set_console_visible(_visible: bool) {}

    pub fn get_cpu_usage_snapshot() -> (u64, u64) { (0, 0) }
    pub fn get_ram_usage() -> (u32, u32) { (0, 0) }

    pub fn get_foreground_process_name() -> Option<String> {
        None
    }

    pub fn get_window_list() -> Vec<(String, String)> {
        Vec::new()
    }

}

pub use platform::*;

/// Calculate CPU usage percentage from two snapshots.
/// Call `get_cpu_usage_snapshot()` twice with a delay between them.
pub fn calc_cpu_percent(prev: (u64, u64), curr: (u64, u64)) -> u32 {
    let total_diff = curr.0.saturating_sub(prev.0);
    let idle_diff = curr.1.saturating_sub(prev.1);
    if total_diff == 0 { return 0; }
    let busy = total_diff.saturating_sub(idle_diff);
    (busy.saturating_mul(100) / total_diff) as u32
}

#[cfg(windows)]
mod scroll_hook {
    use std::sync::mpsc;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::time::Instant;

    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
        MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_MOUSEWHEEL,
    };

    use crate::macro_def::{KeyAction, ScrollDirection};
    use crate::state::AppEvent;

    static HOOK_THREAD_ID: AtomicIsize = AtomicIsize::new(0);

    // Thread-local sender for the hook callback
    thread_local! {
        static SCROLL_TX: std::cell::RefCell<Option<mpsc::Sender<AppEvent>>> = const { std::cell::RefCell::new(None) };
    }

    unsafe extern "system" fn mouse_hook_proc(
        n_code: i32,
        w_param: WPARAM,
        l_param: LPARAM,
    ) -> LRESULT {
        unsafe {
            if n_code >= 0 && w_param.0 == WM_MOUSEWHEEL as usize {
                let info = &*(l_param.0 as *const MSLLHOOKSTRUCT);
                // mouseData high word = wheel delta (positive=up, negative=down)
                let delta = (info.mouseData >> 16) as i16;
                if delta != 0 {
                    let dir = if delta > 0 { ScrollDirection::Up } else { ScrollDirection::Down };
                    let clicks = (delta.unsigned_abs() / crate::input::WHEEL_DELTA as u16).max(1) as u32;
                    SCROLL_TX.with(|tx| {
                        if let Some(tx) = tx.borrow().as_ref() {
                            let _ = tx.send(AppEvent::RecordKey(
                                KeyAction::MouseScroll(dir, clicks),
                                Instant::now(),
                            ));
                        }
                    });
                }
            }
            CallNextHookEx(None, n_code, w_param, l_param)
        }
    }

    /// Install the low-level scroll hook on its own message-pump thread.
    ///
    /// Blocks until the hook is installed and the thread id published: a
    /// `stop()` racing the spawn would otherwise find no target, leaking the
    /// hook and its thread for the rest of the process. Calling `start` twice
    /// is safe; the previous hook is torn down first.
    pub fn start(tx: mpsc::Sender<AppEvent>) {
        use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, PM_NOREMOVE};

        // Idempotent: a second start without a stop would strand the first
        // thread with its hook installed for the rest of the process, and each
        // orphaned WH_MOUSE_LL taxes every mouse event system-wide.
        stop();

        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        let spawned = std::thread::Builder::new()
            .name("scroll-hook".into())
            .spawn(move || {
                unsafe {
                    let warn_tx = tx.clone();
                    SCROLL_TX.with(|cell| {
                        *cell.borrow_mut() = Some(tx);
                    });

                    // The first PeekMessageW creates this thread's message
                    // queue; only then is the published id postable.
                    let mut msg = std::mem::zeroed();
                    let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
                    let tid = windows::Win32::System::Threading::GetCurrentThreadId();

                    let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), None, 0);

                    // Publish the id only once the hook is installed, and only
                    // on success. Releasing the caller earlier leaves a window
                    // where stop() posts WM_QUIT to a thread that has not yet
                    // installed its hook, and storing 0 on the failure path
                    // could clear an id a newer thread had already claimed.
                    if hook.is_ok() {
                        HOOK_THREAD_ID.store(tid as isize, Ordering::SeqCst);
                    }
                    let _ = ready_tx.send(());

                    let Ok(hook) = hook else {
                        // A panic here would only kill this detached thread,
                        // silently losing scroll capture for the recording.
                        let _ = warn_tx.send(AppEvent::Warning(
                            "Scroll capture unavailable: mouse hook failed".into(),
                        ));
                        return;
                    };

                    // Message pump (required for low-level hooks); -1 means
                    // error and must not keep the loop spinning.
                    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {}

                    let _ = UnhookWindowsHookEx(hook);
                }
            });
        if spawned.is_ok() {
            let _ = ready_rx.recv();
        }
    }

    pub fn stop() {
        let tid = HOOK_THREAD_ID.swap(0, Ordering::SeqCst);
        if tid != 0 {
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
                let _ = PostThreadMessageW(tid as u32, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

#[cfg(not(windows))]
mod scroll_hook {
    use std::sync::mpsc;
    use crate::state::AppEvent;

    pub fn start(_tx: mpsc::Sender<AppEvent>) {}
    pub fn stop() {}
}

pub use scroll_hook::start as start_scroll_hook;
pub use scroll_hook::stop as stop_scroll_hook;

/// Advance the xorshift64 state and return the new value.
///
/// `state` must be nonzero: zero is xorshift's fixed point.
pub fn xorshift64(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Execute a single KeyAction globally, reporting whether injection was
/// accepted (see `platform::send`).
pub fn execute_action(action: &KeyAction, stop: &std::sync::atomic::AtomicBool) -> bool {
    use crate::macro_def::ScrollDirection;
    match action {
        KeyAction::KeyDown(sc) => send_key(*sc, false),
        KeyAction::KeyUp(sc) => send_key(*sc, true),
        KeyAction::MouseDown(b) => send_mouse(*b, false),
        KeyAction::MouseUp(b) => send_mouse(*b, true),
        KeyAction::MouseScroll(dir, clicks) => send_mouse_scroll(*dir == ScrollDirection::Up, *clicks),
        KeyAction::MouseMove(x, y) => send_mouse_move(*x, *y),
        KeyAction::TypeText(text) => type_text(text, stop),
        KeyAction::WaitForWindow(_) => true, // handled by executor
    }
}

/// Execute an action, offsetting a MouseMove target by up to ±`jitter` px;
/// any other action passes through to `execute_action` unchanged.
pub fn execute_action_with_jitter(
    action: &KeyAction,
    jitter: u32,
    rng: &mut u64,
    stop: &std::sync::atomic::AtomicBool,
) -> bool {
    if jitter > 0
        && let KeyAction::MouseMove(x, y) = action {
            let jitter_i = jitter as i32;
            let dx = (xorshift64(rng) as i32 % (jitter_i * 2 + 1)) - jitter_i;
            let dy = (xorshift64(rng) as i32 % (jitter_i * 2 + 1)) - jitter_i;
            return send_mouse_move(x + dx, y + dy);
        }
    execute_action(action, stop)
}

/// Type a string by sending key events for each character.
///
/// Checks `stop` between characters: a full-length text takes ~50 s, and an
/// uninterruptible loop here blocks the executor's stop/join for that long.
#[cfg(windows)]
fn type_text(text: &str, stop: &std::sync::atomic::AtomicBool) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{KEYEVENTF_KEYUP, KEYEVENTF_UNICODE};

    let mut all_sent = true;
    for ch in text.encode_utf16() {
        if stop.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        let down = platform::keyboard_input(ch, KEYEVENTF_UNICODE);
        let up = platform::keyboard_input(ch, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
        all_sent &= platform::send(&[down, up]);
        std::thread::sleep(std::time::Duration::from_millis(TYPE_TEXT_CHAR_DELAY_MS));
    }
    all_sent
}

#[cfg(windows)]
const TYPE_TEXT_CHAR_DELAY_MS: u64 = 5;

#[cfg(not(windows))]
fn type_text(_text: &str, _stop: &std::sync::atomic::AtomicBool) -> bool {
    true
}

/// A window handle resolved once and reused across a macro's steps.
///
/// Resolving costs a full EnumWindows sweep with an OpenProcess per visible
/// window, so doing it per step made it the hottest path in the program.
#[cfg(windows)]
#[derive(Default)]
pub struct WindowTarget {
    hwnd: Option<windows::Win32::Foundation::HWND>,
    /// Windows recycles HWND values, so `IsWindow` alone can validate a
    /// handle that now belongs to a different program; ownership is
    /// re-verified on a coarse interval.
    last_identity_check: Option<std::time::Instant>,
}

#[cfg(not(windows))]
#[derive(Default)]
pub struct WindowTarget {}

impl WindowTarget {

    /// Drop the cached handle so the next send re-resolves it.
    pub fn invalidate(&mut self) {
        #[cfg(windows)]
        {
            self.hwnd = None;
            self.last_identity_check = None;
        }
    }
}

#[cfg(windows)]
const IDENTITY_RECHECK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

#[cfg(windows)]
fn window_belongs_to_process(hwnd: windows::Win32::Foundation::HWND, process: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid != 0
        && platform::exe_name_from_pid(pid).is_some_and(|name| name == process.to_lowercase())
}

/// Execute a single KeyAction via PostMessage to a specific process window.
///
/// Reuses the handle cached in `target`, resolving it only on the first call
/// or after the window went away. Returns false if the window was not found.
pub fn execute_action_to_process(
    action: &KeyAction,
    process: &str,
    target: &mut WindowTarget,
    stop: &std::sync::atomic::AtomicBool,
) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::IsWindow;

        // A cached handle goes stale when the window closes; IsWindow is a
        // cheap check next to a full enumeration.
        if let Some(hwnd) = target.hwnd
            && !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                target.hwnd = None;
            }

        if let Some(hwnd) = target.hwnd
            && target
                .last_identity_check
                .is_none_or(|at| at.elapsed() >= IDENTITY_RECHECK_INTERVAL)
        {
            if window_belongs_to_process(hwnd, process) {
                target.last_identity_check = Some(std::time::Instant::now());
            } else {
                target.hwnd = None;
            }
        }

        let hwnd = match target.hwnd {
            Some(hwnd) => hwnd,
            None => {
                let Some(found) = find_window_by_process(process) else {
                    return false;
                };
                target.hwnd = Some(found);
                target.last_identity_check = Some(std::time::Instant::now());
                found
            }
        };

        match action {
            KeyAction::KeyDown(sc) => post_key(hwnd, *sc, false),
            KeyAction::KeyUp(sc) => post_key(hwnd, *sc, true),
            KeyAction::MouseDown(b) => post_mouse(hwnd, *b, false),
            KeyAction::MouseUp(b) => post_mouse(hwnd, *b, true),
            // Scroll and move use global SendInput even in Window mode
            // (PostMessage doesn't support WM_MOUSEWHEEL/WM_MOUSEMOVE reliably)
            KeyAction::MouseScroll(dir, clicks) => {
                use crate::macro_def::ScrollDirection;
                send_mouse_scroll(*dir == ScrollDirection::Up, *clicks)
            }
            KeyAction::MouseMove(x, y) => send_mouse_move(*x, *y),
            KeyAction::WaitForWindow(_) => true, // handled by executor
            KeyAction::TypeText(text) => {
                use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CHAR};
                use windows::Win32::Foundation::{WPARAM, LPARAM};
                let mut all_posted = true;
                for ch in text.encode_utf16() {
                    if stop.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    let posted = unsafe {
                        PostMessageW(Some(hwnd), WM_CHAR, WPARAM(ch as usize), LPARAM(0)).is_ok()
                    };
                    all_posted &= posted;
                    std::thread::sleep(std::time::Duration::from_millis(TYPE_TEXT_CHAR_DELAY_MS));
                }
                all_posted
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (action, process, target, stop);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs only on the Windows CI leg — the half these APIs live in.
    #[cfg(windows)]
    #[test]
    fn common_vks_map_to_nonzero_scancodes() {
        // A wrong map type here means every recorded key silently drops
        // (the recorder skips scancode 0).
        for vk in [0x41, 0x30, 0x20, 0x0D] {
            assert_ne!(vk_to_scancode(vk), 0, "vk {vk:#x} must map");
        }
    }

    #[cfg(windows)]
    #[test]
    fn window_target_starts_unresolved() {
        let mut target = WindowTarget::default();
        target.invalidate();
        assert!(target.hwnd.is_none());
    }

    #[test]
    fn split_scancode_separates_the_extended_prefix() {
        assert_eq!(split_scancode(0x1E), (0x1E, false), "plain A");
        assert_eq!(split_scancode(0xE048), (0x48, true), "arrow up");
        assert_eq!(split_scancode(0x48), (0x48, false), "numpad 8");
    }

    #[test]
    fn extended_and_numpad_twins_stay_distinguishable() {
        // The whole point of keeping the prefix: these two share a low byte
        // and must not collapse onto one another.
        let (up, up_ext) = split_scancode(0xE048);
        let (num8, num8_ext) = split_scancode(0x48);
        assert_eq!(up, num8);
        assert!(up_ext && !num8_ext);
    }

    #[test]
    fn cpu_percent_half_busy() {
        assert_eq!(calc_cpu_percent((0, 0), (100, 50)), 50);
    }

    #[test]
    fn cpu_percent_fully_busy_and_fully_idle() {
        assert_eq!(calc_cpu_percent((0, 0), (100, 0)), 100);
        assert_eq!(calc_cpu_percent((0, 0), (100, 100)), 0);
    }

    #[test]
    fn cpu_percent_identical_snapshots() {
        assert_eq!(calc_cpu_percent((500, 250), (500, 250)), 0);
    }

    #[test]
    fn cpu_percent_idle_exceeding_total_does_not_underflow() {
        // Windows counters make this unreachable (kernel time includes idle),
        // but the bare subtraction would underflow if it ever happened.
        assert_eq!(calc_cpu_percent((1000, 800), (1100, 1000)), 0);
    }
}
