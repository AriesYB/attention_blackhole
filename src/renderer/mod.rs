//! D3D11 渲染器（spec §4.1 renderer/）。仅 `cfg(windows)`。
//!
//! 分层纪律：controller 经 `&mut dyn Renderer` 消费本模块，**不反向** `use crate::renderer`。
//! `D3D11Renderer` 与 mock/`CollectingRenderer` 同路进入 controller。
//!
//! 子模块拆分（spec §4.1）：
//! - `overlay` —— 透明置顶全屏窗口（Task 2）
//! - `shader`  —— 黑洞 HLSL + 常量缓冲（Task 3）
//! - `capture` —— WGC 捕获 + 程序化降级 + on_target/consent 门控（Task 4）
//! - `painter` —— 每帧编排：Frame→uniform + 全屏三角形 + Present（Task 5）
//!
//! 真正绘制在独立 60fps 线程，`Renderer::render` 只经 channel 推 Frame（非阻塞），
//! 渲染卡顿不拖慢 controller 的 10Hz tick。见 spec §8.3。

pub mod d3d;
pub mod overlay; // Task 2
pub mod shader; // Task 3
pub mod capture; // Task 4
pub mod painter; // Task 5

/// renderer 模块统一错误类型。D3D11/WGC 的 windows::core::Error 统一包成此类型，
/// 降级路径（无头/旧 OS）据此回落程序化背景。controller 不感知错误细节——
/// renderer 构造失败即返回 Err，由上层（Plan 5 外壳）决定是否退化为无 overlay。
#[derive(Debug)]
pub enum RenderError {
    /// D3D11 设备创建失败（硬件 + WARP 两条路都失败）。
    DeviceCreate {
        hardware: windows::core::Error,
        warp: windows::core::Error,
    },
    /// SwapChain / DXGI 路径失败。
    Dxgi(windows::core::Error),
    /// 其它 windows::core::Error。
    Windows(windows::core::Error),
}

impl From<windows::core::Error> for RenderError {
    fn from(e: windows::core::Error) -> Self {
        RenderError::Windows(e)
    }
}

// -----------------------------------------------------------------------------
// D3D11Renderer：组合 overlay + device + shaders + capture，impl Renderer。
// 真正绘制在独立 60fps 线程；Renderer::render 经 channel 推 Frame（非阻塞）。
// -----------------------------------------------------------------------------

use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use windows::Win32::Foundation::HWND;

use capture::CaptureSource;
use crate::controller::{Frame, Renderer};

/// D3D11 渲染器。
///
/// 构造时建立 overlay 窗口 + D3D11 设备管线 + 编译 shader + cbuffer + 捕获源，
/// 启动独立渲染线程（~60fps）。`render(&mut self, frame)` 只更新共享的「最新 Frame」，
/// 渲染线程读最新 Frame 编排绘制。渲染卡顿不拖慢 controller 的 10Hz tick（spec §8.3）。
pub struct D3D11Renderer {
    /// 渲染线程读的最新 Frame（render 写，线程读）。
    latest: Arc<Mutex<Option<Frame>>>,
    /// 渲染线程句柄。Drop 时经 stop flag + join 退出。
    handle: Option<JoinHandle<()>>,
    /// 停止信号。
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl D3D11Renderer {
    /// 构造渲染器。
    ///
    /// - `target_hwnd`：注意力目标窗口（WGC 捕获源；None 或 consent=false 时走程序化）。
    /// - `consent`：用户是否同意屏幕捕获。false → 永远用 ProceduralCaptureSource。
    ///
    /// 所有 D3D11/WGC 资源（含 overlay HWND）在**渲染线程内**创建并独占——这些资源
    /// 非 Send（含 HWND 裸指针），故不在主线程创建后 move，而是经初始化结果回传。
    pub fn new(target_hwnd: Option<HWND>, consent: bool) -> Result<Self, RenderError> {
        let latest = Arc::new(Mutex::new(None::<Frame>));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // 初始化结果：线程创建完资源后写 Ok，失败写 Err。主线程 spin 等它就绪。
        let init_result: Arc<Mutex<Option<Result<(), RenderError>>>> = Arc::new(Mutex::new(None));

        let latest_t = latest.clone();
        let stop_t = stop.clone();
        let init_t = init_result.clone();

        // HWND 非 Send（裸指针）：转 usize 跨线程传递，线程内转回。
        let target_hwnd_raw = target_hwnd.map(|h| h.0 as usize);

        let handle = thread::Builder::new()
            .name("abh-render".into())
            .spawn(move || {
                let target_hwnd = target_hwnd_raw.map(|raw| HWND(raw as *mut std::ffi::c_void));
                // 线程内创建全部资源（非 Send，独占本线程）。
                let started = match start_render_resources(target_hwnd, consent) {
                    Ok(res) => {
                        *init_t.lock().unwrap() = Some(Ok(()));
                        res
                    }
                    Err(e) => {
                        *init_t.lock().unwrap() = Some(Err(e));
                        return;
                    }
                };
                render_loop(started, latest_t, stop_t);
            })
            .map_err(|e| RenderError::Windows(windows::core::Error::from(e)))?;

        // 等初始化完成（Ok 或 Err），失败则 join 并返回。
        let init = loop {
            let mut g = init_result.lock().unwrap();
            if g.is_some() {
                break g.take().unwrap();
            }
            drop(g);
            thread::yield_now();
        };
        if let Err(e) = init {
            let _ = handle.join();
            return Err(e);
        }

        Ok(Self {
            latest,
            handle: Some(handle),
            stop,
        })
    }
}

impl Renderer for D3D11Renderer {
    fn render(&mut self, frame: &Frame) {
        // 非阻塞：只更新最新 Frame，渲染线程自会取用。
        *self.latest.lock().unwrap() = Some(*frame);
    }
}

impl Drop for D3D11Renderer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// 渲染线程内创建的所有独占资源。
struct RenderResources {
    _overlay: overlay::OverlayWindow,
    d3d: d3d::D3D11Context,
    shaders: shader::Shaders,
    cbuffer: windows::Win32::Graphics::Direct3D11::ID3D11Buffer,
    rtv: windows::Win32::Graphics::Direct3D11::ID3D11RenderTargetView,
    capture: Box<dyn CaptureSource + Send>,
}

/// 在渲染线程内创建 overlay + device + shaders + cbuffer + capture。
/// 这些资源非 Send，故必须在将使用它们的线程内构造。
fn start_render_resources(
    target_hwnd: Option<HWND>,
    consent: bool,
) -> Result<RenderResources, RenderError> {
    // 1. overlay 窗口（全屏透明置顶，提供 HWND + 尺寸）。
    let overlay = overlay::OverlayWindow::new()?;
    let hwnd = overlay.hwnd;
    let (w, h) = (overlay.size.width, overlay.size.height);

    // 2. D3D11 设备 + SwapChain（绑定 overlay HWND）。
    let d3d = d3d::D3D11Context::new(hwnd, w, h)?;

    // 3. 编译 shader + cbuffer。
    let shaders = shader::Shaders::new(&d3d.device)?;
    let cbuffer = shader::create_frame_constants_buffer(&d3d.device)?;

    // 3b. SwapChain backbuffer → RenderTargetView。PS 输出必须有 RTV 才会写进 backbuffer，
    //     否则 Present 呈现空白。RTV 一次性创建，每帧复用（OMSetRenderTargets）。
    let backbuffer: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D =
        unsafe { d3d.swapchain.GetBuffer(0) }?;
    let mut rtv: Option<windows::Win32::Graphics::Direct3D11::ID3D11RenderTargetView> = None;
    unsafe {
        d3d.device
            .CreateRenderTargetView(&backbuffer, None, Some(&mut rtv))?;
    }
    let rtv = rtv.unwrap();

    // 4. 捕获源：consent + 有目标 HWND 才尝试 WGC，否则程序化降级。
    let capture: Box<dyn CaptureSource + Send> = match (target_hwnd, consent) {
        (Some(hwnd), true) => match capture::WgcCaptureSource::try_new(&d3d.device, hwnd) {
            Ok(src) => Box::new(src),
            Err(e) => {
                eprintln!(
                    "D3D11Renderer: WGC capture unavailable, falling back to procedural: {:?}",
                    e
                );
                Box::new(capture::ProceduralCaptureSource::new())
            }
        },
        _ => Box::new(capture::ProceduralCaptureSource::new()),
    };

    Ok(RenderResources {
        _overlay: overlay,
        d3d,
        shaders,
        cbuffer,
        rtv,
        capture,
    })
}

/// 渲染线程主循环（~60fps）。
fn render_loop(
    res: RenderResources,
    latest: Arc<Mutex<Option<Frame>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    let RenderResources {
        _overlay,
        d3d,
        shaders,
        cbuffer,
        rtv,
        mut capture,
    } = res;
    let resolution = (_overlay.size.width as f32, _overlay.size.height as f32);
    let start = Instant::now();
    let frame_budget = std::time::Duration::from_millis(16);

    while !stop.load(std::sync::atomic::Ordering::SeqCst) {
        let frame_start = Instant::now();

        // 取最新 Frame（无则跳过绘制，但仍 Present 上一帧，避免画面冻结）。
        let frame = { *latest.lock().unwrap() };
        if let Some(ref frame) = frame {
            // capture 门控：on_target 决定是否激活捕获会话。
            capture.set_active(frame.on_target);

            let pctx = painter::PaintContext {
                context: &d3d.context,
                vertex_shader: &shaders.vertex,
                pixel_shader: &shaders.pixel,
                input_layout: &shaders.input_layout,
                cbuffer: &cbuffer,
                rtv: &rtv,
                capture: capture.as_mut(),
            };
            let now = start.elapsed().as_secs_f32();
            if let Err(e) = painter::paint_frame(pctx, frame, now, resolution) {
                eprintln!("abh-render: paint_frame error: {:?}", e);
            }
        }

        if let Err(e) = painter::present(&d3d.swapchain) {
            eprintln!("abh-render: present error: {:?}", e);
        }

        let elapsed = frame_start.elapsed();
        if elapsed < frame_budget {
            thread::sleep(frame_budget - elapsed);
        }
    }
    // 退出前停捕获（隐私：释放纹理）。
    capture.set_active(false);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 契约：D3D11Renderer 实现 Renderer trait（编译期断言）。
    /// 不构造实例（需 GPU），仅证明类型关系成立——controller 经
    /// `&mut dyn Renderer` 消费 D3D11Renderer 的契约在本 crate 编译时即被强制。
    #[allow(dead_code)]
    fn _assert_d3d11renderer_implements_renderer(r: &mut D3D11Renderer) -> &mut dyn Renderer {
        r
    }

    /// RenderError::Windows 由 windows::core::Error 转换（painter/capture 依赖此 From）。
    #[test]
    fn render_error_from_windows_error() {
        let e = windows::core::Error::from(windows::core::HRESULT(-1));
        let r: RenderError = e.into();
        match r {
            RenderError::Windows(_) => {} // ok
            other => panic!("expected RenderError::Windows, got {:?}", other),
        }
    }
}



