//! 前台窗口跟踪（spec §4.1 WindowTracker）。
//!
//! 查询前台窗口标题，判定是否匹配目标串（on_target），并检测句柄变化以计切窗次数。
//! 不做 GetWindowRect —— rect 是 renderer 的事（见 controller.rs Frame 设计注释）。

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
};

use super::logic::title_matches;

pub struct WindowTracker {
    target: String,
    /// 上次查询到的前台窗口句柄原始值。0 = 尚无（桌面/无前台窗口）。
    /// 用 isize 规避 HWND 的生命周期/别名问题，仅用于「是否变化」比较。
    last_hwnd: isize,
}

impl WindowTracker {
    pub fn new(target_window_title_contains: impl Into<String>) -> Self {
        Self {
            target: target_window_title_contains.into(),
            last_hwnd: 0,
        }
    }

    /// 查询当前前台窗口。返回 `(on_target, switched)`：
    /// - `on_target`：当前前台标题是否匹配目标串。
    /// - `switched`：前台窗口句柄是否自上次调用发生了变化（调用方据此计 switches）。
    pub fn poll(&mut self) -> (bool, bool) {
        let hwnd = unsafe { GetForegroundWindow() };
        let raw = hwnd.0 as isize;
        // 仅当有真实窗口且句柄变化时算切换。
        let switched = raw != 0 && raw != self.last_hwnd;
        self.last_hwnd = raw;

        let on_target = if raw == 0 {
            false
        } else {
            let title = read_title(hwnd);
            title_matches(&title, &self.target)
        };
        (on_target, switched)
    }
}

/// 读取窗口标题。空标题/读取失败均返回空串（title_matches 对空串安全）。
fn read_title(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len as usize) + 1];
        let got = GetWindowTextW(hwnd, &mut buf);
        if got <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..got as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_tracker_target_stored() {
        let t = WindowTracker::new("Visual Studio Code");
        assert_eq!(t.target, "Visual Studio Code");
        assert_eq!(t.last_hwnd, 0);
    }
}
