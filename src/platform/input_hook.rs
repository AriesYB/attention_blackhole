//! 低级键鼠 hook（spec §4.1 InputHook）。
//!
//! 在专用线程上安装 WH_KEYBOARD_LL / WH_MOUSE_LL 并跑消息泵。回调只做三件事：
//! 自增计数、记击键时间戳、刷新 last_input。绝不保留按键内容
//! （vkCode 仅用于判定 VK_BACK，不存储）。见 spec §9 隐私硬规则。
//!
//! 线程模型：低级 hook 回调只在「装了 hook 且该线程在跑消息泵」的线程触发。
//! 因此 InputHook 起一个专用线程，在那里 SetWindowsHookExW + GetMessageW 循环。
//! Drop 时 PostThreadMessageW(WM_QUIT) 退出泵，join 线程。

use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    self as wm, CallNextHookEx, DispatchMessageW, KBDLLHOOKSTRUCT, MSLLHOOKSTRUCT,
    PeekMessageW, PM_REMOVE, TranslateMessage, HHOOK, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_QUIT, WM_RBUTTONDOWN, WM_SYSKEYDOWN,
};

use super::logic::Counts;

const VK_BACK: i32 = 0x08;

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
    pub fn drain(&self) -> Counts {
        Counts {
            keys: self.keys.swap(0, Ordering::Relaxed) as u32,
            mouse: self.mouse.swap(0, Ordering::Relaxed) as u32,
            switches: self.switches.swap(0, Ordering::Relaxed) as u32,
            backspaces: self.backspaces.swap(0, Ordering::Relaxed) as u32,
        }
    }
}

/// 低级 hook 回调能访问到的共享状态（通过 thread_local 注入）。
struct CallbackState {
    counts: Arc<AtomicCounts>,
    /// 击键时间戳缓冲：回调 push，主线程 snapshot 时 drain。上限 64。
    key_times: Arc<Mutex<Vec<Instant>>>,
    /// 最后一次输入的毫秒偏移（相对进程起点 Instant::now()），供 idle 计算。
    last_input_ms: Arc<AtomicI64>,
    start: Instant,
}

thread_local! {
    static CALLBACK_STATE: std::cell::RefCell<Option<CallbackState>> = const { std::cell::RefCell::new(None) };
}

const MAX_KEYS: usize = 64;

/// InputHook：后台线程持有 hook，Drop 时优雅退出。
pub struct InputHook {
    thread_id: u32,
    handle: Option<JoinHandle<()>>,
    /// 共享给外部的计数与时间戳缓冲（Win32SignalProvider::snapshot 读）。
    pub counts: Arc<AtomicCounts>,
    pub key_times: Arc<Mutex<Vec<Instant>>>,
    pub last_input_ms: Arc<AtomicI64>,
    /// 诊断：hook 线程进度（0 未启 1 注入状态 2 键盘hook 3 鼠标hook 4 进泵 99 失败）。
    pub diag: Arc<AtomicU32>,
}

impl InputHook {
    /// 启动 hook 线程。
    pub fn new() -> std::io::Result<Self> {
        let counts = Arc::new(AtomicCounts::default());
        let key_times = Arc::new(Mutex::new(Vec::with_capacity(MAX_KEYS)));
        let last_input_ms = Arc::new(AtomicI64::new(0));
        let thread_id = Arc::new(AtomicU32::new(0));
        // 诊断：hook 线程执行进度（供排查为何不计数）。
        // 0=未启动 1=已注入状态 2=键盘hook装上 3=鼠标hook装上 4=进入泵 99=失败
        let diag = Arc::new(AtomicU32::new(0));

        let counts_t = counts.clone();
        let key_times_t = key_times.clone();
        let last_t = last_input_ms.clone();
        let thread_id_t = thread_id.clone();
        let diag_t = diag.clone();

        let handle = thread::Builder::new()
            .name("abh-input-hook".into())
            .spawn(move || {
                // 记录本线程 id 供主线程退出用。
                thread_id_t.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);

                // 把共享状态挂到 thread_local，供 C extern 回调读取。
                CALLBACK_STATE.with(|cs| {
                    *cs.borrow_mut() = Some(CallbackState {
                        counts: counts_t.clone(),
                        key_times: key_times_t.clone(),
                        last_input_ms: last_t.clone(),
                        start: Instant::now(),
                    });
                });
                diag_t.store(1, Ordering::SeqCst);

                unsafe {
                    // LL hook 要求 hMod；用当前进程模块句柄（全局 hook 不注入其它进程，
                    // 传进程模块即可）。文档建议传模块句柄而非 NULL 以保证稳定。
                    // GetModuleHandleW 返回 HMODULE；SetWindowsHookExW 要 Option<HINSTANCE>。
                    // 两者底层都是指针，通过 .into() / as 转换；失败则传 None（NULL）。
                    let hmod = GetModuleHandleW(None)
                        .ok()
                        .map(|m| windows::Win32::Foundation::HINSTANCE(m.0));

                    let kb = match wm::SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_proc), hmod, 0) {
                        Ok(h) => h,
                        Err(_) => {
                            diag_t.store(99, Ordering::SeqCst);
                            return;
                        }
                    };
                    diag_t.store(2, Ordering::SeqCst);
                    let ms = match wm::SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hmod, 0) {
                        Ok(h) => h,
                        Err(_) => {
                            let _ = wm::UnhookWindowsHookEx(kb);
                            diag_t.store(99, Ordering::SeqCst);
                            return;
                        }
                    };
                    diag_t.store(3, Ordering::SeqCst);

                    // 标准消息泵：PeekMessage(PM_REMOVE) 主动轮询 + TranslateMessage +
                    // DispatchMessage。LL hook 回调仅在「安装线程正在 dispatch 消息」时
                    // 由系统触发；若线程长时间阻塞（LowLevelHooksTimeout 默认 300ms），
                    // Windows 会静默移除 hook。故用 PeekMessage 主动取消息而非 GetMessage
                    // 阻塞，并在无消息时短暂 sleep 让线程保持响应。
                    let mut msg = wm::MSG::default();
                    diag_t.store(4, Ordering::SeqCst);
                    loop {
                        if PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).into() {
                            if msg.message == WM_QUIT {
                                break;
                            }
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        } else {
                            // 无消息：短暂让出，避免忙等，同时保持线程对 hook 派发的响应性。
                            thread::sleep(std::time::Duration::from_millis(5));
                        }
                    }

                    let _ = wm::UnhookWindowsHookEx(kb);
                    let _ = wm::UnhookWindowsHookEx(ms);
                }
            })?;

        // 等 hook 线程登记完 thread_id（线程启动极快，spinwait 可接受）。
        let mut waited = 0u32;
        while thread_id.load(Ordering::SeqCst) == 0 && waited < 1_000_000 {
            thread::yield_now();
            waited += 1;
        }

        Ok(Self {
            thread_id: thread_id.load(Ordering::SeqCst),
            handle: Some(handle),
            counts,
            key_times,
            last_input_ms,
            diag,
        })
    }
}

/// 记一次输入：刷新 last_input_ms。
fn note_input(state: &CallbackState) {
    let ms = state.start.elapsed().as_millis() as i64;
    state.last_input_ms.store(ms, Ordering::Relaxed);
}

/// 记一次击键：计数 + push 时间戳 + note_input。
fn note_key(state: &CallbackState, is_backspace: bool) {
    state.counts.keys.fetch_add(1, Ordering::Relaxed);
    if is_backspace {
        state.counts.backspaces.fetch_add(1, Ordering::Relaxed);
    }
    if let Ok(mut guard) = state.key_times.lock() {
        if guard.len() >= MAX_KEYS {
            guard.remove(0); // 滑动窗口：满则丢最旧
        }
        guard.push(Instant::now());
    }
    note_input(state);
}

/// 诊断：回调被系统调用的总次数（key+mouse，不分类型）。
/// 放在 thread_local 访问之前自增，用于隔离「回调没触发」vs「thread_local 读不到」。
pub static CB_FIRED: AtomicI64 = AtomicI64::new(0);
/// 诊断：key_proc 被调用的次数（隔离键盘回调是否触发）。
pub static KEY_CB: AtomicI64 = AtomicI64::new(0);

unsafe extern "system" fn key_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    CB_FIRED.fetch_add(1, Ordering::Relaxed);
    KEY_CB.fetch_add(1, Ordering::Relaxed);
    // 仅 keydown 计一次，避免 up 重复。
    let is_down = wparam.0 == WM_KEYDOWN as usize || wparam.0 == WM_SYSKEYDOWN as usize;
    if is_down {
        // lparam 指向 KBDLLHOOKSTRUCT。只取 vkCode 判 VK_BACK，不存储。
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let is_backspace = kb.vkCode as i32 == VK_BACK;
        CALLBACK_STATE.with(|cs| {
            if let Some(state) = cs.borrow().as_ref() {
                note_key(state, is_backspace);
            }
        });
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    CB_FIRED.fetch_add(1, Ordering::Relaxed);
    // 任意鼠标输入都刷新 last_input（spec §6.5：任何输入重置空闲）。
    // 但只有 button-down 计 mouse 计数（移动不计，避免淹没）。
    let is_button_down = wparam.0 == WM_LBUTTONDOWN as usize
        || wparam.0 == WM_RBUTTONDOWN as usize
        || wparam.0 == WM_MBUTTONDOWN as usize;

    CALLBACK_STATE.with(|cs| {
        if let Some(state) = cs.borrow().as_ref() {
            if is_button_down {
                state.counts.mouse.fetch_add(1, Ordering::Relaxed);
            }
            note_input(state);
        }
    });
    // lparam 未用（MSLLHOOKSTRUCT），保留 size_of 引用以证明类型已知。
    let _ = std::mem::size_of::<MSLLHOOKSTRUCT>();
    CallNextHookEx(None, code, wparam, lparam)
}

impl Drop for InputHook {
    fn drop(&mut self) {
        if self.thread_id != 0 {
            unsafe {
                let _ = wm::PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// 抑制未使用警告（HHOOK 在回调签名推导中被引用，但代码里未显式出现）。
#[allow(dead_code)]
type _UseHHOOK = HHOOK;
