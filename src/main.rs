use std::{
    mem::size_of,
    process::Command,
    sync::{Mutex, OnceLock},
};
use windows::Win32::{
    Foundation::{BOOL, CloseHandle, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, WPARAM},
    System::{
        ProcessStatus::K32GetModuleBaseNameW,
        Threading::{
            AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_QUERY_INFORMATION,
            PROCESS_VM_READ,
        },
    },
    UI::{
        Input::KeyboardAndMouse::{
            GetKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
            KEYEVENTF_KEYUP, SendInput, VK_CAPITAL,
        },
        WindowsAndMessaging::{
            BringWindowToTop, CallNextHookEx, EnumWindows, GWL_STYLE, GetForegroundWindow,
            GetMessageW, GetWindowLongW, GetWindowTextLengthW, GetWindowThreadProcessId, HC_ACTION,
            HHOOK, IsIconic, KBDLLHOOKSTRUCT, KBDLLHOOKSTRUCT_FLAGS, LLKHF_INJECTED, MSG,
            SW_RESTORE, SW_SHOW, SetForegroundWindow, SetWindowsHookExW, ShowWindow,
            UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
            WS_VISIBLE,
        },
    },
};

const DISCORD_EXE: &str = "Discord.exe";
const DISCORD_PATH: &str =
    r"%USERPROFILE%\AppData\Local\Discord\Update.exe --processStart Discord.exe";
const MY_EXTRA_INFO: usize = 0xDEADBEEF;

const VK_D: u32 = 0x44;

#[derive(Default)]
struct CapsState {
    down: bool,
    used_as_modifier: bool,
    initial_state: bool,
}

static CAPS_STATE: OnceLock<Mutex<CapsState>> = OnceLock::new();

fn caps_state() -> &'static Mutex<CapsState> {
    CAPS_STATE.get_or_init(|| Mutex::new(CapsState::default()))
}

fn main() {
    let _ = caps_state();

    let hook = unsafe {
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), HINSTANCE(0), 0)
            .expect("Failed to install hook")
    };

    println!("Listening for Caps combos…");

    let mut msg = MSG::default();

    while unsafe { GetMessageW(&mut msg, HWND(0), 0, 0) }.into() {}

    let _ = unsafe { UnhookWindowsHookEx(hook) };
}

fn activate_or_run(exe: &str, path: Option<&str>) {
    if let Some(hwnd) = find_window_by_process(exe) {
        bring_to_foreground(hwnd);
    } else if let Some(path) = path {
        launch_path(path);
    } else {
        if let Err(e) = Command::new(exe).spawn() {
            eprintln!("Failed to launch {exe}: {e}");
        }
    }
}

fn bring_to_foreground(hwnd: HWND) {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }

        let cur_tid = GetCurrentThreadId();
        let target_tid = GetWindowThreadProcessId(hwnd, None);
        let fg = GetForegroundWindow();
        let fg_tid = if fg.0 != 0 {
            GetWindowThreadProcessId(fg, None)
        } else {
            target_tid
        };

        if fg_tid != cur_tid {
            AttachThreadInput(cur_tid, fg_tid, true);
        }
        if target_tid != cur_tid && target_tid != fg_tid {
            AttachThreadInput(cur_tid, target_tid, true);
        }

        let _ = SetForegroundWindow(hwnd);
        let _ = BringWindowToTop(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
}

fn launch_path(path: &str) {
    let expanded = match expand_userprofile(path) {
        Some(s) => s,
        None => return,
    };

    let mut parts = expanded.split_whitespace();
    let Some(program) = parts.next() else {
        eprintln!("Empty command line");
        return;
    };

    let args: Vec<&str> = parts.collect();
    if let Err(e) = Command::new(program).args(&args).spawn() {
        eprintln!("Failed to launch '{program}': {e}");
    }
}

fn expand_userprofile(path: &str) -> Option<String> {
    if path.contains("%USERPROFILE%") {
        match std::env::var("USERPROFILE") {
            Ok(profile) => Some(path.replace("%USERPROFILE%", &profile)),
            Err(_) => {
                eprintln!("USERPROFILE environment variable not found");
                None
            }
        }
    } else {
        Some(path.to_string())
    }
}

fn find_window_by_process(target_exe: &str) -> Option<HWND> {
    struct SearchState {
        target_exe: String,
        best: Option<HWND>,
        fallback: Option<HWND>,
    }

    let mut state = SearchState {
        target_exe: target_exe.to_string(),
        best: None,
        fallback: None,
    };

    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(&mut state as *mut SearchState as isize),
        );
    }

    return state.best.or(state.fallback);

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = unsafe { &mut *(lparam.0 as *mut SearchState) };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };

        let handle =
            match unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) } {
                Ok(h) => h,
                Err(_) => return BOOL(1),
            };

        let mut buffer = [0u16; 260];
        let len = unsafe { K32GetModuleBaseNameW(handle, HMODULE(0), &mut buffer) };
        let _ = unsafe { CloseHandle(handle) };

        if len > 0 {
            let name = String::from_utf16_lossy(&buffer[..len as usize]);
            if name.eq_ignore_ascii_case(&state.target_exe) {
                let has_title = unsafe { GetWindowTextLengthW(hwnd) } > 0;
                let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
                let has_visible_style = (style & WS_VISIBLE.0) != 0;

                if has_title && has_visible_style {
                    state.best = Some(hwnd);
                    return BOOL(0);
                } else if has_title {
                    if state.fallback.is_none() {
                        state.fallback = Some(hwnd);
                    }
                }
            }
        }

        BOOL(1)
    }
}

fn get_caps_state() -> bool {
    unsafe { (GetKeyState(VK_CAPITAL.0 as i32) & 1) != 0 }
}

fn tap_caps() {
    let inputs = [
        make_caps_input(Default::default()),
        make_caps_input(KEYEVENTF_KEYUP),
    ];

    unsafe {
        let _ = SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

fn make_caps_input(flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_CAPITAL,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: MY_EXTRA_INFO,
            },
        },
    }
}

unsafe extern "system" fn keyboard_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code != HC_ACTION as i32 {
        unsafe { return CallNextHookEx(HHOOK(0), n_code, w_param, l_param) };
    }

    let kb = unsafe { *(l_param.0 as *const KBDLLHOOKSTRUCT) };

    if (kb.flags & KBDLLHOOKSTRUCT_FLAGS(LLKHF_INJECTED.0 as u32)) != KBDLLHOOKSTRUCT_FLAGS(0) {
        return unsafe { CallNextHookEx(HHOOK(0), n_code, w_param, l_param) };
    }

    let msg = w_param.0 as u32;
    let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

    match kb.vkCode {
        k if k == VK_CAPITAL.0 as u32 => {
            handle_caps(is_down, is_up);
            LRESULT(1)
        }
        VK_D if caps_state().lock().unwrap().down => {
            if is_down {
                activate_or_run(DISCORD_EXE, Some(DISCORD_PATH));
            }
            caps_state().lock().unwrap().used_as_modifier = true;
            LRESULT(1)
        }
        _ => unsafe { CallNextHookEx(HHOOK(0), n_code, w_param, l_param) },
    }
}

fn handle_caps(is_down: bool, is_up: bool) {
    let mut state = caps_state().lock().unwrap();

    if is_down {
        state.down = true;
        state.used_as_modifier = false;
        state.initial_state = get_caps_state();
        return;
    }

    if is_up {
        state.down = false;
        let was_modifier = state.used_as_modifier;
        drop(state);

        if !was_modifier {
            tap_caps();
        }
    }
}
