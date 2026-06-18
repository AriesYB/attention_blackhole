//! 每帧渲染编排（spec §4.1 / §8.3）。
//!
//! 把一帧 `Frame` 转成 `FrameConstants`：Map 更新 cbuffer（CPU 写，DISCARD 旧内容），
//! 绑 VS/PS，设 capture SRV（若有），画全屏三角形（3 顶点，SV_VertexID），Present。
//!
//! Painter 是无状态编排：资源（device/context/shaders/cbuffer/capture）由
//! `D3D11Renderer` 持有，每帧借引用调用 `paint_frame`。这样渲染线程独占 D3D11 上下文，
//! 不需额外同步。

use std::mem::size_of;

use windows::Win32::Graphics::Direct3D11::{
    ID3D11Buffer, ID3D11DeviceContext, ID3D11PixelShader, ID3D11ShaderResourceView,
    ID3D11VertexShader, D3D11_MAP_WRITE_DISCARD, D3D11_MAPPED_SUBRESOURCE,
};

use super::capture::CaptureSource;
use super::shader::FrameConstants;
use super::RenderError;
use crate::controller::Frame;
use crate::model::state::AppState;

/// 一帧渲染所需的资源引用包（避免 paint_frame 长参数列表）。
pub struct PaintContext<'a> {
    pub context: &'a ID3D11DeviceContext,
    pub vertex_shader: &'a ID3D11VertexShader,
    pub pixel_shader: &'a ID3D11PixelShader,
    pub cbuffer: &'a ID3D11Buffer,
    pub capture: &'a mut dyn CaptureSource,
}

/// 执行一帧渲染。返回 Err 仅用于诊断；渲染线程应忽略并继续下一帧（不致命）。
///
/// - `frame`：controller 推来的状态（load/state/on_target）。
/// - `now_secs`：渲染时间秒（驱动吸积盘旋转/闪烁）。
/// - `resolution`：backbuffer 像素尺寸。
pub fn paint_frame(
    ctx: PaintContext,
    frame: &Frame,
    now_secs: f32,
    resolution: (f32, f32),
) -> Result<(), RenderError> {
    // 1. 据状态派生 uniform。
    let load = frame.load as f32 / 100.0;
    let dim = dimming_for_state(frame.state);
    // capture 门控：on_target=false 时 capture 源返回 None（set_active 已停）。
    let has_capture = frame.on_target && ctx.capture.has_capture();
    let srv = if frame.on_target {
        ctx.capture.current_frame_srv()
    } else {
        None
    };

    // 2. 组 FrameConstants。
    let consts = FrameConstants::new(load, dim, now_secs, resolution, has_capture);

    // 3. Map 写 cbuffer（WRITE_DISCARD：不阻塞 GPU 旧帧读）。
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        ctx.context
            .Map(ctx.cbuffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))
            .map_err(RenderError::Windows)?;
    }
    if !mapped.pData.is_null() {
        unsafe {
            let dst = mapped.pData as *mut u8;
            std::ptr::copy_nonoverlapping(
                &consts as *const FrameConstants as *const u8,
                dst,
                size_of::<FrameConstants>(),
            );
        }
    }
    unsafe {
        ctx.context.Unmap(ctx.cbuffer, 0);
    }

    // 4. 绑定 VS/PS + cbuffer(b0) + SRV(t0)。
    unsafe {
        ctx.context.VSSetShader(Some(ctx.vertex_shader), None);
        ctx.context.PSSetShader(Some(ctx.pixel_shader), None);
    }
    // cbuffer 绑到 VS 和 PS 的 b0。
    let cbs = [Some(ctx.cbuffer.clone())];
    unsafe {
        ctx.context.VSSetConstantBuffers(0, Some(&cbs));
        ctx.context.PSSetConstantBuffers(0, Some(&cbs));
    }
    // SRV 绑到 t0（无 capture 时给 None，shader 据 u_has_capture 分支）。
    let srvs: [Option<ID3D11ShaderResourceView>; 1] = [srv.clone()];
    unsafe {
        ctx.context.PSSetShaderResources(0, Some(&srvs));
    }
    // 采样器 s0：本实现未创建独立 sampler 状态对象。shader 声明 SamplerState；
    // D3D11 默认 sampler 为点采样 + clamp。生产化应建 D3D11_SAMPLER_DESC 绑定，
    // 此处从简（捕获纹理用作透镜采样，精度要求不苛刻）。

    // 5. 画全屏三角形（3 顶点，SV_VertexID 驱动，无 vertex buffer / input layout）。
    unsafe {
        ctx.context.Draw(3, 0);
    }

    Ok(())
}

/// 派生 u_dim：Dimming/ForcedBreak 状态压暗，其它状态不压。
fn dimming_for_state(state: AppState) -> f32 {
    match state {
        AppState::ForcedBreak => 0.9,
        AppState::Dimming => 0.6,
        _ => 0.0,
    }
}

/// Present（带 VSync）。渲染线程每帧末尾调用。
pub fn present(swapchain: &windows::Win32::Graphics::Dxgi::IDXGISwapChain) -> Result<(), RenderError> {
    use windows::Win32::Graphics::Dxgi::DXGI_PRESENT;
    unsafe {
        // Present(syncInterval=1, flags=DXGI_PRESENT(0))：syncInterval=1 = VSync 对齐。
        swapchain
            .Present(1, DXGI_PRESENT(0))
            .ok()
            .map_err(RenderError::Dxgi)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::state::AppState;

    /// ForcedBreak 压暗最狠（0.9）：强制休息时几乎全黑，逼用户离开。
    #[test]
    fn dimming_forced_break_strongest() {
        assert_eq!(dimming_for_state(AppState::ForcedBreak), 0.9);
    }

    /// Dimming 中等压暗（0.6）。
    #[test]
    fn dimming_dimming_moderate() {
        assert_eq!(dimming_for_state(AppState::Dimming), 0.6);
    }

    /// Working 不压暗（0.0）。
    #[test]
    fn dimming_working_none() {
        assert_eq!(dimming_for_state(AppState::Working), 0.0);
    }

    /// 压暗强度单调：ForcedBreak > Dimming > Working（确保状态升级视觉更暗）。
    #[test]
    fn dimming_monotonic_by_state() {
        let work = dimming_for_state(AppState::Working);
        let dim = dimming_for_state(AppState::Dimming);
        let fb = dimming_for_state(AppState::ForcedBreak);
        assert!(work < dim);
        assert!(dim < fb);
    }
}

