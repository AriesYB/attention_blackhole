//! 诊断 3：完整管线（overlay + D3D11 + RTV）画半透明红。
//! - 看到 红 → 管线通，黑洞不可见是 shader/painter 细节。
//! - 看不到红 → NOREDIRECTIONBITMAP 与本机 DWM/驱动不兼容，需换 layered 方案。
//! Run: cargo run --example overlay_render_diag

#[cfg(windows)]
fn main() {
    use attention_blackhole::renderer::d3d::D3D11Context;
    use attention_blackhole::renderer::overlay::OverlayWindow;
    use std::time::{Duration, Instant};
    use windows::Win32::Graphics::Direct3D11::ID3D11RenderTargetView;
    use windows::Win32::Graphics::Dxgi::DXGI_PRESENT;

    println!("=== 创建 overlay ===");
    let ov = OverlayWindow::new().expect("overlay");
    println!("  hwnd={:?} {}x{}", ov.hwnd, ov.size.width, ov.size.height);

    println!("=== 创建 D3D11 + SwapChain ===");
    let d3d = D3D11Context::new(ov.hwnd, ov.size.width, ov.size.height).expect("d3d");
    println!("  feature_level=0x{:X}", d3d.feature_level);

    // backbuffer → RTV。
    let backbuffer: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D =
        unsafe { d3d.swapchain.GetBuffer(0).expect("getbuffer") };
    let mut rtv: Option<ID3D11RenderTargetView> = None;
    unsafe {
        d3d.device
            .CreateRenderTargetView(&backbuffer, None, Some(&mut rtv))
            .expect("create rtv");
    }
    let rtv = rtv.unwrap();
    println!("  RTV created OK");

    // 设 viewport + 绑 RTV（与 painter 一致）。
    unsafe {
        let rtvs = [Some(rtv.clone())];
        d3d.context.OMSetRenderTargets(Some(&rtvs), None);
        use windows::Win32::Graphics::Direct3D11::D3D11_VIEWPORT;
        d3d.context.RSSetViewports(Some(&[D3D11_VIEWPORT {
            TopLeftX: 0.0, TopLeftY: 0.0,
            Width: ov.size.width as f32, Height: ov.size.height as f32,
            MinDepth: 0.0, MaxDepth: 1.0,
        }]));
    }

    println!("\n>>> 现在应看到【半透明红色】覆盖全屏 10 秒 <<<");
    println!(">>> 看到红  -> 管线通，黑洞不可见是 shader 问题");
    println!(">>> 看不到  -> NOREDIRECTIONBITMAP 兼容性问题，需换 layered 方案");

    let start = Instant::now();
    let mut frame = 0u64;
    while start.elapsed() < Duration::from_secs(10) {
        unsafe {
            // 半透明红 alpha=0.6，预乘：rgb*a。
            d3d.context.ClearRenderTargetView(&rtv, &[1.0 * 0.6, 0.0, 0.0, 0.6]);
            let hr = d3d.swapchain.Present(1, DXGI_PRESENT(0));
            if frame < 3 || !hr.is_ok() {
                println!("  frame {}: Present HRESULT = 0x{:X} ok={}", frame, hr.0, hr.is_ok());
            }
        }
        frame += 1;
        std::thread::sleep(Duration::from_millis(16));
    }
    println!("退出，共 {} 帧", frame);
}

#[cfg(not(windows))]
fn main() {}
