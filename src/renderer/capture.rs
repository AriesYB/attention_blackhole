//! WGC 捕获 + 程序化降级 + on_target/consent 门控（spec §4.1 / §9 隐私）。
//!
//! ## 隐私硬规则（spec §9）
//! 捕获的像素**只在显存**（`ID3D11Texture2D`），绝不拷回 RAM、绝不落盘、不导出、
//! 不读回。renderer 持有的捕获纹理仅作 shader 采样源。切走目标（on_target=false）或
//! 用户未同意（consent=false）时立即停止 `GraphicsCaptureSession`、释放纹理。
//!
//! ## 门控逻辑
//! - `consent=false`：`WgcCaptureSource` 根本不构造，永远走 `ProceduralCaptureSource`。
//! - `on_target=false`：`set_active(false)` → session 停止 + 清纹理与 SRV，
//!   `current_frame_srv()` 返回 None → shader 走 `u_has_capture=0` 程序化分支。
//!
//! ## D3D11 互操作桥接（Plan 3 最易踩坑处）
//! 1. 渲染设备（Win32 `ID3D11Device`）→ `IDXGIDevice` →
//!    `CreateDirect3D11DeviceFromDXGIDevice` → `IInspectable` 取 `IDirect3DDevice`（WinRT），
//!    WGC `Direct3D11CaptureFramePool::CreateFreeThreaded` 用它。
//! 2. HWND → `IGraphicsCaptureItemInterop::CreateForWindow` → `GraphicsCaptureItem`
//!    （桌面互操作路径，非 UWP 的 TryCreateFromWindowId）。
//! 3. 捕获帧的 `IDirect3DSurface` 经 `IDirect3DDxgiInterfaceAccess::GetInterface`
//!    取回 `ID3D11Texture2D`，由 painter 复制到自有的可采样纹理建 SRV 喂 shader。
//!
//! NOTE: `WgcCaptureSource` 的真实捕获验证需 Win10 2004+ + 真实桌面会话（Task 6）。
//! `ProceduralCaptureSource` 是降级主路径，可在任意环境构造。
//!
//! NOTE: 本模块的 `WgcCaptureSource`/`create_idirect3d_device` 等在 Task 5 由
//! `D3D11Renderer`（painter）构造时才被调用，故 Task 4 阶段为 dead_code。
//! `ProceduralCaptureSource` 是唯一即时被测的路径。Task 5 接入后此 allow 可移除。
#![allow(dead_code)]

use windows::core::{factory, Interface};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11ShaderResourceView, ID3D11Texture2D,
    D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

use super::RenderError;

/// 捕获目标：决定 WGC `GraphicsCaptureItem` 从什么创建。
///
/// - `Window(HWND)`：捕获单个目标窗口的像素（spec §8.1「吞噬你的代码」原设计）。
///   WGC 捕获该窗口的合成内容，与 z-order 无关——即使被 overlay 盖住也能拿到。
/// - `Monitor(HMONITOR)`：捕获整个显示器的真实桌面（「扭曲整个屏幕」）。overlay 自身
///   已 `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` 排除出捕获（见 overlay.rs），
///   故捕获纹理 = overlay 背后的真实桌面。
///
/// 两种范围共用同一套 WGC FramePool/Session/零拷贝管线（`WgcCaptureSource`），
/// 仅创建 `GraphicsCaptureItem` 时分支（`CreateForWindow` vs `CreateForMonitor`）。
#[derive(Clone, Copy, Debug)]
pub enum CaptureTarget {
    Window(HWND),
    Monitor(HMONITOR),
}

/// 捕获源抽象：painter 据它决定 shader 走捕获纹理还是程序化背景。
pub trait CaptureSource {
    /// 返回当前捕获帧的 shader resource view（None = 无可用帧 → 程序化背景）。
    fn current_frame_srv(&mut self) -> Option<ID3D11ShaderResourceView>;
    /// on_target 或 consent 变化时调用：启停捕获会话。
    fn set_active(&mut self, active: bool);
    /// 是否处于「有真实捕获」状态（painter 据此设 u_has_capture）。
    fn has_capture(&self) -> bool;
}

// -----------------------------------------------------------------------------
// 程序化降级源：不持有任何真实纹理。current_frame_srv 永远 None，
// has_capture 永远 false → shader 走 u_has_capture=0 程序化星云分支。
// 这是默认安全路径：consent=false / 旧 OS / WGC 创建失败 时回落至此。
// -----------------------------------------------------------------------------

pub struct ProceduralCaptureSource;

impl ProceduralCaptureSource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ProceduralCaptureSource {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureSource for ProceduralCaptureSource {
    fn current_frame_srv(&mut self) -> Option<ID3D11ShaderResourceView> {
        None
    }
    fn set_active(&mut self, _active: bool) {}
    fn has_capture(&self) -> bool {
        false
    }
}

// -----------------------------------------------------------------------------
// WGC 真实捕获源
// -----------------------------------------------------------------------------

/// WGC 捕获源。从目标 HWND 创建 GraphicsCaptureItem + FramePool + Session，
/// 帧到达回调把最新 IDirect3DSurface 转成 ID3D11Texture2D 存进 latest（Arc 共享）。
///
/// latest 用 `Arc<Mutex<Option<...>>>`：回调闭包需 'static + Send，结构体持另一半克隆。
/// `current_frame_srv` 读 latest，若非空则建/缓存 SRV（复用 device）。隐私：纹理只在显存。
pub struct WgcCaptureSource {
    #[allow(dead_code)]
    item: GraphicsCaptureItem,
    #[allow(dead_code)]
    frame_pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    /// 最新捕获帧纹理（捕获回调写，painter 读）。WGC FramePool 拥有、我们持 COM 引用。
    latest: Arc<Mutex<Option<ID3D11Texture2D>>>,
    /// 自有可采样纹理。每帧把 WGC 帧复制进来，shader 永远只采它。
    copy_tex: Option<ID3D11Texture2D>,
    copy_srv: Option<ID3D11ShaderResourceView>,
    copy_size: Option<(u32, u32)>,
    active: bool,
}

use std::sync::{Arc, Mutex};

impl WgcCaptureSource {
    /// 尝试从捕获目标（窗口或显示器）创建 WGC 捕获源。失败（旧 OS / item 不可创建 /
    /// 互操作失败）返回 Err，上层回落 ProceduralCaptureSource。
    ///
    /// `target` 决定 `GraphicsCaptureItem` 的来源：`Window` 走 `CreateForWindow`（spec §8.1
    /// 目标窗口），`Monitor` 走 `CreateForMonitor`（全屏真实桌面）。两者共用同一套
    /// FramePool/Session/回调/SRV 重建管线——WGC 对两种 item 一视同仁。
    pub fn try_new(device: &ID3D11Device, target: CaptureTarget) -> Result<Self, RenderError> {
        // 1. 渲染 device → IDirect3DDevice（WinRT 桥接）。
        let d3d_device = create_idirect3d_device(device)?;

        // 2. 捕获目标 → GraphicsCaptureItem（桌面互操作路径）。
        let interop: IGraphicsCaptureItemInterop =
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                .map_err(RenderError::Windows)?;
        let item: GraphicsCaptureItem = unsafe {
            match target {
                CaptureTarget::Window(hwnd) => interop.CreateForWindow(hwnd),
                CaptureTarget::Monitor(hmon) => interop.CreateForMonitor(hmon),
            }
        }
        .map_err(RenderError::Windows)?;

        // 3. FramePool（free-threaded，不需要 MTA 设备线程）。
        let item_size = item.Size().map_err(RenderError::Windows)?;
        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &d3d_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            SizeInt32 {
                Width: item_size.Width,
                Height: item_size.Height,
            },
        )
        .map_err(RenderError::Windows)?;

        // 4. 帧到达回调：IDirect3DSurface 经互操作转 ID3D11Texture2D 存 latest。
        let latest: Arc<Mutex<Option<ID3D11Texture2D>>> = Arc::new(Mutex::new(None));
        let latest_cb = latest.clone();

        let _token = frame_pool
            .FrameArrived(&TypedEventHandler::<
                Direct3D11CaptureFramePool,
                windows::core::IInspectable,
            >::new(move |pool, _args| {
                let pool = pool.as_ref().ok_or_else(|| {
                    windows::core::Error::from(windows::Win32::Foundation::E_POINTER)
                })?;
                let frame = pool.TryGetNextFrame()?;
                let surface = frame.Surface()?;
                let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
                let texture: ID3D11Texture2D = unsafe { access.GetInterface() }?;
                *latest_cb.lock().unwrap() = Some(texture);
                Ok(())
            }))
            .map_err(RenderError::Windows)?;

        // 5. session（不立即 Start：等 set_active(true)）。
        let session = frame_pool
            .CreateCaptureSession(&item)
            .map_err(RenderError::Windows)?;

        let context = unsafe { device.GetImmediateContext().map_err(RenderError::Windows)? };

        Ok(Self {
            item,
            frame_pool,
            session,
            device: device.clone(),
            context,
            latest,
            copy_tex: None,
            copy_srv: None,
            copy_size: None,
            active: false,
        })
    }

    /// 每帧调用：把最新 WGC 帧复制到自有纹理，并返回自有纹理的 SRV。
    ///
    /// 关键：不要把 WGC 的 frame-pool texture 长期绑定成 shader SRV。FramePool 默认
    /// 只有 2 个缓冲；如果 latest + cached SRV 都持有 WGC 纹理，拖动窗口这种高频
    /// compositor 更新会很快把 frame pool 堵住，表现为“按住鼠标拖动期间卡旧画面，
    /// 松手后才更新”。复制到自有纹理后立刻释放 WGC 帧引用，frame pool 才能持续产帧。
    fn update_copy_from_latest(&mut self) {
        let src = {
            let mut guard = self.latest.lock().unwrap();
            guard.take()
        };
        let Some(src) = src else {
            return;
        };

        let mut src_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            src.GetDesc(&mut src_desc);
        }
        let size = (src_desc.Width, src_desc.Height);

        if self.copy_tex.is_none() || self.copy_size != Some(size) {
            let copy_desc = D3D11_TEXTURE2D_DESC {
                Width: src_desc.Width,
                Height: src_desc.Height,
                MipLevels: 1,
                ArraySize: 1,
                Format: src_desc.Format,
                SampleDesc: src_desc.SampleDesc,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };

            let mut tex: Option<ID3D11Texture2D> = None;
            let tex_result = unsafe {
                self.device
                    .CreateTexture2D(&copy_desc, None, Some(&mut tex))
            };
            if tex_result.is_err() {
                self.copy_tex = None;
                self.copy_srv = None;
                self.copy_size = None;
                return;
            }

            let tex = tex.unwrap();
            let mut srv: Option<ID3D11ShaderResourceView> = None;
            let desc = windows::Win32::Graphics::Direct3D11::D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: copy_desc.Format,
                ViewDimension: windows::Win32::Graphics::Direct3D::D3D_SRV_DIMENSION_TEXTURE2D,
                Anonymous: windows::Win32::Graphics::Direct3D11::D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: windows::Win32::Graphics::Direct3D11::D3D11_TEX2D_SRV {
                        MostDetailedMip: 0,
                        MipLevels: 1,
                    },
                },
            };
            let srv_result = unsafe {
                self.device
                    .CreateShaderResourceView(&tex, Some(&desc), Some(&mut srv))
            };
            if srv_result.is_err() {
                self.copy_tex = None;
                self.copy_srv = None;
                self.copy_size = None;
                return;
            }

            self.copy_tex = Some(tex);
            self.copy_srv = srv;
            self.copy_size = Some(size);
        }

        if let Some(dst) = self.copy_tex.as_ref() {
            unsafe {
                self.context.CopyResource(dst, &src);
            }
            // `src` drops here, releasing the WGC frame-pool buffer promptly.
            return;
        }
    }
}

impl CaptureSource for WgcCaptureSource {
    fn current_frame_srv(&mut self) -> Option<ID3D11ShaderResourceView> {
        if !self.active {
            return None;
        }
        self.update_copy_from_latest();
        self.copy_srv.clone()
    }

    fn set_active(&mut self, active: bool) {
        if active == self.active {
            return;
        }
        self.active = active;
        if active {
            if let Err(e) = self.session.StartCapture() {
                eprintln!("WgcCaptureSource: StartCapture failed: {:?}", e);
                self.active = false;
            }
        } else {
            // 停止 + 清纹理/SRV（隐私：立即释放捕获纹理，不残留）。
            let _ = self.session.Close();
            *self.latest.lock().unwrap() = None;
            self.copy_tex = None;
            self.copy_srv = None;
            self.copy_size = None;
        }
    }

    fn has_capture(&self) -> bool {
        self.active && (self.copy_srv.is_some() || self.latest.lock().unwrap().is_some())
    }
}

impl Drop for WgcCaptureSource {
    fn drop(&mut self) {
        let _ = self.session.Close();
        *self.latest.lock().unwrap() = None;
        self.copy_tex = None;
        self.copy_srv = None;
        self.copy_size = None;
    }
}

/// 渲染设备（Win32 ID3D11Device）→ WinRT IDirect3DDevice。
fn create_idirect3d_device(device: &ID3D11Device) -> Result<IDirect3DDevice, RenderError> {
    let dxgi_device: IDXGIDevice = device.cast().map_err(RenderError::Dxgi)?;
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device) }
        .map_err(RenderError::Windows)?;
    inspectable.cast().map_err(RenderError::Windows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 降级路径不 panic 可构造：ProceduralCaptureSource 永不持有纹理。
    /// 这是 consent=false / 旧 OS / WGC 失败时的默认安全回落（spec §9 门控）。
    #[test]
    fn procedural_source_never_has_capture() {
        let mut src = ProceduralCaptureSource::new();
        assert!(!src.has_capture());
        assert!(src.current_frame_srv().is_none());
        // 激活对程序化源无意义（它本就不捕获），但不 panic。
        src.set_active(true);
        assert!(!src.has_capture());
        src.set_active(false);
        assert!(src.current_frame_srv().is_none());
    }

    /// Default trait 可用（便于上层结构体字段初始化）。
    #[test]
    fn procedural_source_default() {
        let src = ProceduralCaptureSource::default();
        assert!(!src.has_capture());
    }
}

