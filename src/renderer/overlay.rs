//! 透明置顶全屏 overlay 窗口（spec §4.1 / §8.3）。
//!
//! 创建覆盖主显示器全屏的窗口，样式 `WS_EX_LAYERED|WS_EX_TRANSPARENT|WS_EX_TOPMOST` +
//! `WS_POPUP`：点击穿透（鼠标穿过到下层）、永远置顶。DWM 逐像素 alpha 合成靠
//! `DwmExtendFrameIntoClientArea(margin={-1})` —— backbuffer 的 alpha 通道直接决定
//! 每像素透明度，D3D11 blend 输出预乘 alpha 即可让黑洞「悬浮」在桌面之上。
//!
//! 消息泵跑在渲染线程（与 Plan 2 hook 线程同理：PeekMessage + DispatchMessage）。
//! WS_EX_TRANSPARENT 让 hit-test 永远透传，故本窗口收不到鼠标输入——
//! ForcedBreak 的真实输入捕获留 Plan 5（届时切样式为非透明+捕获）。
//!
//! **捕获自排除**：建窗后立即 `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`，
//! 把 overlay 从任何 WGC 屏幕捕获中排除——否则捕获全屏显示器会把 overlay 自身（黑洞
//! 渲染结果）也捕获进来，形成无限自指反馈环。排除后 WGC 捕获纹理里 overlay 区域为
//! 透明，shader 采样到的就是 overlay 背后的真实桌面，引力透镜才扭曲真实屏幕内容。
//!
//! NOTE: Task 1 阶段为 dead_code，Task 5 由 D3D11Renderer 接入后此 allow 可移除。
#![allow(dead_code)]

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::WindowsAndMessaging::{
    self as wm, CreateWindowExW, DefWindowProcW, DispatchMessageW, LoadCursorW, PeekMessageW,
    PostThreadMessageW, RegisterClassExW, SetWindowDisplayAffinity, TranslateMessage, HCURSOR,
    IDC_ARROW, MSG, PM_REMOVE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_DESTROY, WM_NCCREATE, WM_QUIT,
    WM_TIMER, WNDCLASSEXW, WDA_EXCLUDEFROMCAPTURE, WS_EX_LAYERED, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP,
};

use super::RenderError;

/// Overlay 窗口的客户区像素尺寸（主显示器分辨率）。
#[derive(Debug, Clone, Copy)]
pub struct OverlaySize {
    pub width: u32,
    pub height: u32,
}

/// 已创建的 overlay：持有 HWND 与负责消息泵的后台线程句柄。
///
/// Drop 时 `PostThreadMessageW(WM_QUIT)` 退出泵并 join，与 `InputHook` 同模式。
pub struct OverlayWindow {
    pub hwnd: HWND,
    /// 客户区像素尺寸（SwapChain backbuffer 大小）。
    pub size: OverlaySize,
    thread_id: u32,
    handle: Option<JoinHandle<()>>,
}

impl OverlayWindow {
    /// 创建全屏透明置顶窗口，并在专用线程跑消息泵。
    ///
    /// 返回 (hwnd, size)：D3D11Context 用 hwnd 绑 SwapChain，用 size 定 backbuffer。
    pub fn new() -> Result<Self, RenderError> {
        // 主显示器分辨率：SM_CXSCREEN/SM_CYSCREEN。多显示器扩展留后续 plan。
        let cx = unsafe { wm::GetSystemMetrics(wm::SM_CXSCREEN) };
        let cy = unsafe { wm::GetSystemMetrics(wm::SM_CYSCREEN) };
        if cx <= 0 || cy <= 0 {
            return Err(RenderError::Windows(windows::core::Error::from(
                windows::core::HRESULT(-1),
            )));
        }
        let size = OverlaySize {
            width: cx as u32,
            height: cy as u32,
        };

        // 线程间传 HWND：HWND 不 Send（含裸指针），故用 AtomicUsize 存其原始指针值，
        // 取回时再包回 HWND。与 WindowTracker 用 isize 比较 HWND 同一规避手法。
        let hwnd_raw = Arc::new(AtomicUsize::new(0));
        let thread_id = Arc::new(AtomicU32::new(0));
        // 诊断：泵线程进度（0 未启 1 注册类 2 建窗 3 DWM 4 进泵 99 失败）。
        let diag = Arc::new(AtomicU32::new(0));

        let hwnd_t = hwnd_raw.clone();
        let tid_t = thread_id.clone();
        let diag_t = diag.clone();

        let handle = thread::Builder::new()
            .name("abh-overlay".into())
            .spawn(move || {
                tid_t.store(
                    unsafe { windows::Win32::System::Threading::GetCurrentThreadId() },
                    Ordering::SeqCst,
                );

                let result = unsafe { create_overlay_window(&hwnd_t, &diag_t) };
                if let Err(e) = result {
                    diag_t.store(99, Ordering::SeqCst);
                    eprintln!("abh-overlay: create failed: {:?}", e);
                    return;
                }

                // 消息泵：PeekMessage 主动轮询（与 InputHook 同理，避免 GetMessage 阻塞
                // 导致 D3D11 Present 无法在同线程推进）。本窗口 WS_EX_TRANSPARENT，
                // 几乎收不到鼠标输入；泵主要服务于系统重绘/定时器/退出消息。
                let mut msg = MSG::default();
                diag_t.store(4, Ordering::SeqCst);
                loop {
                    unsafe {
                        if PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                            if msg.message == WM_QUIT {
                                break;
                            }
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        } else {
                            // 无消息时短暂让出，保持对 Present（同线程）与 hook 派发的响应。
                            thread::sleep(std::time::Duration::from_millis(5));
                        }
                    }
                }
            })
            .map_err(|e| RenderError::Windows(windows::core::Error::from(e)))?;

        // 等泵线程登记 thread_id（极快，spinwait 可接受）。
        let mut waited = 0u32;
        while thread_id.load(Ordering::SeqCst) == 0 && waited < 1_000_000 {
            thread::yield_now();
            waited += 1;
        }
        // 等窗口创建完成（hwnd_raw 写入或 diag 标记失败）。
        let hwnd = loop {
            let raw = hwnd_raw.load(Ordering::SeqCst);
            if raw != 0 {
                break HWND(raw as *mut std::ffi::c_void);
            }
            let d = diag.load(Ordering::SeqCst);
            if d == 99 {
                let _ = handle.join();
                return Err(RenderError::Windows(windows::core::Error::from(
                    windows::core::HRESULT(-1),
                )));
            }
            thread::yield_now();
        };

        Ok(Self {
            hwnd,
            size,
            thread_id: thread_id.load(Ordering::SeqCst),
            handle: Some(handle),
        })
    }

    /// 诊断：泵线程进度（4=进泵就绪，99=失败）。
    pub fn pump_ready(&self) -> bool {
        // handle 创建后窗口已 ready（new 已确认），保留方法供将来调试。
        self.thread_id != 0
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        if self.thread_id != 0 {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// 泵线程内执行：注册类 + 建窗 + DWM 透明合成。失败返回 Error。
///
/// # Safety
/// 调用方必须在创建窗口的线程上下文中（CreateWindow 的窗口属于调用线程，
/// 消息泵也必须跑在同一线程）。
unsafe fn create_overlay_window(
    hwnd_raw: &Arc<AtomicUsize>,
    diag: &Arc<AtomicU32>,
) -> Result<(), windows::core::Error> {
    let class_name = w!("AttentionBlackholeOverlay");

    let mut class = WNDCLASSEXW::default();
    class.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
    class.lpfnWndProc = Some(wnd_proc);
    class.hInstance = HINSTANCE(get_module_handle());
    class.lpszClassName = class_name;
    class.hCursor = LoadCursorW(None, IDC_ARROW).ok().unwrap_or(HCURSOR(std::ptr::null_mut()));

    // RegisterClassExW 返回 ATOM（u16）；0 表示失败。
    let atom = RegisterClassExW(&class);
    if atom == 0 {
        return Err(windows::core::Error::from_win32());
    }

    diag.store(1, Ordering::SeqCst);

    let cx = wm::GetSystemMetrics(wm::SM_CXSCREEN);
    let cy = wm::GetSystemMetrics(wm::SM_CYSCREEN);

    // ex_style：
    // - WS_EX_LAYERED：逐像素 alpha 合成的基础（配合 DWM）。
    // - WS_EX_TRANSPARENT：hit-test 透传 → 鼠标点击穿过到下层（点击穿透）。
    // - WS_EX_TOPMOST：永远置顶。
    //
    // 注意：曾用 WS_EX_NOREDIRECTIONBITMAP（跳过 GDI 重定向位图，D3D11 直接合成），
    // 但它在部分显卡驱动 / 旧 Win10 上 swapchain 内容不被 DWM 合成（实测本机看不到
    // 任何输出，即使 Present 返回 S_OK）。故改回经典方案：纯 LAYERED + DWM 扩展框架
    // （margins=-1）+ 普通 swapchain，兼容性最广。backbuffer 的 alpha 经 DWM 逐像素合成。
    let ex_style = WINDOW_EX_STYLE(WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0 | WS_EX_TOPMOST.0);
    let style = WINDOW_STYLE(WS_POPUP.0);

    let hwnd = CreateWindowExW(
        ex_style,
        class_name,
        w!("Attention Blackhole"),
        style,
        0, // x
        0, // y
        cx,
        cy,
        None, // hwndParent
        None, // hMenu
        Some(class.hInstance),
        None, // lpParam
    )?;

    diag.store(2, Ordering::SeqCst);

    // DWM 逐像素 alpha：margin={-1} 把「扩展框架」覆盖整个客户区，
    // 使 swapchain backbuffer 的 alpha 通道成为合成 alpha（经典 LAYERED + DWM 方案，
    // 兼容性最广；放弃 NOREDIRECTIONBITMAP 因其在部分驱动上不合成）。
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    DwmExtendFrameIntoClientArea(hwnd, &margins)?;

    // LAYERED 窗口默认完全透明（alpha=0），需设一次全局 alpha=255（完全不透明），
    // 让 DWM 的逐像素 alpha 接管——否则窗口永远 alpha=0 不可见。
    // crKey=0(黑) + LWA_ALPHA：按 alpha 值合成，颜色 key 不生效。
    wm::SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), 255, wm::LWA_ALPHA)?;

    diag.store(3, Ordering::SeqCst);

    // 显式 ShowWindow：WS_POPUP 默认不可见。
    let _ = wm::ShowWindow(hwnd, wm::SW_SHOWNORMAL);

    // 把 overlay 自身从 WGC 屏幕捕获中排除，打破「捕获包含 overlay → overlay 画捕获 → 无限
    // 自指」的反馈环。这样 WGC 捕获本显示器时，overlay 区域在捕获纹理里是透明/黑，
    // shader 采样到的就是 overlay **背后**的真实桌面，引力透镜才能扭曲真实屏幕内容。
    // WDA_EXCLUDEFROMCAPTURE 需 Win10 2004+（本项目已要求）；旧 OS 退化为 WDA_MONITOR
    // （overlay 区域捕获为黑块），不致命——失败仅记日志，窗口仍正常显示。
    if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).is_err() {
        eprintln!(
            "abh-overlay: SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) failed; \
             real-screen-capture will include this overlay (feedback-loop risk)"
        );
    }

    // 把 HWND 原始指针值写回，供 new 取回（AtomicUsize 可跨线程）。
    hwnd_raw.store(hwnd.0 as usize, Ordering::SeqCst);
    Ok(())
}

/// 取当前进程模块句柄（窗口类的 hInstance）。
unsafe fn get_module_handle() -> *mut std::ffi::c_void {
    windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
        .map(|h| h.0 as *mut std::ffi::c_void)
        .unwrap_or(std::ptr::null_mut())
}

/// 默认窗口过程。本窗口不处理任何自定义逻辑（WS_EX_TRANSPARENT 让输入透传），
/// 仅走 DefWindowProc。WM_NCCREATE 必须返回 TRUE 否则 CreateWindow 失败。
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        return LRESULT(1);
    }
    if msg == WM_DESTROY {
        // 不主动 PostQuitMessage —— 退出由 Drop 的 PostThreadMessage(WM_QUIT) 驱动。
        return LRESULT(0);
    }
    // WM_TIMER 引用以表明已识别该消息常量（占位，未设置任何定时器）。
    let _ = WM_TIMER;
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
