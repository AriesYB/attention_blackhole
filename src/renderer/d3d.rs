//! D3D11 设备与交换链骨架（spec §4.1）。
//!
//! 负责：创建 `ID3D11Device` + immediate `ID3D11DeviceContext`（`D3D11CreateDevice`，
//! flag `BGRA_SUPPORT` —— WGC 互操作要求），通过 device 取 `IDXGIFactory`
//! 创建绑定到 overlay HWND 的 `IDXGISwapChain`。feature level 优先 11_0，失败回 9_3。
//!
//! 本模块是纯设备/管线骨架，不做 shader/capture/painter 编排（那些在各自子模块）。
//! 隐私硬规则（spec §9）：本模块创建的设备后续只用于 GPU 显存内的捕获纹理采样，
//! 绝不把捕获纹理拷回 RAM。
//!
//! NOTE: 本模块的 `D3D11Context` 在 Task 5 由 `D3D11Renderer`（mod.rs）构造时才被调用，
//! 故 Task 1 阶段为 dead_code。Task 5 接入后此 allow 可移除。
#![allow(dead_code)]

use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL,
    D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_9_3,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, ID3D11Device,
    ID3D11DeviceContext,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_DESC, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIFactory, IDXGISwapChain, DXGI_SWAP_CHAIN_DESC, DXGI_SWAP_EFFECT_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

use super::RenderError;

/// D3D11 设备 + immediate context + 绑定到 overlay HWND 的 SwapChain。
///
/// 渲染线程独占持有；非线程安全（D3D11 immediate context 本身单线程）。
pub struct D3D11Context {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub swapchain: IDXGISwapChain,
    /// 创建时使用的 feature level（诊断用）。
    pub feature_level: i32,
}

impl D3D11Context {
    /// 创建设备与交换链。
    ///
    /// - `hwnd`：渲染目标窗口（overlay HWND）。
    /// - `width`/`height`：客户区像素尺寸（SwapChain backbuffer 大小）。
    pub fn new(hwnd: HWND, width: u32, height: u32) -> Result<Self, RenderError> {
        // 尝试 feature level 优先级：11_0 → 9_3（最低保底，旧硬件/虚拟机）。
        let wanted = [D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_9_3];

        // 先硬件驱动；失败（无 GPU/驱动）则回退 WARP（软件光栅）。
        let (device, context, feature_level) = match try_create_device(
            D3D_DRIVER_TYPE_HARDWARE,
            &wanted,
        ) {
            Ok(t) => t,
            Err(hw_err) => match try_create_device(D3D_DRIVER_TYPE_WARP, &wanted) {
                Ok(t) => t,
                Err(warp_err) => {
                    return Err(RenderError::DeviceCreate {
                        hardware: hw_err,
                        warp: warp_err,
                    });
                }
            },
        };

        // SwapChain 需经 IDXGIFactory 创建。从 device 沿 DXGI 父链上溯取 factory。
        let swapchain = create_swapchain(&device, hwnd, width, height)?;

        Ok(Self {
            device,
            context,
            swapchain,
            feature_level,
        })
    }
}

/// 用指定驱动类型 + feature level 候选创建 device/context。
/// 返回 (device, immediate context, 实际用上的 feature level)。
fn try_create_device(
    driver_type: D3D_DRIVER_TYPE,
    wanted_feature_levels: &[D3D_FEATURE_LEVEL],
) -> Result<(ID3D11Device, ID3D11DeviceContext, i32), windows::core::Error> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    let mut feature_level: D3D_FEATURE_LEVEL = D3D_FEATURE_LEVEL_9_3;

    // flags 含 BGRA_SUPPORT：WGC（DirectX 11 Capture）互操作的硬性要求，
    // 也保证 2D BGRA 纹理可读写。
    let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;

    unsafe {
        D3D11CreateDevice(
            None, // adapter：NULL = 默认适配器（P0 泛型接受 None）
            driver_type,
            Default::default(), // 软件光栅模块句柄（仅 SOFTWARE 驱动用，这里 HMODULE::default）
            flags,
            Some(wanted_feature_levels), // 候选 feature level，按顺序尝试
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut feature_level),
            Some(&mut context),
        )?;
    }

    Ok((device.unwrap(), context.unwrap(), feature_level.0))
}

/// 从 device 沿 DXGI 父链取 IDXGIFactory，创建 SwapChain。
fn create_swapchain(
    device: &ID3D11Device,
    hwnd: HWND,
    width: u32,
    height: u32,
) -> Result<IDXGISwapChain, RenderError> {
    // Interface::cast 把 ID3D11Device 查询到 IDXGIDevice。
    let dxgi_device: IDXGIDevice = device.cast().map_err(RenderError::Dxgi)?;
    // device → adapter → factory（GetParent 是泛型，用 IDXGIAdapter 标注中间类型）
    let adapter: windows::Win32::Graphics::Dxgi::IDXGIAdapter =
        unsafe { dxgi_device.GetParent() }.map_err(RenderError::Dxgi)?;
    let factory: IDXGIFactory = unsafe { adapter.GetParent() }.map_err(RenderError::Dxgi)?;

    let desc = DXGI_SWAP_CHAIN_DESC {
        BufferDesc: DXGI_MODE_DESC {
            Width: width,
            Height: height,
            RefreshRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            Format: DXGI_FORMAT_B8G8R8A8_UNORM, // 与 BGRA_SUPPORT flag 配合
            ScanlineOrdering: Default::default(),
            Scaling: Default::default(),
        },
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1, // 不用 MSAA（raymarching 全屏 pass 无需）
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2, // 双缓冲
        OutputWindow: hwnd,
        Windowed: true.into(),
        SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
        Flags: 0,
    };

    // CreateSwapChain 是原生 COM 签名：返回 HRESULT，SwapChain 经 out 指针返回。
    // HRESULT.ok() 把失败码转成 windows::core::Error。
    let mut swapchain: Option<IDXGISwapChain> = None;
    unsafe {
        factory
            .CreateSwapChain(device, &desc, &mut swapchain)
            .ok()
            .map_err(RenderError::Dxgi)?;
    }
    Ok(swapchain.unwrap())
}
