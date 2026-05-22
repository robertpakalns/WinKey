use std::{
    mem::size_of,
    process::Command,
    ptr::null_mut,
    sync::{Mutex, OnceLock},
};
use windows::Win32::{
    Foundation::{CloseHandle, HINSTANCE, HMODULE, HWND, LPARAM, LRESULT, WPARAM},
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
            HHOOK, IsIconic, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, SW_RESTORE, SW_SHOW,
            SetForegroundWindow, SetWindowsHookExW, ShowWindow, UnhookWindowsHookEx,
            WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WS_VISIBLE,
        },
    },
};
use windows_core::BOOL;

use crate::config::{BindingMap, Modifier, load_config};

mod config;

const MY_EXTRA_INFO: usize = 0xDEADBEEF;

#[derive(Default)]
struct ModifierState {
    down: bool,
    used_as_modifier: bool,
    initial_state: bool,
}

static MODIFIER_STATE: OnceLock<Mutex<ModifierState>> = OnceLock::new();
static BINDINGS: OnceLock<BindingMap> = OnceLock::new();

fn modifier_state() -> &'static Mutex<ModifierState> {
    MODIFIER_STATE.get_or_init(|| Mutex::new(ModifierState::default()))
}

fn bindings() -> &'static BindingMap {
    BINDINGS.get().expect("BINDINGS not initialised")
}

fn main() {
    let mut args = std::env::args_os();

    args.next();
    let config_path = match args.next() {
        Some(arg) if arg == "--config" => args.next().unwrap_or_else(|| {
            eprintln!("Missing config path after --config");
            std::process::exit(1);
        }),

        _ => {
            eprintln!("Usage: myhotkeys --config <config-path>");
            std::process::exit(1);
        }
    };

    let config_path = config_path.to_string_lossy();

    let map = load_config(&config_path);
    println!("Loaded {} binding(s):", map.len());
    for ((modifier, vk), (exe, path)) in &map {
        let key_char = char::from_u32(*vk).unwrap_or('?');
        match path {
            Some(p) => println!("{modifier:?}+{key_char} -> {exe} ({p})"),
            None => println!("{modifier:?}+{key_char} -> {exe}"),
        }
    }

    BINDINGS.get_or_init(|| map);
    let _ = modifier_state();

    let hook = unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_proc),
            Some(HINSTANCE(null_mut())),
            0,
        )
        .expect("Failed to install keyboard hook")
    };

    println!("Listening for combos…");

    let mut msg = MSG::default();

    while unsafe { GetMessageW(&mut msg, Some(HWND(null_mut())), 0, 0) }.into() {}

    let _ = unsafe { UnhookWindowsHookEx(hook) };
}

fn handle_modifier(is_down: bool, is_up: bool) {
    let mut state = modifier_state().lock().unwrap();
    if is_down {
        state.down = true;
        state.used_as_modifier = false;
        state.initial_state = get_modifier_state();
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
        let fg_tid = if fg.0 != null_mut() {
            GetWindowThreadProcessId(fg, None)
        } else {
            target_tid
        };

        if fg_tid != cur_tid {
            let _ = AttachThreadInput(cur_tid, fg_tid, true);
        }
        if target_tid != cur_tid && target_tid != fg_tid {
            let _ = AttachThreadInput(cur_tid, target_tid, true);
        }

        let _ = SetForegroundWindow(hwnd);
        let _ = BringWindowToTop(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
}

fn launch_path(path: &str) {
    let expanded = match expand_env_vars(path) {
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

fn expand_env_vars(path: &str) -> Option<String> {
    let mut result = String::new();
    let remaining = path;
    let mut last_end = 0;

    while let Some(start) = remaining[last_end..].find('%') {
        let start_pos = last_end + start;
        if let Some(end) = remaining[start_pos + 1..].find('%') {
            let end_pos = start_pos + 1 + end;
            let var_name = &remaining[start_pos + 1..end_pos];

            // Append the text before the variable
            result.push_str(&remaining[last_end..start_pos]);

            // Get the environment variable value
            match std::env::var(var_name) {
                Ok(value) => result.push_str(&value),
                Err(_) => {
                    eprintln!("Environment variable '%{}' not found", var_name);
                    // Keep the original variable pattern if not found
                    result.push_str(&remaining[start_pos..=end_pos]);
                }
            }

            last_end = end_pos + 1;
        } else {
            // No closing "%", treat the rest as literal
            result.push_str(&remaining[last_end..]);
            break;
        }
    }

    // Append any remaining text
    if last_end < remaining.len() {
        result.push_str(&remaining[last_end..]);
    }

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn find_window_by_process(target_exe: &str) -> Option<HWND> {
    struct SearchState {
        target_exe: String,
        found: Option<HWND>,
    }

    let mut state = SearchState {
        target_exe: target_exe.to_string(),
        found: None,
    };

    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(&mut state as *mut SearchState as isize),
        );
    }

    return state.found;

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
        let len = unsafe { K32GetModuleBaseNameW(handle, Some(HMODULE(null_mut())), &mut buffer) };
        let _ = unsafe { CloseHandle(handle) };
        if len == 0 {
            return BOOL(1);
        }

        let name = String::from_utf16_lossy(&buffer[..len as usize]);
        if !name.eq_ignore_ascii_case(&state.target_exe) {
            return BOOL(1);
        }

        let has_title = unsafe { GetWindowTextLengthW(hwnd) } > 0;
        let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
        let is_visible = (style & WS_VISIBLE.0) != 0;
        let is_minimized = unsafe { IsIconic(hwnd) }.as_bool();

        // Accept visible windows with a title, or minimized windows with a title
        // Reject everything else (tray icons, background helper windows, etc.)
        if has_title && (is_visible || is_minimized) {
            state.found = Some(hwnd);
            return BOOL(0);
        }

        BOOL(1)
    }
}

fn get_modifier_state() -> bool {
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
        unsafe { return CallNextHookEx(Some(HHOOK(null_mut())), n_code, w_param, l_param) };
    }

    let kb = unsafe { *(l_param.0 as *const KBDLLHOOKSTRUCT) };

    if (kb.flags & LLKHF_INJECTED).0 != 0 && kb.dwExtraInfo != MY_EXTRA_INFO {
        return unsafe { CallNextHookEx(Some(HHOOK(null_mut())), n_code, w_param, l_param) };
    }

    let msg = w_param.0 as u32;
    let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
    let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

    // Caps Lock itself — only relevant when at least one Caps+X binding exists
    if kb.vkCode == VK_CAPITAL.0 as u32 {
        let has_caps_bindings = bindings().keys().any(|(m, _)| *m == Modifier::Caps);
        if has_caps_bindings {
            handle_modifier(is_down, is_up);
            return LRESULT(1);
        }
    }

    // Check active modifier states and look up the combo
    if modifier_state().lock().unwrap().down {
        if let Some((exe, path)) = bindings().get(&(Modifier::Caps, kb.vkCode)) {
            if is_down {
                activate_or_run(exe, path.as_deref());
            }
            modifier_state().lock().unwrap().used_as_modifier = true;
            return LRESULT(1);
        }
    }

    unsafe { CallNextHookEx(Some(HHOOK(null_mut())), n_code, w_param, l_param) }
}
