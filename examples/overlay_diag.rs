//! 诊断：创建 OverlayWindow 后打印窗口真实状态，判断「看不见」的根因。
//! Run: cargo run --example overlay_diag

#[cfg(windows)]
fn main() {
    use attention_blackhole::renderer::overlay::OverlayWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, IsWindowVisible, GetWindowRect, GWL_EXSTYLE, GWL_STYLE, WS_VISIBLE,
        WS_EX_LAYERED, WS_EX_TRANSPARENT, WS_EX_TOPMOST, WS_EX_NOREDIRECTIONBITMAP,
        GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
    };

    println!("=== 创建 OverlayWindow ===");
    let ov = match OverlayWindow::new() {
        Ok(o) => {
            println!("  OK: hwnd={:?}, size={}x{}", o.hwnd, o.size.width, o.size.height);
            o
        }
        Err(e) => {
            eprintln!("  FAILED: {:?}", e);
            std::process::exit(1);
        }
    };

    let hwnd = ov.hwnd;
    unsafe {
        println!("\n=== 窗口状态查询 ===");
        println!("  IsWindowVisible = {}", IsWindowVisible(hwnd).as_bool());

        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let st = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        println!("  EX style = 0x{:08X}", ex);
        println!("    LAYERED            = {}", (ex & WS_EX_LAYERED.0) != 0);
        println!("    TRANSPARENT        = {}", (ex & WS_EX_TRANSPARENT.0) != 0);
        println!("    TOPMOST            = {}", (ex & WS_EX_TOPMOST.0) != 0);
        println!("    NOREDIRECTIONBITMAP= {}", (ex & WS_EX_NOREDIRECTIONBITMAP.0) != 0);
        println!("  STYLE     = 0x{:08X}", st);
        println!("    WS_VISIBLE         = {}", (st & WS_VISIBLE.0) != 0);

        let mut rect = windows::Win32::Foundation::RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);
        println!("  WindowRect = left={} top={} right={} bottom={} ({}x{})",
            rect.left, rect.top, rect.right, rect.bottom,
            rect.right - rect.left, rect.bottom - rect.top);
    }

    unsafe {
        let cx = GetSystemMetrics(SM_CXSCREEN);
        let cy = GetSystemMetrics(SM_CYSCREEN);
        println!("\n=== 屏幕 ===");
        println!("  SM_CXSCREEN x SM_CYSCREEN = {} x {}", cx, cy);
    }

    println!("\n保持窗口 6 秒供观察，然后退出...");
    std::thread::sleep(std::time::Duration::from_secs(6));
    drop(ov);
    println!("退出");
}

#[cfg(not(windows))]
fn main() {}
