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

mod d3d;
mod overlay; // Task 2
mod shader; // Task 3
mod capture; // Task 4
mod painter; // Task 5

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
