//! Windows 平台信号源实现（spec §4.1 platform/）。
//!
//! 仅 `cfg(windows)` 编译。`controller` 通过 `&mut dyn SignalProvider` 消费，
//! 本模块不反向依赖 controller。privacy 硬规则：所有 hook 回调只计数、只记时间戳，
//! 绝不保留按键内容（vkCode 除外比较 VK_BACK 这一个常量）。见 spec §9。
//!
//! Task 1（本阶段）只立模块骨架 + logic 纯逻辑。InputHook / WindowTracker /
//! Win32SignalProvider 在 Task 2/3/4 填充。

mod logic;

#[cfg(windows)]
mod input_hook;
#[cfg(windows)]
mod window_tracker;

#[cfg(windows)]
pub use window_tracker::WindowTracker;

// Task 4 起在此定义 Win32SignalProvider 并 impl SignalProvider。
