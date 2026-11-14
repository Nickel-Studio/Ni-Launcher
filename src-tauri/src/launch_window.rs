//! 游戏窗口识别与标题 / 最大化动作；仅接受 Minecraft 实际渲染窗口的窗口类。
//!
//! 加载器启动画面常与游戏使用同一 Java 进程和相同窗口类，因此不能只按 PID
//! 命中；这里排除 FML / Quilt / GLFW 消息窗口，避免过早隐藏启动器。

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameWindowInfo {
    pub title: String,
    pub class_name: String,
}

pub fn is_game_window_class(class_name: &str) -> bool {
    matches!(class_name, "GLFW30" | "SDL_app" | "LWJGL" | "SunAwtFrame")
}

pub fn is_splash_title(title: &str) -> bool {
    let title = title.trim().to_ascii_lowercase();
    title.starts_with("fml")
        || title.starts_with("quilt")
        || title.starts_with("glfw")
        || title == "popupmessagewindow"
}

pub fn is_game_window(
    target_pid: u32,
    window_pid: u32,
    class_name: &str,
    title: &str,
) -> bool {
    window_pid == target_pid
        && is_game_window_class(class_name)
        && !is_splash_title(title)
}

pub fn replace_title_placeholders(template: &str, date: &str, time: &str) -> String {
    template
        .replace("{date}", date)
        .replace("{time}", time)
}

pub fn hide_after_window(mode: &str) -> bool {
    matches!(mode, "hide" | "hide-close")
}

pub fn minimize_after_window(mode: &str) -> bool {
    mode == "minimize"
}

pub fn close_after_window(mode: &str) -> bool {
    mode == "close"
}

pub fn restore_on_exit(mode: &str) -> bool {
    mode == "hide"
}

pub fn close_on_exit(mode: &str) -> bool {
    mode == "hide-close"
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::ffi::c_void;

    const MAX_WINDOW_TEXT: usize = 1024;
    const SW_MAXIMIZE: i32 = 3;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(
            callback: unsafe extern "system" fn(*mut c_void, isize) -> i32,
            data: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(window: *mut c_void, pid: *mut u32) -> u32;
        fn IsWindowVisible(window: *mut c_void) -> i32;
        fn GetClassNameW(window: *mut c_void, buffer: *mut u16, capacity: i32) -> i32;
        fn GetWindowTextW(window: *mut c_void, buffer: *mut u16, capacity: i32) -> i32;
        fn SetWindowTextW(window: *mut c_void, text: *const u16) -> i32;
        fn ShowWindow(window: *mut c_void, command: i32) -> i32;
    }

    #[derive(Default)]
    struct Search {
        pid: u32,
        found: Option<(*mut c_void, GameWindowInfo)>,
    }

    unsafe extern "system" fn visit(window: *mut c_void, data: isize) -> i32 {
        // SAFETY: EnumWindows 同步回调；data 指向本次调用独占的 Search。
        unsafe {
            let search = &mut *(data as *mut Search);
            if search.found.is_some() || IsWindowVisible(window) == 0 {
                return 1;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, &mut pid);
            if pid != search.pid {
                return 1;
            }
            let mut class_buffer = [0_u16; 128];
            let class_len =
                GetClassNameW(window, class_buffer.as_mut_ptr(), class_buffer.len() as i32);
            let class_name = String::from_utf16_lossy(
                &class_buffer[..class_len.max(0) as usize],
            );
            let mut title_buffer = [0_u16; MAX_WINDOW_TEXT];
            let title_len =
                GetWindowTextW(window, title_buffer.as_mut_ptr(), title_buffer.len() as i32);
            let title = String::from_utf16_lossy(
                &title_buffer[..title_len.max(0) as usize],
            );
            if !is_game_window(search.pid, pid, &class_name, &title) {
                return 1;
            }
            search.found = Some((
                window,
                GameWindowInfo {
                    title,
                    class_name,
                },
            ));
            0
        }
    }

    fn find(pid: u32) -> Option<(*mut c_void, GameWindowInfo)> {
        let mut search = Search { pid, found: None };
        // SAFETY: callback 不保存借用，也不跨线程访问 Search。
        unsafe {
            EnumWindows(visit, &mut search as *mut Search as isize);
        }
        search.found
    }

    pub fn find_game_window(pid: u32) -> Option<GameWindowInfo> {
        find(pid).map(|(_, info)| info)
    }

    pub fn set_window_title(pid: u32, template: &str) -> bool {
        let Some((window, _)) = find(pid) else {
            return false;
        };
        if template.is_empty() {
            return true;
        }
        let (date, time) = local_date_time();
        let title = replace_title_placeholders(template, &date, &time);
        let wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        // SAFETY: 窗口句柄来自当前枚举；字符串在同步调用期间有效。
        unsafe { SetWindowTextW(window, wide.as_ptr()) != 0 }
    }

    pub fn maximize_window(pid: u32) -> bool {
        let Some((window, _)) = find(pid) else {
            return false;
        };
        // SAFETY: 窗口句柄来自当前枚举，ShowWindow 在同步调用中使用。
        unsafe { ShowWindow(window, SW_MAXIMIZE) != 0 }
    }

    #[repr(C)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(time: *mut SystemTime);
    }

    fn local_date_time() -> (String, String) {
        let mut time = SystemTime {
            year: 0,
            month: 0,
            day_of_week: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            milliseconds: 0,
        };
        // SAFETY: 输出结构布局与 SYSTEMTIME 一致，指针只在同步调用中使用。
        unsafe { GetLocalTime(&mut time) };
        (
            format!("{}/{}/{}", time.year, time.month, time.day),
            format!("{:02}:{:02}:{:02}", time.hour, time.minute, time.second),
        )
    }
}

#[cfg(windows)]
pub use platform::{find_game_window, maximize_window, set_window_title};

#[cfg(not(windows))]
pub fn find_game_window(_pid: u32) -> Option<GameWindowInfo> {
    None
}

#[cfg(not(windows))]
pub fn set_window_title(_pid: u32, _template: &str) -> bool {
    false
}

#[cfg(not(windows))]
pub fn maximize_window(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_game_window_classes_pass_splash_filter() {
        for class_name in ["GLFW30", "SDL_app", "LWJGL", "SunAwtFrame"] {
            assert!(is_game_window(9, 9, class_name, "Minecraft 1.21"));
        }
        assert!(!is_game_window(9, 8, "GLFW30", "Minecraft"));
        assert!(!is_game_window(9, 9, "Chrome_WidgetWin_1", "Minecraft"));
        assert!(!is_game_window(9, 9, "GLFW30", "FML early loading"));
        assert!(!is_game_window(9, 9, "GLFW30", "Quilt Loader"));
        assert!(!is_game_window(9, 9, "GLFW30", "GLFW message window"));
        assert!(!is_game_window(9, 9, "GLFW30", "PopupMessageWindow"));
    }

    #[test]
    fn dynamic_title_replaces_every_date_and_time_marker() {
        assert_eq!(
            replace_title_placeholders("MC {date} {time} / {date}", "2026/11/11", "09:08:07"),
            "MC 2026/11/11 09:08:07 / 2026/11/11"
        );
        assert_eq!(replace_title_placeholders("Minecraft", "x", "y"), "Minecraft");
    }

    #[test]
    fn visibility_actions_match_five_modes() {
        for (mode, hide, minimize, close, restore, close_exit) in [
            ("keep", false, false, false, false, false),
            ("hide", true, false, false, true, false),
            ("hide-close", true, false, false, false, true),
            ("close", false, false, true, false, false),
            ("minimize", false, true, false, false, false),
        ] {
            assert_eq!(hide_after_window(mode), hide, "{mode}");
            assert_eq!(minimize_after_window(mode), minimize, "{mode}");
            assert_eq!(close_after_window(mode), close, "{mode}");
            assert_eq!(restore_on_exit(mode), restore, "{mode}");
            assert_eq!(close_on_exit(mode), close_exit, "{mode}");
        }
    }
}
