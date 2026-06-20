//! 黑洞 HLSL shader 装载与常量缓冲（spec §4.1）。
//!
//! - HLSL 源经 `include_str!("../../shaders/blackhole.hlsl")` 编译进二进制，运行时由
//!   `D3DCompile`（d3dcompiler_47.dll）编译为 VS/PS blob，再 `CreateVertexShader`/
//!   `CreatePixelShader`。
//! - cbuffer `FrameConstants` 每帧由 painter 更新：u_load/u_dim/u_time/u_resolution/
//!   u_has_capture。
//!
//! shader 源为**原创实现**（见 blackhole.hlsl 文件头许可证声明），非 ShaderToy 移植。
//!
//! NOTE: Task 1 阶段为 dead_code，Task 5 由 Painter 接入后此 allow 可移除。
#![allow(dead_code)]

use std::mem::size_of;

use windows::core::{s, PCSTR};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_OPTIMIZATION_LEVEL3};
use windows::Win32::Graphics::Direct3D::{ID3DBlob, ID3DInclude};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Buffer, ID3D11Device, ID3D11InputLayout, ID3D11PixelShader, ID3D11SamplerState,
    ID3D11VertexShader, D3D11_BIND_CONSTANT_BUFFER, D3D11_BUFFER_DESC, D3D11_CPU_ACCESS_WRITE,
    D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_SAMPLER_DESC, D3D11_TEXTURE_ADDRESS_MIRROR,
    D3D11_USAGE_DYNAMIC,
};

use super::RenderError;

/// cbuffer FrameConstants 的 Rust 镜像。布局必须与 blackhole.hlsl 的 cbuffer 完全一致
///（含 padding 对齐）。HLSL cbuffer 按 16 字节 pack：见各字段注释。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FrameConstants {
    /// 第一个 float4 槽位：u_load, u_dim, u_time, _pad。
    pub u_load: f32,
    pub u_dim: f32,
    pub u_time: f32,
    _pad0: f32,
    /// 第二个 float4 槽位：u_resolution.xy, u_has_capture, _pad1。
    pub u_resolution: [f32; 2],
    pub u_has_capture: u32,
    _pad1: f32,
}

impl FrameConstants {
    pub fn new(load: f32, dim: f32, time: f32, resolution: (f32, f32), has_capture: bool) -> Self {
        Self {
            u_load: load,
            u_dim: dim,
            u_time: time,
            _pad0: 0.0,
            u_resolution: [resolution.0, resolution.1],
            u_has_capture: if has_capture { 1 } else { 0 },
            _pad1: 0.0,
        }
    }
}

/// 编译好的 shader 集合：VS + PS 对象 + input layout。
pub struct Shaders {
    pub vertex: ID3D11VertexShader,
    pub pixel: ID3D11PixelShader,
    /// 全屏三角形 input layout（空元素：VS 只用 SV_VertexID，无顶点缓冲输入）。
    /// 必须绑定一次 IASetInputLayout，否则 Input Assembler 不向 VS 供应顶点，
    /// Draw 不产生任何几何体 → backbuffer 全空（这是黑洞不可见的根因）。
    pub input_layout: ID3D11InputLayout,
}

impl Shaders {
    /// 编译 HLSL 源并创建 VS/PS + input layout。
    pub fn new(device: &ID3D11Device) -> Result<Self, RenderError> {
        let hlsl = include_str!("../../shaders/blackhole.hlsl");

        // VS。
        let vs_blob = compile(hlsl, s!("vs_main"), s!("vs_5_0"))?;
        let mut vertex: Option<ID3D11VertexShader> = None;
        unsafe {
            device
                .CreateVertexShader(&vs_blob, None, Some(&mut vertex))
                .map_err(RenderError::Windows)?;
        }

        // input layout：用 VS blob 创建。VS 输入只有 SV_VertexID（系统语义，不来自
        // 顶点缓冲），故 layout 元素为空数组。D3D11 要求 IASetInputLayout 被绑定过
        // （即使是空 layout）才会让 Draw 产生几何体——这是「无顶点缓冲全屏三角形」
        // 方案的官方要求。
        let mut input_layout: Option<ID3D11InputLayout> = None;
        unsafe {
            device
                .CreateInputLayout(&[], &vs_blob, Some(&mut input_layout))
                .map_err(RenderError::Windows)?;
        }

        // PS。
        let ps_blob = compile(hlsl, s!("ps_main"), s!("ps_5_0"))?;
        let mut pixel: Option<ID3D11PixelShader> = None;
        unsafe {
            device
                .CreatePixelShader(&ps_blob, None, Some(&mut pixel))
                .map_err(RenderError::Windows)?;
        }

        Ok(Self {
            vertex: vertex.unwrap(),
            input_layout: input_layout.unwrap(),
            pixel: pixel.unwrap(),
        })
    }
}

/// 用 D3DCompile 编译一段 HLSL。返回字节 vec（从 ID3DBlob 拷出，blob 随后释放）。
fn compile(hlsl: &str, entry: PCSTR, target: PCSTR) -> Result<Vec<u8>, RenderError> {
    let src = hlsl.as_bytes();
    let mut blob: Option<ID3DBlob> = None;
    // shader 无 #include，pinclude 传 None::<&ID3DInclude>（NULL include 接口）。
    let no_include: Option<&ID3DInclude> = None;
    unsafe {
        D3DCompile(
            src.as_ptr() as *const _,
            src.len(),
            s!("blackhole.hlsl"),
            None,
            no_include,
            entry,
            target,
            // flags1：最高优化级。默认 flags=0 等于 LEVEL1（调试友好、优化少），32 步测地线
            // + 捕获纹理采样的全屏 PS 在 LEVEL1 下明显卡顿。LEVEL3 显著提速（用户反馈「卡」）。
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0, // flags2
            &mut blob,
            None, // error blob（简化：不取，靠 HRESULT）
        )
        .map_err(RenderError::Windows)?;
    }
    let blob = blob.ok_or_else(|| {
        RenderError::Windows(windows::core::Error::from(windows::core::HRESULT(-1)))
    })?;
    let ptr = unsafe { blob.GetBufferPointer() } as *const u8;
    let len = unsafe { blob.GetBufferSize() };
    let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
    Ok(slice.to_vec())
}

/// 创建 FrameConstants 的动态 cbuffer（CPU 可写，每帧 Map 更新）。
pub fn create_frame_constants_buffer(device: &ID3D11Device) -> Result<ID3D11Buffer, RenderError> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: size_of::<FrameConstants>() as u32,
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut buffer: Option<ID3D11Buffer> = None;
    unsafe {
        device
            .CreateBuffer(&desc, None, Some(&mut buffer))
            .map_err(RenderError::Windows)?;
    }
    Ok(buffer.unwrap())
}

/// 创建 capture 纹理的线性采样器（绑 s0）。
///
/// plan-3 终审的 should-fix #4：painter 原依赖 D3D11 默认 sampler（点采样+clamp），
/// 透镜扭曲后的捕获纹理显块状。这里建一个线性三线性 sampler + 镜像重复寻址
///（`ADDRESS_MIRROR`，对应 shader 的 `mirrorUV` 语义，让透镜后的越界采样不出界、
/// 不边缘涂抹）。几何盘内的捕获纹理作 lensed sky plane，线性采样让扭曲的代码边缘平滑。
pub fn create_linear_sampler(device: &ID3D11Device) -> Result<ID3D11SamplerState, RenderError> {
    let desc = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D11_TEXTURE_ADDRESS_MIRROR,
        AddressV: D3D11_TEXTURE_ADDRESS_MIRROR,
        AddressW: D3D11_TEXTURE_ADDRESS_MIRROR,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: windows::Win32::Graphics::Direct3D11::D3D11_COMPARISON_NEVER,
        BorderColor: [0.0, 0.0, 0.0, 0.0],
        MinLOD: 0.0,
        MaxLOD: 0.0, // 捕获纹理无 mipmap，只用最高分辨率层
    };
    let mut sampler: Option<ID3D11SamplerState> = None;
    unsafe {
        device
            .CreateSamplerState(&desc, Some(&mut sampler))
            .map_err(RenderError::Windows)?;
    }
    Ok(sampler.unwrap())
}

// 编译期断言：FrameConstants 必须是 32 字节（两个 float4 槽位），与 HLSL cbuffer 对齐。
const _: () = assert!(
    size_of::<FrameConstants>() == 32,
    "FrameConstants must be 32 bytes (two float4 slots) to match HLSL cbuffer packing"
);

#[cfg(test)]
mod tests {
    use super::*;

    /// FrameConstants::new 字段映射正确（painter 据此驱动 shader uniform）。
    #[test]
    fn frame_constants_fields_mapped() {
        let c = FrameConstants::new(0.5, 0.3, 12.5, (1920.0, 1080.0), true);
        assert!((c.u_load - 0.5).abs() < 1e-6);
        assert!((c.u_dim - 0.3).abs() < 1e-6);
        assert!((c.u_time - 12.5).abs() < 1e-6);
        assert_eq!(c.u_resolution, [1920.0, 1080.0]);
        assert_eq!(c.u_has_capture, 1);
    }

    /// has_capture=false 时 u_has_capture=0（shader 据此走程序化背景分支）。
    #[test]
    fn frame_constants_no_capture_flag() {
        let c = FrameConstants::new(0.0, 0.0, 0.0, (800.0, 600.0), false);
        assert_eq!(c.u_has_capture, 0);
    }

    /// Default 产生全零（安全初值，渲染线程首帧前不会用未初始化内存）。
    #[test]
    fn frame_constants_default_zeroed() {
        let c = FrameConstants::default();
        assert_eq!(c.u_load, 0.0);
        assert_eq!(c.u_has_capture, 0);
        assert_eq!(c.u_resolution, [0.0, 0.0]);
    }
}

