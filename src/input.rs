use crate::macro_def::KeyAction;

// ── Windows API wrappers ──
// These are only compiled on Windows. On other platforms we provide stubs
// so the project can still be type-checked.

#[cfg(windows)]
mod platform {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, MapVirtualKeyA, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE,
        KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MAP_VIRTUAL_KEY_TYPE,
        MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
        MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL,
        MOUSEINPUT, VIRTUAL_KEY,
    };

    use crate::macro_def::MouseButton;

    pub fn is_key_pressed(vk: i32) -> bool {
        let state = unsafe { GetAsyncKeyState(vk) };
        (state as u16 & 0x8000) != 0
    }

    pub fn vk_to_scancode(vk: i32) -> u16 {
        unsafe { MapVirtualKeyA(vk as u32, MAP_VIRTUAL_KEY_TYPE(0)) as u16 }
    }

    pub fn send_key(scancode: u16, up: bool) {
        let flags = if up { KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP } else { KEYEVENTF_SCANCODE };
        let input = INPUT {
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
        };
        unsafe {
            SendInput(&[input], size_of::<INPUT>() as i32);
        }
    }

    pub fn send_mouse(button: MouseButton, up: bool) {
        let flags = match (button, up) {
            (MouseButton::Left, false) => MOUSEEVENTF_LEFTDOWN,
            (MouseButton::Left, true) => MOUSEEVENTF_LEFTUP,
            (MouseButton::Right, false) => MOUSEEVENTF_RIGHTDOWN,
            (MouseButton::Right, true) => MOUSEEVENTF_RIGHTUP,
            (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEDOWN,
            (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEUP,
        };
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe {
            SendInput(&[input], size_of::<INPUT>() as i32);
        }
    }

    pub fn send_mouse_scroll(up: bool, clicks: u32) {
        // WHEEL_DELTA = 120; positive = scroll up, negative = scroll down
        let delta = if up { 120i32 * clicks as i32 } else { -(120i32 * clicks as i32) };
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: delta as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe {
            SendInput(&[input], size_of::<INPUT>() as i32);
        }
    }

    pub fn send_mouse_move(x: i32, y: i32) {
        // Convert pixel coords to absolute (0..65535) normalized coords
        let (sx, sy) = unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
            (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
        };
        if sx == 0 || sy == 0 { return; }
        let abs_x = ((x as i64 * 65536) / sx as i64) as i32;
        let abs_y = ((y as i64 * 65536) / sy as i64) as i32;
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: abs_x,
                    dy: abs_y,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe {
            SendInput(&[input], size_of::<INPUT>() as i32);
        }
    }

    pub fn get_cursor_pos() -> (i32, i32) {
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        use windows::Win32::Foundation::POINT;
        let mut pt = POINT { x: 0, y: 0 };
        unsafe { let _ = GetCursorPos(&mut pt); }
        (pt.x, pt.y)
    }

    /// Get the exe name (lowercase) of a process by PID.
    fn exe_name_from_pid(pid: u32) -> Option<String> {
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

    pub fn begin_high_res_timer() {
        unsafe {
            windows::Win32::Media::timeBeginPeriod(1);
        }
    }

    pub fn end_high_res_timer() {
        unsafe {
            windows::Win32::Media::timeEndPeriod(1);
        }
    }

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

    /// Find the main window HWND of a process by its exe name.
    /// Returns the first visible window belonging to that process.
    pub fn find_window_by_process(exe_name: &str) -> Option<windows::Win32::Foundation::HWND> {
        use std::sync::Mutex as StdMutex;
        use windows::Win32::Foundation::{HWND, LPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
        };

        let target = exe_name.to_lowercase();

        unsafe extern "system" fn callback(
            hwnd: HWND,
            lparam: LPARAM,
        ) -> windows::core::BOOL {
            unsafe {
                use windows::core::BOOL;
                let cont = BOOL(1);
                let stop = BOOL(0);

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

                let data = &*(lparam.0 as *const (String, StdMutex<Option<HWND>>));
                if name == data.0 {
                    if let Ok(mut r) = data.1.lock() {
                        *r = Some(hwnd);
                    }
                    return stop;
                }
                cont
            }
        }

        let data = (target, StdMutex::new(None::<HWND>));
        unsafe {
            let ptr = &data as *const (String, StdMutex<Option<HWND>>);
            let _ = EnumWindows(Some(callback), LPARAM(ptr as isize));
        }
        data.1.into_inner().unwrap_or(None)
    }

    /// Send a key event to a specific window via PostMessage (no focus needed).
    pub fn post_key(hwnd: windows::Win32::Foundation::HWND, scancode: u16, up: bool) {
        use windows::Win32::UI::Input::KeyboardAndMouse::{MapVirtualKeyA, MAP_VIRTUAL_KEY_TYPE};
        use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
        use windows::Win32::Foundation::{WPARAM, LPARAM};

        unsafe {
            let vk = MapVirtualKeyA(scancode as u32, MAP_VIRTUAL_KEY_TYPE(3)) as usize;
            let (msg, lparam) = if up {
                (0x0101u32, 1u32 | ((scancode as u32) << 16) | 0xC000_0000)  // WM_KEYUP
            } else {
                (0x0100u32, 1u32 | ((scancode as u32) << 16))                // WM_KEYDOWN
            };
            let _ = PostMessageW(Some(hwnd), msg, WPARAM(vk), LPARAM(lparam as isize));
        }
    }

    /// Send a mouse button event to a specific window via PostMessage.
    pub fn post_mouse(hwnd: windows::Win32::Foundation::HWND, button: MouseButton, up: bool) {
        use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
        use windows::Win32::Foundation::{WPARAM, LPARAM};

        let msg = match (button, up) {
            (MouseButton::Left, false) => 0x0201u32,   // WM_LBUTTONDOWN
            (MouseButton::Left, true) => 0x0202u32,    // WM_LBUTTONUP
            (MouseButton::Right, false) => 0x0204u32,  // WM_RBUTTONDOWN
            (MouseButton::Right, true) => 0x0205u32,   // WM_RBUTTONUP
            (MouseButton::Middle, false) => 0x0207u32, // WM_MBUTTONDOWN
            (MouseButton::Middle, true) => 0x0208u32,  // WM_MBUTTONUP
        };
        unsafe {
            let _ = PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0));
        }
    }

    /// Returns the executable name (e.g. "notepad.exe") of the foreground window,
    /// or None if it can't be determined.
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
        use std::sync::Mutex as StdMutex;
        use windows::Win32::Foundation::{HWND, LPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
            IsWindowVisible,
        };

        let results: StdMutex<Vec<(String, String)>> = StdMutex::new(Vec::new());

        unsafe extern "system" fn enum_callback(
            hwnd: HWND,
            lparam: LPARAM,
        ) -> windows::core::BOOL {
            unsafe {
                use windows::core::BOOL;
                let cont = BOOL(1);
                if !IsWindowVisible(hwnd).as_bool() {
                    return cont;
                }
                let len = GetWindowTextLengthW(hwnd);
                if len == 0 {
                    return cont;
                }
                let mut title_buf = vec![0u16; (len + 1) as usize];
                GetWindowTextW(hwnd, &mut title_buf);
                let title = String::from_utf16_lossy(&title_buf[..len as usize]);
                if title.is_empty() {
                    return cont;
                }

                let mut pid: u32 = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                if pid == 0 {
                    return cont;
                }

                let Some(exe_name) = super::platform::exe_name_from_pid(pid) else {
                    return cont;
                };

                let list = &*(lparam.0 as *const StdMutex<Vec<(String, String)>>);
                if let Ok(mut v) = list.lock() {
                    v.push((exe_name, title));
                }
                cont
            }
        }

        unsafe {
            let ptr = &results as *const StdMutex<Vec<(String, String)>>;
            let _ = EnumWindows(Some(enum_callback), LPARAM(ptr as isize));
        }

        let mut list = results.into_inner().unwrap_or_default();
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
    pub fn send_key(_scancode: u16, _up: bool) {}
    pub fn send_mouse(_button: MouseButton, _up: bool) {}
    pub fn send_mouse_scroll(_up: bool, _clicks: u32) {}
    pub fn send_mouse_move(_x: i32, _y: i32) {}
    pub fn get_cursor_pos() -> (i32, i32) { (0, 0) }
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

// Re-export platform functions
pub use platform::*;

/// Calculate CPU usage percentage from two snapshots.
/// Call `get_cpu_usage_snapshot()` twice with a delay between them.
pub fn calc_cpu_percent(prev: (u64, u64), curr: (u64, u64)) -> u32 {
    let total_diff = curr.0.saturating_sub(prev.0);
    let idle_diff = curr.1.saturating_sub(prev.1);
    if total_diff == 0 { return 0; }
    (((total_diff - idle_diff) * 100) / total_diff) as u32
}

// ── Scroll recording hook ──

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
                    let clicks = (delta.unsigned_abs() / 120).max(1) as u32;
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

    pub fn start(tx: mpsc::Sender<AppEvent>) {
        std::thread::Builder::new()
            .name("scroll-hook".into())
            .spawn(move || {
                unsafe {
                    SCROLL_TX.with(|cell| {
                        *cell.borrow_mut() = Some(tx);
                    });

                    let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), None, 0)
                        .expect("failed to install mouse hook");

                    // Store thread ID so we can post WM_QUIT to stop
                    let tid = windows::Win32::System::Threading::GetCurrentThreadId();
                    HOOK_THREAD_ID.store(tid as isize, Ordering::SeqCst);

                    // Message pump (required for low-level hooks)
                    let mut msg = std::mem::zeroed();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {}

                    let _ = UnhookWindowsHookEx(hook);
                }
            })
            .expect("failed to spawn scroll hook thread");
    }

    pub fn stop() {
        let tid = HOOK_THREAD_ID.swap(0, Ordering::SeqCst);
        if tid != 0 {
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
                // WM_QUIT = 0x0012
                let _ = PostThreadMessageW(tid as u32, 0x0012, WPARAM(0), LPARAM(0));
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

/// Execute a single KeyAction via SendInput (global, needs focus).
pub fn execute_action(action: &KeyAction) {
    use crate::macro_def::ScrollDirection;
    match action {
        KeyAction::KeyDown(sc) => send_key(*sc, false),
        KeyAction::KeyUp(sc) => send_key(*sc, true),
        KeyAction::MouseDown(b) => send_mouse(*b, false),
        KeyAction::MouseUp(b) => send_mouse(*b, true),
        KeyAction::MouseScroll(dir, clicks) => send_mouse_scroll(*dir == ScrollDirection::Up, *clicks),
        KeyAction::MouseMove(x, y) => send_mouse_move(*x, *y),
        KeyAction::TypeText(text) => type_text(text),
        KeyAction::WaitForWindow(_) => {} // handled by executor
    }
}

/// Execute a MouseMove with jitter applied.
pub fn execute_action_with_jitter(action: &KeyAction, jitter: u32, rng: &mut u64) {
    if jitter > 0 {
        if let KeyAction::MouseMove(x, y) = action {
            // xorshift64 for offset
            *rng ^= *rng << 13;
            *rng ^= *rng >> 7;
            *rng ^= *rng << 17;
            let jitter_i = jitter as i32;
            let dx = (*rng as i32 % (jitter_i * 2 + 1)) - jitter_i;
            *rng ^= *rng << 13;
            *rng ^= *rng >> 7;
            *rng ^= *rng << 17;
            let dy = (*rng as i32 % (jitter_i * 2 + 1)) - jitter_i;
            send_mouse_move(x + dx, y + dy);
            return;
        }
    }
    execute_action(action);
}

/// Type a string by sending key events for each character.
#[cfg(windows)]
fn type_text(text: &str) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, VIRTUAL_KEY,
    };

    for ch in text.encode_utf16() {
        let down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: ch,
                    dwFlags: KEYEVENTF_UNICODE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let up = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: ch,
                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe {
            SendInput(&[down, up], size_of::<INPUT>() as i32);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[cfg(not(windows))]
fn type_text(_text: &str) {}

/// Execute a single KeyAction via PostMessage to a specific process window.
/// Returns false if the window was not found.
pub fn execute_action_to_process(action: &KeyAction, process: &str) -> bool {
    #[cfg(windows)]
    {
        let Some(hwnd) = find_window_by_process(process) else {
            return false;
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
                send_mouse_scroll(*dir == ScrollDirection::Up, *clicks);
            }
            KeyAction::MouseMove(x, y) => send_mouse_move(*x, *y),
            KeyAction::WaitForWindow(_) => {} // handled by executor
            KeyAction::TypeText(text) => {
                use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
                use windows::Win32::Foundation::{WPARAM, LPARAM};
                for ch in text.encode_utf16() {
                    unsafe {
                        let _ = PostMessageW(Some(hwnd), 0x0102, WPARAM(ch as usize), LPARAM(0));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
        true
    }
    #[cfg(not(windows))]
    {
        let _ = (action, process);
        false
    }
}
