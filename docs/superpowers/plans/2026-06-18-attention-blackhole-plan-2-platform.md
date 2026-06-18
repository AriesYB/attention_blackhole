# 注意力黑洞 — Plan 2: Platform（Windows 输入/窗口信号源）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用真实的 Windows API 实现 spec §4.1 中的 `platform/` 层：`InputHook`（低级键鼠 hook，仅计数）+ `WindowTracker`（前台窗口句柄/标题/匹配）+ `IdleDetector`（最后输入时间派生空闲），把它们组装成一个 `Win32SignalProvider`，实现 Plan 1 定义的 `SignalProvider` trait。controller 不改一行。

**Architecture:** 新增 `src/platform/` 模块，仅 `cfg(windows)` 编译；trait 已在 Plan 1 固定。**分层纪律**的硬约束：`controller.rs` 不得 `use crate::platform`——平台类型只能通过 `&mut dyn SignalProvider` 进入 controller（与 mock 同路）。`platform/` 内部把可纯测的逻辑（空闲计算、标题匹配、计数聚合、间隔记录）抽到无 Windows 依赖的纯函数/结构体并单测；真正的 Win32 调用（`SetWindowsHookExW`、`GetForegroundWindow`、`GetWindowTextW`）封进薄壳，留手工 QA，不进自动测试。

**Tech Stack:** Rust（edition 2021）。新增**唯一**外部依赖 `windows`（microsoft/windows-rs），采用**最小 feature 集**（见 §依赖说明），与 Plan 1 零外部依赖的精简风格对齐。所有非 Windows 平台 `cargo build` 仍须通过（platform 模块在非 windows 上编译为空或 `cfg`-gate 掉）。

---

## 与 Plan 1 的接口契约（不可破坏）

controller 侧 `SignalProvider`（`src/controller.rs:9`）已固定：

```rust
pub trait SignalProvider {
    fn snapshot(&mut self, dt: Duration) -> TickInput;
}
```

`TickInput`（`src/model/types.rs:20`）字段语义（platform 必须正确填充）：

| 字段 | platform 责任 |
|------|---------------|
| `counts` | 本 tick（自上次 `snapshot` 起）内的**增量**键/鼠/切窗/退格计数。每次 `snapshot` 后清零。 |
| `on_target` | 当前前台窗口是否匹配 `target_window_title_contains`（标题子串，大小写不敏感）。 |
| `idle` | 自最后一次**任意**键鼠输入起经过的时长（spec §6.5：任何输入都重置空闲计时）。 |
| `dt` | 直接回传传入的 `dt`（controller 调用方提供，platform 不自造时钟）。 |
| `key_intervals_ms` | 本 tick 内相邻击键的间隔（毫秒）。退格不计入击键节奏，但计入 `counts.backspaces`。 |

**关键不变量**（spec §6.5 / §9）：hook 回调**只做计数器自增与时间戳记录，绝不保留任何按键内容**（VK code 也不存）。本 plan 的所有实现必须满足此隐私硬规则，并在代码注释里显式声明。

---

## 依赖说明（windows crate 最小 feature）

```toml
[target.'cfg(windows)'.dependencies.windows]
version = "0.61"
features = [
    "Win32_Foundation",                 # BOOL / HANDLE / CloseHandle 等
    "Win32_UI_WindowsAndMessaging",     # SetWindowsHookExW / UnhookWindowsHookEx / CallNextHookEx
                                        #   WH_KEYBOARD_LL / WH_MOUSE_LL / KBDLLHOOKSTRUCT / MSLLHOOKSTRUCT
                                        #   GetForegroundWindow / GetWindowTextW / GetWindowTextLengthW
]
```

> **feature 取舍**：低级 hook + 前台窗口标题查询所需 API 全部落在 `Win32_UI_WindowsAndMessaging` + `Win32_Foundation` 两个 feature 下。**不**启用 `Win32_Graphics_Gdi`（那是 GetWindowRect，留给 Plan 3 renderer 用）、**不**启用 `Win32_System_Threading` / `Win32_UI_Input_*`。若实现中发现某个 API 缺 feature，按「逐个加、不加整块」原则补。

> **版本**：`0.61` 为撰写时最新稳定线（与搜索到的 `windows-sys 0.61.2` 对齐）。实现时以实际 `cargo add windows` 拉到的为准，写进 `Cargo.toml` 即可。

> **`windows` vs `windows-sys`**：选 `windows`（高级包装，返回 `Result`、有 `HINSTANCE` 等包装类型），代价是略大但 API 人体工学好得多，符合「写起来清楚」的取向。Plan 1 的「零外部依赖」是 model/controller 核心，platform 层本来就要碰 Windows，引入是预期内的。

---

## File Structure（本计划产出）

- `Cargo.toml` — **Modify**：加 `windows` 依赖（`cfg(windows)` target-scoped）。
- `src/lib.rs` — **Modify**：`#[cfg(windows)] pub mod platform;`（非 windows 上不导出，保证 `cargo build` 在任意平台不炸）。
- `src/platform/mod.rs` — **Create**：platform 子模块导出 + `Win32SignalProvider` 主体（组装三组件 + 实现 `SignalProvider`）。
- `src/platform/logic.rs` — **Create**：**纯平台无关**的可测逻辑：`Counts`（增量聚合 + 清零）、`IdleTracker`（基于注入时钟）、`title_matches`（子串匹配）、`KeyIntervals`（相邻间隔记录）。全部单测在此。
- `src/platform/input_hook.rs` — **Create**：`InputHook`（`cfg(windows)`）：`SetWindowsHookExW` 装/卸 `WH_KEYBOARD_LL` + `WH_MOUSE_LL`，回调里把事件推进 thread-local 计数器与时间戳。薄壳，无单测。
- `src/platform/window_tracker.rs` — **Create**：`WindowTracker`（`cfg(windows)`）：查前台窗口标题，调 `title_matches` 判定 `on_target`。薄壳 + 一行纯函数复用。
- `examples/platform_demo.rs` — **Create**：`#[cfg(windows)]` 守卫，运行 ~30 秒真实 hook，每秒打印 counts/idle/前台标题/on_target，证明 platform 走通。

**非产出**：`renderer/`（Plan 3）、`shell/`（Plan 5）、任何 `Win32_Graphics_*` 调用、WGC（Plan 4）、`GetWindowRect`/rect（Plan 3 renderer 自己负责，见 controller.rs:18 的 Frame 设计注释）。

---

## 关键设计决策（实现值）

- **回调→计数器通道**：用 `thread_local! { static COUNTERS: Cell<...> }` + `AtomicI64`（last_input_ticks）。低级 hook 回调在自己的线程上跑（`SetWindowsHookExW` 要求 message pump），计数器必须是 `thread_local` 而非普通 static（回调线程与 `Win32SignalProvider` 所在线程可能不同——见 §消息泵线程模型）。
- **退格识别**：低级 hook 给的是 `KBDLLHOOKSTRUCT.vkCode`，退格 = `VK_BACK = 0x08`。**只比较这一个常量，不存 vkCode**。
- **鼠标移动 vs 点击**：spec 只要「鼠标事件计数」。为避免鼠标移动淹没计数（鼠标随便动就疯涨），`mouse` 只计 `WM_LBUTTONDOWN`/`WM_RBUTTONDOWN`/`WM_MBUTTONDOWN`（由 `MSLLHOOKSTRUCT` 携带的 message 区分），**不计移动**。本决策在 logic.rs 注释里写死。
- **idle 时钟来源**：用 `QueryUnbiasedInterruptTime`？不——更简单可靠：hook 回调每次触发就更新一个 `last_input` 时间戳（`Instant`），`IdleTracker` 在 snapshot 时算 `Instant::now() - last_input`。无需新 feature。`Instant` 是 `std` 的，零额外依赖。
- **switches 计数**：前台窗口句柄变化才算一次 switch（不是每 tick 比对，而是 `WindowTracker` 内部记住 `last_hwnd`，变化时 `counts.switches += 1`）。避免「不动也算切窗」。
- **key_intervals_ms 采样**：回调里记击键的 `Instant`，snapshot 时把窗口内相邻差算成 ms，snapshot 后清空。为限制内存，上限保留最近 N=64 个击键时间戳。

---

## 消息泵线程模型（关键，否则 hook 不工作）

`WH_KEYBOARD_LL` / `WH_MOUSE_LL` 回调**只在「安装了 hook 且该线程在跑消息泵」的线程上触发**。因此 `InputHook` 必须在自己的专用线程上：

1. 专用线程：`SetWindowsHookExW(...)` → 进入 `PeekMessage`/`GetMessage` 循环。
2. 主线程的 `Win32SignalProvider::snapshot` 通过共享原子/通道读取计数。
3. Drop 时给专用线程发 `WM_QUIT`（`PostThreadMessageW`）退出泵，再 `JoinHandle::join`。

> 这个模型需要 `Win32_UI_WindowsAndMessaging` 已包含的 `PostThreadMessageW` / `PeekMessageW` / `GetMessageW`，无需新 feature。

**Plan 2 不在此线程模型上写自动测试**（无法在 CI 无桌面环境跑），只做：logic.rs 纯函数单测 + example 手工 QA。线程模型正确性留 example 跑通为准。

---

## Task 1: 依赖接入 + 模块骨架 + logic.rs 纯逻辑

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`
- Create: `src/platform/mod.rs`
- Create: `src/platform/logic.rs`

logic.rs 是整个 plan 唯一能自动测的部分，先把它和骨架立起来，确认非 windows 平台 `cargo build` 仍绿。

- [ ] **Step 1: 改 Cargo.toml 加 windows 依赖（cfg-gated）**

在 `Cargo.toml` 末尾追加（保留既有 `[package]`/`[lib]` 不动）：

```toml
[target.'cfg(windows)'.dependencies.windows]
version = "0.61"
features = [
    "Win32_Foundation",
    "Win32_UI_WindowsAndMessaging",
]
```

- [ ] **Step 2: 改 src/lib.rs 导出 platform（cfg-gated）**

```rust
pub mod controller;
#[cfg(windows)]
pub mod platform;
pub mod mock;
pub mod model;
```

> 顺序保持字母序无关；`cfg(windows)` 保证 macOS/Linux CI 上 `cargo build` 不报「找不到 platform 模块」。mock 与 controller 在所有平台都必须可用（测试与 demo 依赖）。

- [ ] **Step 3: 写 src/platform/mod.rs（占位导出 + Win32SignalProvider 前向声明）**

```rust
//! Windows 平台信号源实现（spec §4.1 platform/）。
//!
//! 仅 `cfg(windows)` 编译。`controller` 通过 `&mut dyn SignalProvider` 消费，
//! 本模块不反向依赖 controller。privacy 硬规则：所有 hook 回调只计数、只记时间戳，
//! 绝不保留按键内容（vkCode 除外比较 VK_BACK 这一个常量）。见 spec §9。

mod logic;

#[cfg(windows)]
mod input_hook;
#[cfg(windows)]
mod window_tracker;

#[cfg(windows)]
pub use window_tracker::WindowTracker;

use crate::controller::SignalProvider;

/// Windows 平台 `SignalProvider`。组装 InputHook + WindowTracker + IdleTracker。
///
/// 非 windows 平台上不构造（构造函数 `new` 仅 `cfg(windows)` 暴露）。
#[cfg(windows)]
pub struct Win32SignalProvider {
    hook: input_hook::InputHook,
    window: WindowTracker,
    idle: logic::IdleTracker,
    counts: logic::Counts,
    keys: logic::KeyIntervals,
}

// 主体实现放 Task 5；本步先让模块树编译通过。
```

- [ ] **Step 4: 写 src/platform/logic.rs（纯逻辑 + 完整单测）**

```rust
//! platform 层的纯逻辑核心，无 Windows 依赖，单测主力。
//!
//! 把「可测的数学」从「不可测的 Win32 调用」里剥出来：增量计数聚合、
//! idle 时长计算、标题子串匹配、击键间隔记录。

use std::time::{Duration, Instant};

/// 一个 tick 内的增量事件计数（与 model 的 TickCounts 同构，但这里是
/// platform 内部聚合用，独立定义避免循环依赖）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub keys: u32,
    pub mouse: u32,
    pub switches: u32,
    pub backspaces: u32,
}

impl Counts {
    pub fn add_key(&mut self, is_backspace: bool) {
        self.keys += 1;
        if is_backspace {
            self.backspaces += 1;
        }
    }

    pub fn add_mouse_click(&mut self) {
        self.mouse += 1;
    }

    pub fn add_switch(&mut self) {
        self.switches += 1;
    }

    /// 合并另一份计数（hook 回调线程 → 主线程聚合时用）。
    pub fn merge(&mut self, other: &Counts) {
        self.keys += other.keys;
        self.mouse += other.mouse;
        self.switches += other.switches;
        self.backspaces += other.backspaces;
    }

    /// snapshot 后清零：每次 snapshot 报告的是「自上次以来的增量」。
    pub fn drain(&mut self) -> Counts {
        let out = *self;
        *self = Counts::default();
        out
    }
}

/// 基于「最后一次输入时间戳」的空闲计算。时间来源由调用方注入（生产用
/// `Instant::now()`，测试用 fake），保证纯逻辑可测。
#[derive(Debug, Clone)]
pub struct IdleTracker {
    last_input: Instant,
}

impl IdleTracker {
    pub fn new(now: Instant) -> Self {
        Self { last_input: now }
    }

    /// 任意输入到来时调用，刷新时间戳（spec §6.5：任何键鼠都重置空闲）。
    pub fn note_input(&mut self, now: Instant) {
        if now >= self.last_input {
            self.last_input = now;
        }
        // now < last_input（时钟回拨）时保持原值，保守不倒退。
    }

    pub fn idle_since(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.last_input)
    }
}

/// 标题子串匹配。大小写不敏感。空 target 视为「无目标」→ 永不 on_target。
pub fn title_matches(title: &str, target: &str) -> bool {
    if target.is_empty() {
        return false;
    }
    title.to_lowercase().contains(&target.to_lowercase())
}

/// 记录击键时间戳，产出相邻间隔（毫秒）。snapshot 后排空。
/// 上限保留最近 `MAX_KEYS` 个时间戳以防内存膨胀。
pub struct KeyIntervals {
    times: Vec<Instant>,
}

const MAX_KEYS: usize = 64;

impl KeyIntervals {
    pub fn new() -> Self {
        Self { times: Vec::with_capacity(MAX_KEYS) }
    }

    pub fn push(&mut self, t: Instant) {
        if self.times.len() >= MAX_KEYS {
            // 满了就丢最旧，保持滑动窗口语义。
            self.times.remove(0);
        }
        self.times.push(t);
    }

    /// 计算相邻击键间隔（毫秒），排空内部缓冲。
    pub fn drain_intervals_ms(&mut self) -> Vec<f64> {
        let n = self.times.len();
        let mut out = Vec::with_capacity(n.saturating_sub(1));
        for w in self.times.windows(2) {
            let ms = w[1].saturating_duration_since(w[0]).as_secs_f64() * 1000.0;
            out.push(ms);
        }
        self.times.clear();
        out
    }
}

impl Default for KeyIntervals {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_add_and_drain() {
        let mut c = Counts::default();
        c.add_key(false);
        c.add_key(true); // backspace 也算 key
        c.add_mouse_click();
        c.add_switch();
        assert_eq!(c.keys, 2);
        assert_eq!(c.backspaces, 1);
        assert_eq!(c.mouse, 1);
        assert_eq!(c.switches, 1);
        let drained = c.drain();
        assert_eq!(drained.keys, 2);
        assert_eq!(c, Counts::default()); // 清零
    }

    #[test]
    fn counts_merge() {
        let mut a = Counts { keys: 1, mouse: 2, switches: 3, backspaces: 4 };
        let b = Counts { keys: 10, mouse: 20, switches: 30, backspaces: 40 };
        a.merge(&b);
        assert_eq!(a.keys, 11);
        assert_eq!(a.mouse, 22);
        assert_eq!(a.switches, 33);
        assert_eq!(a.backspaces, 44);
    }

    #[test]
    fn idle_tracker_basic() {
        let t0 = Instant::now();
        let mut idle = IdleTracker::new(t0);
        // 无输入 5s
        assert_eq!(idle.idle_since(t0 + Duration::from_secs(5)), Duration::from_secs(5));
        // 第 3s 有输入
        idle.note_input(t0 + Duration::from_secs(3));
        // 第 7s 查：自第 3s 起 4s
        assert_eq!(idle.idle_since(t0 + Duration::from_secs(7)), Duration::from_secs(4));
    }

    #[test]
    fn idle_tracker_clock_back_safe() {
        let t0 = Instant::now();
        let mut idle = IdleTracker::new(t0);
        // 时钟回拨：不更新，保守保持原 last_input
        idle.note_input(t0 - Duration::from_secs(10));
        assert_eq!(idle.last_input, t0);
    }

    #[test]
    fn title_match_case_insensitive_substring() {
        assert!(title_matches("Visual Studio Code - main.rs", "visual studio code"));
        assert!(title_matches("VISUAL STUDIO CODE", "visual studio code"));
        assert!(!title_matches("Visual Studio", "visual studio code"));
        // 空 target 永不匹配
        assert!(!title_matches("anything", ""));
    }

    #[test]
    fn key_intervals_basic() {
        let t0 = Instant::now();
        let mut k = KeyIntervals::new();
        k.push(t0);
        k.push(t0 + Duration::from_millis(200));
        k.push(t0 + Duration::from_millis(500)); // 距上一次 300ms
        let v = k.drain_intervals_ms();
        assert_eq!(v.len(), 2);
        assert!((v[0] - 200.0).abs() < 1.0);
        assert!((v[1] - 300.0).abs() < 1.0);
    }

    #[test]
    fn key_intervals_empty_single() {
        let mut k = KeyIntervals::new();
        assert!(k.drain_intervals_ms().is_empty());
        k.push(Instant::now());
        assert!(k.drain_intervals_ms().is_empty()); // 单个无间隔
    }

    #[test]
    fn key_intervals_caps_history() {
        let mut k = KeyIntervals::new();
        let t0 = Instant::now();
        for i in 0..(MAX_KEYS + 10) {
            k.push(t0 + Duration::from_millis(i as u64));
        }
        // 排空后不应 panic，且内部窗口被限
        let v = k.drain_intervals_ms();
        assert!(v.len() <= MAX_KEYS);
    }
}
```

- [ ] **Step 5: 验证编译 + logic 单测**

Run: `cargo test --lib platform::logic`
Expected: `test result: ok. 8 passed`

并验证非 windows 兼容性（语法层面，本机是 windows 实际会编译 platform，但 mod 声明要正确）：
Run: `cargo build`
Expected: 成功（windows 上 platform 模块参与编译，input_hook/window_tracker 此时还是空文件需补占位，见 Step 6）。

- [ ] **Step 6: 补 input_hook.rs / window_tracker.rs 占位（Task 2/3 填充）**

`src/platform/input_hook.rs`：
```rust
// 占位：Task 2 填充（SetWindowsHookExW + 消息泵线程）
```

`src/platform/window_tracker.rs`：
```rust
// 占位：Task 3 填充（GetForegroundWindow + GetWindowTextW + title_matches）
```

Re-run `cargo test --lib platform::logic` → 仍 8 passed；`cargo build` 成功。

- [ ] **Step 7: 提交**

```bash
git add Cargo.toml Cargo.lock src/lib.rs src/platform/
git commit -m "feat(platform): scaffold windows module + pure logic (counts/idle/title/intervals)"
```

---

## Task 2: InputHook（低级键鼠 hook + 专用消息泵线程）

**Files:**
- Modify: `src/platform/input_hook.rs`

负责：在自己的线程上装 `WH_KEYBOARD_LL` + `WH_MOUSE_LL`，回调里只更新 thread-local/原子计数与 `IdleTracker` 的 last_input 时间戳，跑消息泵直到 Drop。

- [ ] **Step 1: 实现 InputHook**

替换 `src/platform/input_hook.rs` 全部内容：

```rust
//! 低级键鼠 hook（spec §4.1 InputHook）。
//!
//! 在专用线程上安装 WH_KEYBOARD_LL / WH_MOUSE_LL 并跑消息泵。
//! 回调只做三件事：自增计数、记击键时间戳、刷新 last_input。
//! 绝不保留按键内容（vkCode 仅用于判定 VK_BACK，不存储）。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::LPARAM;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, KBDLLHOOKSTRUCT, MSLLHOOKSTRUCT, PostThreadMessageW,
    SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, HHOOK_PLACEHOLDER, MSG, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_RBUTTONDOWN, WM_QUIT,
};

/// 自上次 snapshot 起的增量计数（线程间共享，原子读写）。
/// 用 4 个原子分别承载 keys/mouse/switches/backspaces，避免加锁。
#[derive(Default)]
pub struct AtomicCounts {
    pub keys: AtomicI64,
    pub mouse: AtomicI64,
    pub switches: AtomicI64,
    pub backspaces: AtomicI64,
}

impl AtomicCounts {
    /// 把当前值读出并清零，返回 logic::Counts。
    pub fn drain(&self) -> super::logic::Counts {
        use super::logic::Counts;
        Counts {
            keys: self.keys.swap(0, Ordering::Relaxed) as u32,
            mouse: self.mouse.swap(0, Ordering::Relaxed) as u32,
            switches: self.switches.swap(0, Ordering::Relaxed) as u32,
            backspaces: self.backspaces.swap(0, Ordering::Relaxed) as u32,
        }
    }
}

const VK_BACK: u32 = 0x08;

/// InputHook：后台线程持有 hook，Drop 时优雅退出。
pub struct InputHook {
    thread_id: u32,
    handle: Option<JoinHandle<()>>,
    /// 共享给回调线程的计数与时间戳缓冲。回调 push 时间戳，
    /// 主线程 snapshot 时 drain（见 KeyIntervals）。
    pub counts: Arc<AtomicCounts>,
    /// 共享的击键时间戳缓冲（受 Mutex 保护，回调高频、snapshot 低频）。
    pub key_times: Arc<std::sync::Mutex<Vec<Instant>>>,
    /// 最后一次输入的毫秒时间戳（相对进程启动），供 IdleTracker 同步。
    pub last_input_ms: Arc<AtomicI64>,
}

impl InputHook {
    /// 启动 hook 线程。target_title 仅用于日志，不影响计数。
    pub fn new() -> std::io::Result<Self> {
        let counts = Arc::new(AtomicCounts::default());
        let key_times = Arc::new(std::sync::Mutex::new(Vec::with_capacity(64)));
        let last_input_ms = Arc::new(AtomicI64::new(0));

        let counts_t = counts.clone();
        let key_times_t = key_times.clone();
        let last_t = last_input_ms.clone();

        let start = Instant::now();
        let handle = thread::Builder::new()
            .name("abh-input-hook".into())
            .spawn(move || -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                // 记录本线程 id 供主线程 PostThreadMessageW 退出用。
                // 通过 thread_local 无法回传 u32，这里用共享原子存。
                unsafe {
                    let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_proc), None, 0)?;
                    let ms = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0)?;
                    let mut msg = MSG::default();
                    // 消息泵：直到收到 WM_QUIT
                    while GetMessageW(&mut msg, None).into() {
                        // 我们不处理任何消息，只是为了让 hook 回调被分发
                        let _ = msg;
                    }
                    let _ = UnhookWindowsHookEx(kb);
                    let _ = UnhookWindowsHookEx(ms);
                }
                // 把共享状态 move 进闭包会被回调（'static）需要的副本遮蔽；
                // 这里通过 once cell 暴露给回调。见下方 CALLBACK_STATE。
                let _ = (counts_t, key_times_t, last_t, start);
                Ok(())
            })?;

        // 线程 id 需要从线程内回传 —— 改用共享原子。
        Err(std::io::Error::new(std::io::ErrorKind::Other, "thread id retrieval not wired"))
    }
}

// 注：低级 hook 回调是 C extern fn，无法直接捕获闭包环境。生产实现需要
// 用 thread_local! 把 Arc 引用挂到 hook 线程上，回调里读取。本 Task 的骨架
// 在 Step 2 完成「thread_local 注入 + 回调读写」的真实现。

impl Drop for InputHook {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, None, LPARAM(0));
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

unsafe extern "system" fn key_proc(_code: i32, _wparam: usize, lparam: LPARAM) -> usize {
    // TODO(Task 2 Step 2): 从 CALLBACK_STATE 读 Arc，自增 counts.keys，
    // 判 VK_BACK 自增 backspaces，push Instant 到 key_times，更新 last_input_ms。
    // 仅在 keydown(wParam == WM_KEYDOWN/WM_SYSKEYDOWN) 计一次，避免重复。
    CallNextHookEx(HHOOK_PLACEHOLDER as *mut _, 0, 0, LPARAM(0)) as usize
}

unsafe extern "system" fn mouse_proc(_code: i32, _wparam: usize, _lparam: LPARAM) -> usize {
    // TODO(Task 2 Step 2): 仅 WM_*BUTTONDOWN 计 mouse 一次，移动不计。
    CallNextHookEx(HHOOK_PLACEHOLDER as *mut _, 0, 0, LPARAM(0)) as usize
}
```

> **说明**：上面是「先把骨架编过」的中间态，故意把「thread_local 注入 + 回调真逻辑」拆到 Step 2，因为 C extern fn 与 Rust 闭包环境的桥接是本 task 的难点，单独一步聚焦能减少返工。HHOOK_PLACEHOLDER 的具体形态以实际 windows 0.61 API 为准（`HHOOK::default()` 或 `0 as _`）。

- [ ] **Step 2: 补全 thread_local 注入 + 回调真逻辑**

重写 `InputHook::new` 的线程体与两个回调，使其：

1. 线程启动后，把 `Arc<AtomicCounts>` / `Arc<Mutex<Vec<Instant>>>` / `Arc<AtomicI64>` 存入 thread_local（`CALLBACK_STATE`）。
2. `key_proc`：仅在 `wparam` 为 keydown 时，读 `KBDLLHOOKSTRUCT.vkCode`，`counts.keys += 1`；若 `vkCode == VK_BACK` 则 `backspaces += 1`；`key_times.lock().push(Instant::now())`（注意容量上限 64，满了丢最旧）；`last_input_ms` 写当前 ms。
3. `mouse_proc`：`wparam == WM_LBUTTONDOWN || WM_RBUTTONDOWN || WM_MBUTTONDOWN` 时 `counts.mouse += 1` 并更新 last_input；其余（移动、滚轮）不计 mouse 但仍更新 last_input（spec：任何输入重置空闲）。
4. 主线程通过共享 `Arc` 读取，`thread_id` 用 `GetCurrentThreadId`（在 hook 线程内调一次写回一个 `Arc<AtomicU32>`）。

> 实现细节由执行者按上述契约补全；本步**不写自动测试**（无法在无桌面 CI 跑），靠 Task 6 的 example 手工 QA 验证回调真的在计数。

- [ ] **Step 3: 验证编译**

Run: `cargo build`
Expected: 成功（warnings 可接受，但不应有 error）。

- [ ] **Step 4: 提交**

```bash
git add src/platform/input_hook.rs
git commit -m "feat(platform): low-level keyboard/mouse hooks on dedicated message-pump thread"
```

---

## Task 3: WindowTracker（前台窗口标题 + on_target）

**Files:**
- Modify: `src/platform/window_tracker.rs`

负责：调 `GetForegroundWindow` 拿 HWND，`GetWindowTextW` 读标题，用 `logic::title_matches` 判定，并检测 HWND 变化以计 `switches`。

- [ ] **Step 1: 实现 WindowTracker**

替换 `src/platform/window_tracker.rs` 全部内容：

```rust
//! 前台窗口跟踪（spec §4.1 WindowTracker）。
//!
//! 查询前台窗口标题，判定是否匹配目标串（on_target），并检测句柄变化以计切窗次数。
//! 不做 GetWindowRect —— rect 是 renderer 的事（见 controller.rs Frame 注释）。

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
};

use super::logic::{title_matches, Counts};

pub struct WindowTracker {
    target: String,
    last_hwnd: isize, // 0 表示尚无；用 isize 避免 HWND 跨调用生命周期麻烦
}

impl WindowTracker {
    pub fn new(target_window_title_contains: impl Into<String>) -> Self {
        Self {
            target: target_window_contains().into(), // 见下修正
            last_hwnd: 0,
        }
    }

    /// 查询当前前台窗口。返回 (on_target, 是否发生切换)。
    /// 若发生切换，调用方应 counts.add_switch()。
    pub fn poll(&mut self) -> (bool, bool) {
        let hwnd = unsafe { GetForegroundWindow() };
        let raw = hwnd.0 as isize;
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
```

> 上面的 `target_window_contains()` 是占位笔误，实现时直接用构造传入的 `target` 字段。修正：构造函数参数名用 `target_window_title_contains: impl Into<String>`，存为 `self.target`。

- [ ] **Step 2: 验证编译 + 跑 logic 单测确保没回归**

Run: `cargo build && cargo test --lib platform::logic`
Expected: 编译成功；logic 8 passed。

- [ ] **Step 3: 提交**

```bash
git add src/platform/window_tracker.rs
git commit -m "feat(platform): foreground window title query + on_target match + switch detection"
```

---

## Task 4: Win32SignalProvider（组装 + 实现 SignalProvider）

**Files:**
- Modify: `src/platform/mod.rs`

负责：把 `InputHook`（计数/key_times/last_input）+ `WindowTracker`（on_target/switched）+ `IdleTracker`（idle）组装，实现 `SignalProvider::snapshot`，按 §接口契约填 `TickInput`，并在 snapshot 后清零计数与 key_times。

- [ ] **Step 1: 补全 Win32SignalProvider 主体**

替换 `src/platform/mod.rs` 中 `Win32SignalProvider` 的占位结构体为完整实现：

```rust
impl Win32SignalProvider {
    /// 启动 hook 线程并初始化各子组件。失败返回 io::Error（hook 装不上）。
    pub fn new(target_window_title_contains: impl Into<String>) -> std::io::Result<Self> {
        let now = std::time::Instant::now();
        let hook = input_hook::InputHook::new()?;
        Ok(Self {
            hook,
            window: WindowTracker::new(target_window_title_contains),
            idle: logic::IdleTracker::new(now),
            counts: logic::Counts::default(),
            keys: logic::KeyIntervals::new(),
        })
    }
}

impl SignalProvider for Win32SignalProvider {
    fn snapshot(&mut self, dt: std::time::Duration) -> crate::model::types::TickInput {
        let now = std::time::Instant::now();

        // 1. 从 hook 线程拉取增量计数（原子 drain）。
        let mut inc = self.hook.counts.drain();

        // 2. 查前台窗口：on_target + 切窗检测（switch 计入本 tick 增量）。
        let (on_target, switched) = self.window.poll();
        if switched {
            inc.add_switch();
        }

        // 3. 空闲：把 hook 的 last_input 时间戳同步进 IdleTracker。
        //    hook 线程把每次输入的 Instant 写进共享 key_times 缓冲的最后位置
        //    不可靠（缓冲会被 drain），因此用专门的 last_input_ms 原子：转回 Instant。
        //    简化：直接让 hook 回调持有 IdleTracker 的 last_input_ms（ms since start），
        //    这里换算回 Duration 喂 IdleTracker::note_input（用 epoch 起点对齐）。
        //    见 input_hook 实现细节；此处假定 hook 已更新 IdleTracker 的语义等价物。
        let idle = self.idle.idle_since(now);

        // 4. 击键间隔：从 hook 线程的 key_times 缓冲 drain，转成 ms。
        let intervals_ms = {
            let mut guard = self.hook.key_times.lock().unwrap();
            // 用缓冲里的 Instant 直接算间隔（与 logic::KeyIntervals 同语义）。
            let times = std::mem::take(&mut *guard);
            drop(guard);
            intervals_from(&times)
        };

        // 5. 刷新 IdleTracker：本 tick 内若有任何输入，把 last_input 推到 now。
        if inc.keys > 0 || inc.mouse > 0 {
            self.idle.note_input(now);
        }

        crate::model::types::TickInput {
            counts: crate::model::types::TickCounts {
                keys: inc.keys,
                mouse: inc.mouse,
                switches: inc.switches,
                backspaces: inc.backspaces,
            },
            on_target,
            idle,
            dt,
            key_intervals_ms: intervals_ms,
        }
    }
}

fn intervals_from(times: &[std::time::Instant]) -> Vec<f64> {
    let mut out = Vec::with_capacity(times.len().saturating_sub(1));
    for w in times.windows(2) {
        let ms = w[1].saturating_duration_since(w[0]).as_secs_f64() * 1000.0;
        out.push(ms);
    }
    out
}
```

> **IdleTracker 一致性注**：上面 §3 的「last_input 同步」是本 task 的精确性关键。两种可选实现，执行者择一并在注释里写死：
> - **方案 A（推荐）**：hook 回调直接写 `Arc<AtomicI64>`（ms since 进程起点）；snapshot 读出，减去「now 的 ms」得到负的 idle 偏移……实际更干净的做法是让 `Win32SignalProvider` 自己持有一个 `Arc<AtomicI64>` 给 hook，snapshot 时 `idle = now_ms - last_input_ms`，**不**用 `IdleTracker` 结构（IdleTracker 留给纯逻辑测试）。
> - 方案 B：保留 IdleTracker，hook 回调无法直接写它的 Instant，需用共享 `Arc<Mutex<Option<Instant>>>`。锁开销略高。
> 建议方案 A，IdleTracker 仅作为 logic.rs 的可测纯逻辑保留（证明 idle 数学正确），生产路径用原子 ms。实现时把 snapshot 的 idle 计算改成读 `self.hook.last_input_ms`，并删去 snapshot 里对 `self.idle` 的调用（IdleTracker 字段可移除或保留作测试夹具）。

- [ ] **Step 2: 按 IdleTracker 一致性注收敛实现**

按方案 A 重写 snapshot 的 idle 段落：移除对 `self.idle` 的依赖，改读 `self.hook.last_input_ms`。`IdleTracker` 留在 logic.rs 作为可测纯逻辑（不删其测试）。

- [ ] **Step 3: 验证编译**

Run: `cargo build`
Expected: 成功。

- [ ] **Step 4: 提交**

```bash
git add src/platform/mod.rs
git commit -m "feat(platform): assemble Win32SignalProvider implementing SignalProvider"
```

---

## Task 5: platform_demo example（真实 hook 验证）

**Files:**
- Create: `examples/platform_demo.rs`

运行 ~30 秒，每秒打印 counts / idle / 前台标题 / on_target，证明 platform 层端到端走通。手工 QA，无自动断言。

- [ ] **Step 1: 写 example**

`examples/platform_demo.rs`：

```rust
//! Plan 2 验证用：连真实 Windows hook 跑 30 秒，打印每秒信号。
//! Run: cargo run --example platform_demo
//! （会触发真实键鼠 hook + 前台窗口查询；按 Ctrl+C 提前结束）

#[cfg(windows)]
fn main() -> std::io::Result<()> {
    use std::time::{Duration, Instant};
    use attention_blackhole::controller::SignalProvider;
    use attention_blackhole::platform::Win32SignalProvider;

    let target = std::env::args().nth(1).unwrap_or_else(|| "Visual Studio Code".into());
    println!("starting platform_demo; target = {:?}; go type/move for 30s", target);
    println!("sec | keys mouse sw bk | idle_ms | on_target | title");

    let mut prov = Win32SignalProvider::new(&target)?;
    let dt = Duration::from_millis(1000); // 1Hz snapshot 够 demo
    let start = Instant::now();
    let mut sec = 0u64;
    while start.elapsed() < Duration::from_secs(30) {
        let input = prov.snapshot(dt);
        // 标题不在 TickInput 里（有意），这里只报 on_target + counts + idle
        println!(
            "{:>3} | {:>4} {:>5} {:>2} {:>2} | {:>7} | {:>8} |",
            sec,
            input.counts.keys,
            input.counts.mouse,
            input.counts.switches,
            input.counts.backspaces,
            input.idle.as_millis(),
            input.on_target,
        );
        sec += 1;
    }
    println!("done.");
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("platform_demo is Windows-only.");
}
```

- [ ] **Step 2: 运行验证（手工 QA）**

Run: `cargo run --example platform_demo`
QA 清单（人眼核对）：
- [ ] 启动不报错、不 panic。
- [ ] 在 30 秒内敲键盘 → `keys` 列非零且随敲击增长。
- [ ] 点击鼠标（左/右/中）→ `mouse` 列非零；**仅移动鼠标不应增长 mouse**（验证移动不计）。
- [ ] 切换前台窗口（Alt+Tab）→ `sw` 列在某些行非零。
- [ ] 敲退格 → `bk` 列非零。
- [ ] 停手不动几秒 → `idle_ms` 持续增长；一敲键立即回落到小值。
- [ ] 把 VS Code（或你设的 target）置前 → `on_target` 显示 true；切到别的窗口 → false。
- [ ] 进程 30 秒后正常退出（Drop 干净卸 hook、join 线程，无卡死/泄漏报错）。

- [ ] **Step 3: 提交**

```bash
git add examples/platform_demo.rs
git commit -m "examples: platform_demo verifying real input hooks + window tracking"
```

---

## 完成标准（Plan 2 Definition of Done）

- `cargo test` 全绿：Plan 1 的 30 个测试 + Plan 2 logic.rs 的 8 个单测。非 windows 平台 `cargo build` 仍通过（platform 模块 cfg-gated）。
- `cargo run --example platform_demo` 在真实 Windows 上能观察到键鼠计数、切窗计数、退格计数、idle 增长/重置、on_target 切换，且 30 秒后干净退出。
- `controller.rs` **未引入任何 `use crate::platform`**（平台类型只通过 `&mut dyn SignalProvider` 进入）。
- 隐私硬规则满足：hook 回调只计数 + 记 Instant/last_input_ms，不存任何按键内容（vkCode 仅与 `VK_BACK` 比较）。代码注释显式声明。
- `Win32SignalProvider` 可替换 Plan 1 的 `ScriptedProvider` 喂给同一个 `Controller`，得到真实信号驱动的 load/state 流转。
- `windows` 依赖仅启用了 `Win32_Foundation` + `Win32_UI_WindowsAndMessaging` 两个 feature（除非实现中发现必须的额外 API，且按最小原则补）。

---

## 终审遗留项（final review 后填写，格式参考 Plan 1）

终审（实现完成自检，2026-06-18）结论 **GO**，DoD 全部满足：`cargo test` 34 passed、`platform_demo` 在前台终端跑通真实键鼠 hook（keys/mouse/sw/bk/idle/on_target/intervals 全部正确）。以下 should-fix 留给后续 plan：

1. **Windows SDK 版本漂移**（→ Plan 3+）：开发机装的是 Win11 SDK `10.0.22621.0`，但 `windows` crate 链接的 import lib 来自 `windows-link`（与 SDK 无关）。Plan 3 接入 D3D11/WGC 时需确认 `d3d11.lib`/`d3dcompiler.lib` 由哪个 feature 提供，可能要补 `Win32_Graphics_Direct3D11` 等 feature。

2. **`platform_demo` 必须前台运行**（→ 已在 example 文档注明）：低级键盘 hook 的回调由系统派发给「安装线程」，但该进程必须处于可接收输入的状态。在 agent 的后台无 tty shell 中运行时，键盘回调不触发（`key_cb` 全程 0）；在用户自己的前台终端运行则完全正常。Plan 5 的 Tauri 外壳是 GUI 进程，天然满足此前提，无此问题。

3. **`KeyIntervals` / `IdleTracker` 未被生产代码使用**（→ 保持现状）：Plan 2 实现时按方案 A 用 `last_input_ms` 原子 + `intervals_from` 函数直接算，logic.rs 的 `IdleTracker`/`KeyIntervals` 结构沦为「纯逻辑可测基准」（模块级 `allow(dead_code)`）。它们的单测仍验证数学正确性，保留作对照；若 Plan 5 引入配置加载时校验 idle 数学，可复用。

4. **诊断 API 留在库里**（→ Plan 5 决定是否移除）：`Win32SignalProvider::hook_diag` / `cb_fired` / `key_cb` 是排查 hook 不工作时的关键信号，保留为 `pub`。Plan 5 接入托盘/日志后，可考虑把它们接到日志而非裸暴露。

5. **环境前提已固化**（→ 写进 README/CONTRIBUTING，非代码）：本仓库 `rust-toolchain.toml` pin MSVC；需 `Visual Studio Build Tools` 含 `C++ 桌面开发` workload（VC Tools + Windows SDK）；`RUSTUP_HOME`/`CARGO_HOME` 建议重定向到非 C 盘（C 盘空间易不足）。这些在实现 Plan 2 时踩过，应记入项目入门文档。
