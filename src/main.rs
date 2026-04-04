use std::{mem::size_of, process::Command};
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
            GetKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
            VK_CAPITAL,
        },
        WindowsAndMessaging::{
            BringWindowToTop, CallNextHookEx, EnumWindows, GetForegroundWindow, GetMessageW,
            GetWindowThreadProcessId, HC_ACTION, HHOOK, IsIconic, KBDLLHOOKSTRUCT,
            KBDLLHOOKSTRUCT_FLAGS, LLKHF_INJECTED, MSG, SW_RESTORE, SW_SHOW, SetForegroundWindow,
            SetWindowsHookExW, ShowWindow, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN,
            WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

const DISCORD_STR: &str =
    r"%USERPROFILE%\AppData\Local\Discord\Update.exe --processStart Discord.exe";

static mut CAPS_DOWN: bool = false;
static mut CAPS_USED_AS_MODIFIER: bool = false;
static mut CAPS_INITIAL_STATE: bool = false;

const MY_EXTRA_INFO: usize = 0xDEADBEEF;

fn main() {
    unsafe {
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), HINSTANCE(0), 0)
            .expect("Failed to install hook");

        println!("Listening for Caps combos...");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(0), 0, 0).into() {}

        let _ = UnhookWindowsHookEx(hook);
    }
}

fn activate_or_run(exe: &str, path: Option<&str>) {
    if let Some(hwnd) = find_window_by_process(exe) {
        unsafe {
            if IsIconic(hwnd).as_bool() {
                ShowWindow(hwnd, SW_RESTORE);
            } else {
                ShowWindow(hwnd, SW_SHOW);
            }

            let fg = GetForegroundWindow();
            if fg.0 != 0 {
                let cur_tid = GetCurrentThreadId();
                let fg_tid = GetWindowThreadProcessId(fg, None);

                AttachThreadInput(cur_tid, fg_tid, true);
                SetForegroundWindow(hwnd);
                let _ = BringWindowToTop(hwnd);
                AttachThreadInput(cur_tid, fg_tid, false);
            } else {
                SetForegroundWindow(hwnd);
            }
        }
    } else if let Some(path) = path {
        let expanded = if path.contains("%USERPROFILE%") {
            if let Ok(profile) = std::env::var("USERPROFILE") {
                path.replace("%USERPROFILE%", &profile)
            } else {
                eprintln!("USERPROFILE environment variable not found");
                return;
            }
        } else {
            path.to_string()
        };

        let mut parts = expanded.split_whitespace();
        let program = match parts.next() {
            Some(p) => p,
            None => {
                eprintln!("Empty command line");
                return;
            }
        };
        let args: Vec<&str> = parts.collect();

        if let Err(e) = Command::new(program).args(args).spawn() {
            eprintln!("Failed to launch Discord: {e}");
        }
    } else {
        if let Err(e) = Command::new(exe).spawn() {
            eprintln!("Failed to launch {exe}: {e}");
        }
    }
}

fn find_window_by_process(target: &str) -> Option<HWND> {
    let mut result: Option<HWND> = None;

    let _ = unsafe { EnumWindows(Some(enum_proc), LPARAM(&mut result as *mut _ as isize)) };

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };

        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) };

        if let Ok(handle) = handle {
            let mut buffer = [0u16; 260];

            if unsafe { K32GetModuleBaseNameW(handle, HMODULE(0), &mut buffer) } > 0 {
                let name = String::from_utf16_lossy(&buffer);
                let name = name.trim_matches(char::from(0));

                if name.eq_ignore_ascii_case("Discord.exe") {
                    let result = lparam.0 as *mut Option<HWND>;
                    *result = Some(hwnd);
                    let _ = unsafe { CloseHandle(handle) };
                    return BOOL(0);
                }
            }

            let _ = unsafe { CloseHandle(handle) };
        }

        BOOL(1)
    }

    result
}

fn get_caps_state() -> bool {
    unsafe { (GetKeyState(VK_CAPITAL.0 as i32) & 1) != 0 }
}

unsafe extern "system" fn keyboard_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code != HC_ACTION as i32 {
        unsafe { return CallNextHookEx(HHOOK(0), n_code, w_param, l_param) };
    }

    let kb = *(l_param.0 as *const KBDLLHOOKSTRUCT);

    if (kb.flags & KBDLLHOOKSTRUCT_FLAGS(LLKHF_INJECTED.0 as u32)) != KBDLLHOOKSTRUCT_FLAGS(0) {
        unsafe { return CallNextHookEx(HHOOK(0), n_code, w_param, l_param) };
    }

    let is_down = w_param.0 as u32 == WM_KEYDOWN || w_param.0 as u32 == WM_SYSKEYDOWN;
    let is_up = w_param.0 as u32 == WM_KEYUP || w_param.0 as u32 == WM_SYSKEYUP;

    match kb.vkCode {
        k if k == VK_CAPITAL.0 as u32 => {
            if is_down {
                CAPS_DOWN = true;
                CAPS_USED_AS_MODIFIER = false;
                CAPS_INITIAL_STATE = get_caps_state();

                return LRESULT(1); // Suppress the real Caps press
            }

            if is_up {
                CAPS_DOWN = false;

                if CAPS_USED_AS_MODIFIER {
                    return LRESULT(1);
                } else {
                    tap_caps();
                    return LRESULT(1);
                }
            }
        }

        0x44 => {
            if CAPS_DOWN {
                CAPS_USED_AS_MODIFIER = true;

                if is_down {
                    activate_or_run("Discord.exe", Some(DISCORD_STR));
                }

                return LRESULT(1);
            }
        }

        _ => {}
    }

    unsafe { CallNextHookEx(HHOOK(0), n_code, w_param, l_param) }
}

fn tap_caps() {
    unsafe {
        let mut inputs = [
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_CAPITAL,
                        wScan: 0,
                        dwFlags: Default::default(),
                        time: 0,
                        dwExtraInfo: MY_EXTRA_INFO,
                    },
                },
            },
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_CAPITAL,
                        wScan: 0,
                        dwFlags: KEYEVENTF_KEYUP,
                        time: 0,
                        dwExtraInfo: MY_EXTRA_INFO,
                    },
                },
            },
        ];

        let _ = SendInput(&mut inputs, size_of::<INPUT>() as i32);
    }
}
