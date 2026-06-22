#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    if let Err(err) = app::run() {
        app::show_error(&err);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("ornament_demo is Windows-only (D3D11 + WGC + tray icon).");
    std::process::exit(2);
}

#[cfg(windows)]
mod app {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::time::Instant;

    use attention_blackhole::controller::Frame;
    use attention_blackhole::model::state::AppState;
    use attention_blackhole::renderer::capture::{
        CaptureSource, CaptureTarget, ProceduralCaptureSource, WgcCaptureSource,
    };
    use attention_blackhole::renderer::d3d::D3D11Context;
    use attention_blackhole::renderer::painter::{self, PaintContext};
    use attention_blackhole::renderer::shader::{
        self, ShaderOptions, MODE_ORNAMENT_CENTER,
    };
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{
        COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
    };
    use windows::Win32::Graphics::Direct3D11::{
        ID3D11RenderTargetView, ID3D11Texture2D,
    };
    use windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
    use windows::Win32::Graphics::Gdi::{
        CreateEllipticRgn, GetMonitorInfoW, MonitorFromWindow, SetWindowRgn, COLOR_WINDOW,
        HBRUSH, HMONITOR, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Controls::{
        InitCommonControls, MARGINS, BST_CHECKED, BST_UNCHECKED, TBM_SETPOS,
        TBM_SETRANGE, TBS_AUTOTICKS, TBS_HORZ, TRACKBAR_CLASSW,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NOTIFYICONDATAW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD,
        NIM_DELETE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreateIcon, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
        DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos,
        GetDesktopWindow, GetDlgItem, GetWindowLongPtrW, HMENU, LoadCursorW,
        MessageBoxW, PeekMessageW, PostQuitMessage, RegisterClassExW, SendMessageW,
        SetForegroundWindow, SetLayeredWindowAttributes, SetWindowDisplayAffinity,
        SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TrackPopupMenu,
        TranslateMessage, BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, CREATESTRUCTW,
        GWLP_USERDATA, HCURSOR, HICON, HTCLIENT, HTTRANSPARENT, IDC_ARROW, LWA_ALPHA,
        MB_ICONERROR, MB_OK, MF_SEPARATOR, MF_STRING, MSG, PM_REMOVE, SW_SHOW,
        SW_SHOWNA, TPM_RETURNCMD, TPM_RIGHTBUTTON, SWP_HIDEWINDOW, SWP_NOACTIVATE,
        SWP_NOZORDER, SWP_SHOWWINDOW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE,
        WM_COMMAND, WM_DESTROY, WM_HSCROLL, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
        WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE, WM_NCHITTEST, WM_QUIT, WM_RBUTTONUP,
        WM_USER, WNDCLASSEXW, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WS_CAPTION, WS_CHILD,
        WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_EX_TRANSPARENT, WS_OVERLAPPED, WS_POPUP, WS_SYSMENU, WS_VISIBLE,
    };

    const WM_TRAYICON: u32 = WM_USER + 1;
    const TRAY_ID: u32 = 1;

    const MENU_LOCK_TOGGLE: usize = 1003;
    const MENU_SETTINGS: usize = 1004;
    const MENU_SCREENSHOT_TOGGLE: usize = 1005;
    const MENU_EXIT: usize = 1099;

    const ID_SIZE: i32 = 2001;
    const ID_MASS: i32 = 2002;
    const ID_RING: i32 = 2003;
    const ID_DISK_STRENGTH: i32 = 2004;
    const ID_DISK_COLOR: i32 = 2005;
    const ID_DISK_ENABLE: i32 = 2006;
    const ID_DISK_TILT: i32 = 2007;
    const ID_DISK_SPAN: i32 = 2008;
    const ID_SIZE_VALUE: i32 = 2101;
    const ID_MASS_VALUE: i32 = 2102;
    const ID_RING_VALUE: i32 = 2103;
    const ID_DISK_STRENGTH_VALUE: i32 = 2104;
    const ID_DISK_COLOR_VALUE: i32 = 2105;
    const ID_DISK_TILT_VALUE: i32 = 2106;
    const ID_DISK_SPAN_VALUE: i32 = 2107;
    const TBM_GETPOS_LOCAL: u32 = WM_USER;

    pub fn run() -> Result<(), String> {
        unsafe {
            InitCommonControls();
        }
        let monitor = primary_monitor()?;
        let screen = ScreenRect::from_monitor(monitor)?;
        let mut state = Box::new(OrnamentState::new(screen));

        let hwnd = unsafe { create_ornament_window(&mut state) }?;
        state.visual_hwnd = Some(hwnd);
        let handle_hwnd = unsafe { create_handle_window(&mut state) }?;
        state.handle_hwnd = Some(handle_hwnd);
        state.sync_handle_window();
        let tray = TrayIcon::add(hwnd)?;

        let d3d = D3D11Context::new(hwnd, screen.width as u32, screen.height as u32)
            .map_err(|e| format!("create D3D11 context failed: {e:?}"))?;
        let shaders = shader::Shaders::new(&d3d.device)
            .map_err(|e| format!("compile blackhole shader failed: {e:?}"))?;
        let cbuffer = shader::create_frame_constants_buffer(&d3d.device)
            .map_err(|e| format!("create shader constants failed: {e:?}"))?;
        let sampler = shader::create_linear_sampler(&d3d.device)
            .map_err(|e| format!("create sampler failed: {e:?}"))?;
        let rtv = create_render_target_view(&d3d)?;

        let mut capture: Box<dyn CaptureSource> =
            match WgcCaptureSource::try_new(&d3d.device, CaptureTarget::Monitor(monitor)) {
                Ok(src) => Box::new(src),
                Err(_) => Box::new(ProceduralCaptureSource::new()),
            };
        capture.set_active(true);

        let start = Instant::now();
        let resolution = (screen.width as f32, screen.height as f32);
        let mut screenshot_visible_applied = false;
        let frame = Frame {
            load: 72.0,
            state: AppState::Working,
            on_target: true,
        };

        while state.running {
            pump_messages(&mut state);
            if state.screenshot_visible != screenshot_visible_applied {
                apply_screenshot_visibility(
                    hwnd,
                    capture.as_mut(),
                    state.screenshot_visible,
                );
                screenshot_visible_applied = state.screenshot_visible;
            }
            state.update_center(start.elapsed().as_secs_f32());

            let options = state.shader_options();
            let pctx = PaintContext {
                context: &d3d.context,
                vertex_shader: &shaders.vertex,
                pixel_shader: &shaders.pixel,
                input_layout: &shaders.input_layout,
                cbuffer: &cbuffer,
                rtv: &rtv,
                sampler: &sampler,
                capture: capture.as_mut(),
            };

            let now = start.elapsed().as_secs_f32();
            if let Err(e) =
                painter::paint_frame_with_options(pctx, &frame, now, resolution, options)
            {
                return Err(format!("paint ornament frame failed: {e:?}"));
            }
            if let Err(e) = painter::present(&d3d.swapchain) {
                return Err(format!("present ornament frame failed: {e:?}"));
            }
            state.sync_handle_window();
            std::thread::yield_now();
        }

        capture.set_active(false);
        drop(tray);
        unsafe {
            if let Some(settings_hwnd) = state.settings_hwnd.take() {
                let _ = DestroyWindow(settings_hwnd);
            }
            let _ = DestroyWindow(handle_hwnd);
            let _ = DestroyWindow(hwnd);
        }
        Ok(())
    }

    pub fn show_error(message: &str) {
        let text = wide_null(message);
        unsafe {
            let _ = MessageBoxW(
                None,
                PCWSTR(text.as_ptr()),
                w!("Attention Blackhole"),
                MB_OK | MB_ICONERROR,
            );
        }
    }

    fn apply_screenshot_visibility(
        hwnd: HWND,
        capture: &mut dyn CaptureSource,
        screenshot_visible: bool,
    ) {
        if screenshot_visible {
            let _ = capture.current_frame_srv();
            capture.set_frame_frozen(true);
            set_capture_excluded(hwnd, false);
        } else {
            set_capture_excluded(hwnd, true);
            capture.set_frame_frozen(false);
        }
    }

    fn set_capture_excluded(hwnd: HWND, excluded: bool) {
        let affinity = if excluded {
            WDA_EXCLUDEFROMCAPTURE
        } else {
            WDA_NONE
        };
        unsafe {
            if SetWindowDisplayAffinity(hwnd, affinity).is_err() {
                // Older Windows builds may not support WDA_EXCLUDEFROMCAPTURE.
            }
        }
    }

    #[derive(Clone, Copy)]
    struct ScreenRect {
        left: i32,
        top: i32,
        width: i32,
        height: i32,
    }

    impl ScreenRect {
        fn from_monitor(monitor: HMONITOR) -> Result<Self, String> {
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                rcMonitor: RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
                rcWork: RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
                dwFlags: 0,
            };
            let ok = unsafe { GetMonitorInfoW(monitor, &mut info).as_bool() };
            if !ok {
                return Err("GetMonitorInfoW failed".to_string());
            }
            let width = info.rcMonitor.right - info.rcMonitor.left;
            let height = info.rcMonitor.bottom - info.rcMonitor.top;
            if width <= 0 || height <= 0 {
                return Err("primary monitor has invalid size".to_string());
            }
            Ok(Self {
                left: info.rcMonitor.left,
                top: info.rcMonitor.top,
                width,
                height,
            })
        }

        fn aspect(self) -> f32 {
            self.width as f32 / self.height.max(1) as f32
        }

        fn screen_to_uv(self, pt: POINT) -> [f32; 2] {
            [
                ((pt.x - self.left) as f32 / self.width as f32).clamp(0.0, 1.0),
                ((pt.y - self.top) as f32 / self.height as f32).clamp(0.0, 1.0),
            ]
        }
    }

    struct OrnamentState {
        screen: ScreenRect,
        running: bool,
        locked: bool,
        screenshot_visible: bool,
        visual_hwnd: Option<HWND>,
        handle_hwnd: Option<HWND>,
        settings_hwnd: Option<HWND>,
        handle_region_size: i32,
        base_center: [f32; 2],
        center: [f32; 2],
        drag_offset: [f32; 2],
        dragging: bool,
        size_scale: f32,
        mass_scale: f32,
        ring_scale: f32,
        disk_strength: f32,
        disk_enabled: bool,
        disk_hue: f32,
        disk_tilt: f32,
        disk_span: f32,
    }

    impl OrnamentState {
        fn new(screen: ScreenRect) -> Self {
            Self {
                screen,
                running: true,
                locked: false,
                screenshot_visible: false,
                visual_hwnd: None,
                handle_hwnd: None,
                settings_hwnd: None,
                handle_region_size: 0,
                base_center: [0.58, 0.48],
                center: [0.58, 0.48],
                drag_offset: [0.0, 0.0],
                dragging: false,
                size_scale: 0.58,
                mass_scale: 1.0,
                ring_scale: 1.0,
                disk_strength: 1.0,
                disk_enabled: true,
                disk_hue: 0.08,
                disk_tilt: 0.92,
                disk_span: 1.0,
            }
        }

        fn update_center(&mut self, time: f32) {
            if self.dragging {
                return;
            }
            let drift = [
                0.018 * (time * 0.85).sin() + 0.007 * (time * 1.73 + 0.4).sin(),
                0.014 * (time * 0.97 + 1.3).sin() + 0.005 * (time * 1.41).sin(),
            ];
            self.center = self.clamp_center([
                self.base_center[0] + drift[0],
                self.base_center[1] + drift[1],
            ]);
        }

        fn shader_center(&self) -> [f32; 2] {
            [self.center[0], 1.0 - self.center[1]]
        }

        fn shader_options(&self) -> ShaderOptions {
            let disk_strength = if self.disk_enabled {
                self.disk_strength
            } else {
                0.0
            };
            ShaderOptions {
                ornament_center: self.shader_center(),
                size_scale: self.size_scale,
                mode_flags: MODE_ORNAMENT_CENTER,
                visual_params: [
                    self.mass_scale,
                    self.ring_scale,
                    disk_strength,
                    self.mass_scale,
                ],
                disk_tint: self.disk_tint(),
                disk_params: [
                    self.disk_tilt,
                    self.disk_span,
                    disk_strength.max(0.01),
                    0.0,
                ],
            }
        }

        fn disk_tint(&self) -> [f32; 4] {
            let rgb = hsv_to_rgb(self.disk_hue, 0.78, 1.12);
            [rgb[0], rgb[1], rgb[2], 0.0]
        }

        fn toggle_lock(&mut self) {
            self.locked = !self.locked;
            if self.locked {
                self.end_drag();
            }
            self.sync_handle_window();
        }

        fn toggle_screenshot_visible(&mut self) {
            self.screenshot_visible = !self.screenshot_visible;
        }

        fn sync_handle_window(&mut self) {
            let Some(hwnd) = self.handle_hwnd else {
                return;
            };

            unsafe {
                if self.locked {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOZORDER | SWP_NOACTIVATE | SWP_HIDEWINDOW,
                    );
                    return;
                }

                let radius = self.horizon_radius_px();
                let size = (radius * 2).max(1);
                if self.handle_region_size != size {
                    let region = CreateEllipticRgn(0, 0, size, size);
                    if !region.is_invalid() && SetWindowRgn(hwnd, Some(region), true) != 0 {
                        self.handle_region_size = size;
                    }
                }

                let center_x = self.screen.left + (self.center[0] * self.screen.width as f32) as i32;
                let center_y = self.screen.top + (self.center[1] * self.screen.height as f32) as i32;
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    center_x - radius,
                    center_y - radius,
                    size,
                    size,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
        }

        fn horizon_radius_px(&self) -> i32 {
            (self.horizon_radius_uv() * self.screen.height as f32)
                .round()
                .clamp(18.0, 180.0) as i32
        }

        fn horizon_radius_uv(&self) -> f32 {
            let load = 0.72_f32;
            let rh = 0.015 + (0.15 - 0.015) * load.powf(0.7);
            rh * self.size_scale
        }

        fn begin_drag(&mut self, hwnd: HWND) -> bool {
            if self.locked {
                return false;
            }
            let Some(uv) = cursor_uv(self.screen) else {
                return false;
            };
            if !self.hit_test_uv(uv) {
                return false;
            }
            self.dragging = true;
            self.drag_offset = [self.center[0] - uv[0], self.center[1] - uv[1]];
            unsafe {
                SetCapture(hwnd);
            }
            true
        }

        fn drag_to_cursor(&mut self) {
            if !self.dragging {
                return;
            }
            if let Some(uv) = cursor_uv(self.screen) {
                let next = [
                    uv[0] + self.drag_offset[0],
                    uv[1] + self.drag_offset[1],
                ];
                self.center = self.clamp_center(next);
                self.base_center = self.center;
            }
        }

        fn end_drag(&mut self) {
            if self.dragging {
                self.dragging = false;
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
        }

        fn hit_test_uv(&self, uv: [f32; 2]) -> bool {
            let dx = (uv[0] - self.center[0]) * self.screen.aspect();
            let dy = uv[1] - self.center[1];
            let radius = self.horizon_radius_uv() * 1.05;
            (dx * dx + dy * dy).sqrt() <= radius
        }

        fn set_visual_params(
            &mut self,
            size_scale: f32,
            mass_scale: f32,
            ring_scale: f32,
            disk_strength: f32,
            disk_hue: f32,
            disk_tilt: f32,
            disk_span: f32,
            disk_enabled: bool,
        ) {
            self.size_scale = size_scale.clamp(0.35, 1.30);
            self.mass_scale = mass_scale.clamp(0.45, 1.75);
            self.ring_scale = ring_scale.clamp(0.50, 1.75);
            self.disk_strength = disk_strength.clamp(0.0, 1.0);
            self.disk_hue = disk_hue.rem_euclid(1.0);
            self.disk_tilt = disk_tilt.clamp(0.0, 1.0);
            self.disk_span = disk_span.clamp(0.60, 1.80);
            self.disk_enabled = disk_enabled;
            self.base_center = self.clamp_center(self.base_center);
            self.center = self.clamp_center(self.center);
            self.sync_handle_window();
            self.update_settings_labels();
        }

        fn update_settings_labels(&self) {
            let Some(hwnd) = self.settings_hwnd else {
                return;
            };
            unsafe {
                update_settings_labels(hwnd, self);
            }
        }

        fn clamp_center(&self, center: [f32; 2]) -> [f32; 2] {
            let margin_y = (0.18 * self.size_scale).clamp(0.06, 0.22);
            let margin_x = (margin_y / self.screen.aspect()).clamp(0.04, 0.18);
            [
                center[0].clamp(margin_x, 1.0 - margin_x),
                center[1].clamp(margin_y, 1.0 - margin_y),
            ]
        }
    }

    struct TrayIcon {
        hwnd: HWND,
        icon: HICON,
    }

    impl TrayIcon {
        fn add(hwnd: HWND) -> Result<Self, String> {
            let mut data = tray_data(hwnd);
            data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            data.uCallbackMessage = WM_TRAYICON;
            data.hIcon = create_blackhole_icon()?;
            write_tip(&mut data.szTip, "Attention Blackhole");

            let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &data).as_bool() };
            if !ok {
                unsafe {
                    let _ = DestroyIcon(data.hIcon);
                }
                return Err("Shell_NotifyIconW(NIM_ADD) failed".to_string());
            }
            Ok(Self {
                hwnd,
                icon: data.hIcon,
            })
        }
    }

    impl Drop for TrayIcon {
        fn drop(&mut self) {
            let data = tray_data(self.hwnd);
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
                let _ = DestroyIcon(self.icon);
            }
        }
    }

    fn create_blackhole_icon() -> Result<HICON, String> {
        const W: usize = 32;
        const H: usize = 32;
        let mut and_mask = [0xff_u8; W * H / 8];
        let mut xor_bits = [0_u8; W * H * 4];

        for y in 0..H {
            for x in 0..W {
                let dx = x as f32 - 15.5;
                let dy = y as f32 - 15.5;
                let r = (dx * dx + dy * dy).sqrt();
                if r > 14.5 {
                    continue;
                }

                clear_mask_bit(&mut and_mask, x, y);
                let color = if r < 5.8 {
                    [2, 1, 5]
                } else if (7.0..=9.2).contains(&r) {
                    [255, 235, 180]
                } else if (9.2..=12.0).contains(&r) {
                    [255, 130, 40]
                } else {
                    [110, 185, 255]
                };
                let row = H - 1 - y;
                let idx = (row * W + x) * 4;
                xor_bits[idx] = color[2];
                xor_bits[idx + 1] = color[1];
                xor_bits[idx + 2] = color[0];
                xor_bits[idx + 3] = 0;
            }
        }

        unsafe {
            CreateIcon(
                None,
                W as i32,
                H as i32,
                1,
                32,
                and_mask.as_ptr(),
                xor_bits.as_ptr(),
            )
            .map_err(|e| format!("CreateIcon failed: {e:?}"))
        }
    }

    fn clear_mask_bit(mask: &mut [u8], x: usize, y: usize) {
        let byte = y * 4 + x / 8;
        let bit = 7 - (x % 8);
        mask[byte] &= !(1 << bit);
    }

    fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW::default();
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = TRAY_ID;
        data
    }

    fn write_tip(dst: &mut [u16; 128], text: &str) {
        let wide = wide_null(text);
        let len = wide.len().saturating_sub(1).min(dst.len() - 1);
        dst[..len].copy_from_slice(&wide[..len]);
        dst[len] = 0;
    }

    fn primary_monitor() -> Result<HMONITOR, String> {
        let desktop = unsafe { GetDesktopWindow() };
        let monitor = unsafe { MonitorFromWindow(desktop, MONITOR_DEFAULTTOPRIMARY) };
        if monitor.is_invalid() {
            Err("MonitorFromWindow returned an invalid monitor".to_string())
        } else {
            Ok(monitor)
        }
    }

    fn create_render_target_view(d3d: &D3D11Context) -> Result<ID3D11RenderTargetView, String> {
        let backbuffer: ID3D11Texture2D = unsafe { d3d.swapchain.GetBuffer(0) }
            .map_err(|e| format!("GetBuffer(0) failed: {e:?}"))?;
        let mut rtv: Option<ID3D11RenderTargetView> = None;
        unsafe {
            d3d.device
                .CreateRenderTargetView(&backbuffer, None, Some(&mut rtv))
                .map_err(|e| format!("CreateRenderTargetView failed: {e:?}"))?;
        }
        rtv.ok_or_else(|| "CreateRenderTargetView returned None".to_string())
    }

    fn pump_messages(state: &mut OrnamentState) {
        let mut msg = MSG::default();
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    state.running = false;
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    unsafe fn create_ornament_window(state: &mut OrnamentState) -> Result<HWND, String> {
        let class_name = w!("AttentionBlackholeOrnament");
        let module =
            GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW failed: {e:?}"))?;
        let hinstance = HINSTANCE(module.0);

        let mut class = WNDCLASSEXW::default();
        class.cbSize = size_of::<WNDCLASSEXW>() as u32;
        class.lpfnWndProc = Some(wnd_proc);
        class.hInstance = hinstance;
        class.lpszClassName = class_name;
        class.hCursor = LoadCursorW(None, IDC_ARROW)
            .ok()
            .unwrap_or(HCURSOR(std::ptr::null_mut()));

        if RegisterClassExW(&class) == 0 {
            return Err("RegisterClassExW failed".to_string());
        }

        let ex_style = WINDOW_EX_STYLE(
            WS_EX_LAYERED.0
                | WS_EX_TRANSPARENT.0
                | WS_EX_TOPMOST.0
                | WS_EX_TOOLWINDOW.0
                | WS_EX_NOACTIVATE.0,
        );
        let style = WINDOW_STYLE(WS_POPUP.0);
        let screen = state.screen;
        let hwnd = CreateWindowExW(
            ex_style,
            class_name,
            w!("Attention Blackhole Ornament"),
            style,
            screen.left,
            screen.top,
            screen.width,
            screen.height,
            None,
            None,
            Some(hinstance),
            Some(state as *mut OrnamentState as *const c_void),
        )
        .map_err(|e| format!("CreateWindowExW failed: {e:?}"))?;

        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        DwmExtendFrameIntoClientArea(hwnd, &margins)
            .map_err(|e| format!("DwmExtendFrameIntoClientArea failed: {e:?}"))?;
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)
            .map_err(|e| format!("SetLayeredWindowAttributes failed: {e:?}"))?;
        let _ = ShowWindow(hwnd, SW_SHOWNA);

        set_capture_excluded(hwnd, true);
        Ok(hwnd)
    }

    unsafe fn create_handle_window(state: &mut OrnamentState) -> Result<HWND, String> {
        let class_name = w!("AttentionBlackholeHandle");
        let module =
            GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW failed: {e:?}"))?;
        let hinstance = HINSTANCE(module.0);

        let mut class = WNDCLASSEXW::default();
        class.cbSize = size_of::<WNDCLASSEXW>() as u32;
        class.lpfnWndProc = Some(wnd_proc);
        class.hInstance = hinstance;
        class.lpszClassName = class_name;
        class.hCursor = LoadCursorW(None, IDC_ARROW)
            .ok()
            .unwrap_or(HCURSOR(std::ptr::null_mut()));

        if RegisterClassExW(&class) == 0 {
            return Err("RegisterClassExW(handle) failed".to_string());
        }

        let ex_style = WINDOW_EX_STYLE(
            WS_EX_LAYERED.0 | WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0,
        );
        let hwnd = CreateWindowExW(
            ex_style,
            class_name,
            w!("Attention Blackhole Handle"),
            WINDOW_STYLE(WS_POPUP.0),
            state.screen.left,
            state.screen.top,
            1,
            1,
            None,
            None,
            Some(hinstance),
            Some(state as *mut OrnamentState as *const c_void),
        )
        .map_err(|e| format!("CreateWindowExW(handle) failed: {e:?}"))?;

        SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA)
            .map_err(|e| format!("SetLayeredWindowAttributes(handle) failed: {e:?}"))?;
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        Ok(hwnd)
    }

    unsafe fn open_settings_window(state: &mut OrnamentState) {
        if let Some(hwnd) = state.settings_hwnd {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            return;
        }

        match create_settings_window(state) {
            Ok(hwnd) => {
                state.settings_hwnd = Some(hwnd);
                update_settings_labels(hwnd, state);
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
            }
            Err(err) => show_error(&err),
        }
    }

    unsafe fn create_settings_window(state: &mut OrnamentState) -> Result<HWND, String> {
        let class_name = w!("AttentionBlackholeSettings");
        let module =
            GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW failed: {e:?}"))?;
        let hinstance = HINSTANCE(module.0);

        let mut class = WNDCLASSEXW::default();
        class.cbSize = size_of::<WNDCLASSEXW>() as u32;
        class.lpfnWndProc = Some(wnd_proc);
        class.hInstance = hinstance;
        class.lpszClassName = class_name;
        class.hCursor = LoadCursorW(None, IDC_ARROW)
            .ok()
            .unwrap_or(HCURSOR(std::ptr::null_mut()));
        class.hbrBackground = HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut c_void);

        let _ = RegisterClassExW(&class);

        let x = state.screen.left + 80;
        let y = state.screen.top + 80;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0),
            class_name,
            w!("黑洞参数"),
            WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0),
            x,
            y,
            390,
            505,
            None,
            None,
            Some(hinstance),
            Some(state as *mut OrnamentState as *const c_void),
        )
        .map_err(|e| format!("CreateWindowExW(settings) failed: {e:?}"))?;

        create_settings_controls(hwnd, hinstance, state)?;
        Ok(hwnd)
    }

    unsafe fn create_settings_controls(
        parent: HWND,
        hinstance: HINSTANCE,
        state: &OrnamentState,
    ) -> Result<(), String> {
        create_static(parent, hinstance, 0, "视界大小", 18, 18, 150, 20)?;
        create_static(parent, hinstance, ID_SIZE_VALUE, "", 285, 18, 80, 20)?;
        create_trackbar(
            parent,
            hinstance,
            ID_SIZE,
            18,
            40,
            340,
            value_to_pos(state.size_scale, 0.35, 1.30),
        )?;

        create_static(parent, hinstance, 0, "视界质量", 18, 74, 150, 20)?;
        create_static(parent, hinstance, ID_MASS_VALUE, "", 285, 74, 80, 20)?;
        create_trackbar(
            parent,
            hinstance,
            ID_MASS,
            18,
            96,
            340,
            value_to_pos(state.mass_scale, 0.45, 1.75),
        )?;

        create_static(parent, hinstance, 0, "爱因斯坦环", 18, 130, 150, 20)?;
        create_static(parent, hinstance, ID_RING_VALUE, "", 285, 130, 80, 20)?;
        create_trackbar(
            parent,
            hinstance,
            ID_RING,
            18,
            152,
            340,
            value_to_pos(state.ring_scale, 0.50, 1.75),
        )?;

        let checkbox = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("BUTTON"),
            w!("显示吸积盘"),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | BS_AUTOCHECKBOX as u32),
            18,
            194,
            150,
            24,
            Some(parent),
            child_menu(ID_DISK_ENABLE),
            Some(hinstance),
            None,
        )
        .map_err(|e| format!("CreateWindowExW(disk checkbox) failed: {e:?}"))?;
        let check_state = if state.disk_enabled {
            BST_CHECKED.0
        } else {
            BST_UNCHECKED.0
        };
        let _ = SendMessageW(
            checkbox,
            BM_SETCHECK,
            Some(WPARAM(check_state as usize)),
            None,
        );

        create_static(parent, hinstance, 0, "吸积盘强度", 18, 224, 150, 20)?;
        create_static(parent, hinstance, ID_DISK_STRENGTH_VALUE, "", 285, 224, 80, 20)?;
        create_trackbar(
            parent,
            hinstance,
            ID_DISK_STRENGTH,
            18,
            246,
            340,
            value_to_pos(state.disk_strength, 0.0, 1.0),
        )?;

        create_static(parent, hinstance, 0, "吸积盘颜色", 18, 284, 150, 20)?;
        create_static(parent, hinstance, ID_DISK_COLOR_VALUE, "", 285, 284, 80, 20)?;
        create_trackbar(
            parent,
            hinstance,
            ID_DISK_COLOR,
            18,
            306,
            340,
            value_to_pos(state.disk_hue, 0.0, 1.0),
        )?;

        create_static(parent, hinstance, 0, "吸积盘倾角", 18, 344, 150, 20)?;
        create_static(parent, hinstance, ID_DISK_TILT_VALUE, "", 285, 344, 80, 20)?;
        create_trackbar(
            parent,
            hinstance,
            ID_DISK_TILT,
            18,
            366,
            340,
            value_to_pos(state.disk_tilt, 0.0, 1.0),
        )?;

        create_static(parent, hinstance, 0, "吸积盘范围", 18, 402, 150, 20)?;
        create_static(parent, hinstance, ID_DISK_SPAN_VALUE, "", 285, 402, 80, 20)?;
        create_trackbar(
            parent,
            hinstance,
            ID_DISK_SPAN,
            18,
            424,
            340,
            value_to_pos(state.disk_span, 0.60, 1.80),
        )?;

        Ok(())
    }

    unsafe fn create_static(
        parent: HWND,
        hinstance: HINSTANCE,
        id: i32,
        text: &str,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Result<HWND, String> {
        let wide = wide_null(text);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            PCWSTR(wide.as_ptr()),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
            x,
            y,
            width,
            height,
            Some(parent),
            child_menu(id),
            Some(hinstance),
            None,
        )
        .map_err(|e| format!("CreateWindowExW(static) failed: {e:?}"))
    }

    unsafe fn create_trackbar(
        parent: HWND,
        hinstance: HINSTANCE,
        id: i32,
        x: i32,
        y: i32,
        width: i32,
        position: i32,
    ) -> Result<HWND, String> {
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            TRACKBAR_CLASSW,
            PCWSTR::null(),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | TBS_HORZ | TBS_AUTOTICKS),
            x,
            y,
            width,
            34,
            Some(parent),
            child_menu(id),
            Some(hinstance),
            None,
        )
        .map_err(|e| format!("CreateWindowExW(trackbar) failed: {e:?}"))?;
        let _ = SendMessageW(
            hwnd,
            TBM_SETRANGE,
            Some(WPARAM(1)),
            Some(LPARAM(((100_u32) << 16) as isize)),
        );
        let _ = SendMessageW(
            hwnd,
            TBM_SETPOS,
            Some(WPARAM(1)),
            Some(LPARAM(position.clamp(0, 100) as isize)),
        );
        Ok(hwnd)
    }

    fn child_menu(id: i32) -> Option<HMENU> {
        if id == 0 {
            None
        } else {
            Some(HMENU(id as usize as *mut c_void))
        }
    }

    unsafe fn read_settings_controls(hwnd: HWND, state: &mut OrnamentState) {
        let size_scale = pos_to_value(read_slider(hwnd, ID_SIZE), 0.35, 1.30);
        let mass_scale = pos_to_value(read_slider(hwnd, ID_MASS), 0.45, 1.75);
        let ring_scale = pos_to_value(read_slider(hwnd, ID_RING), 0.50, 1.75);
        let disk_strength = pos_to_value(read_slider(hwnd, ID_DISK_STRENGTH), 0.0, 1.0);
        let disk_hue = pos_to_value(read_slider(hwnd, ID_DISK_COLOR), 0.0, 1.0);
        let disk_tilt = pos_to_value(read_slider(hwnd, ID_DISK_TILT), 0.0, 1.0);
        let disk_span = pos_to_value(read_slider(hwnd, ID_DISK_SPAN), 0.60, 1.80);
        let disk_enabled = GetDlgItem(Some(hwnd), ID_DISK_ENABLE)
            .map(|checkbox| {
                SendMessageW(checkbox, BM_GETCHECK, None, None).0
                    == BST_CHECKED.0 as isize
            })
            .unwrap_or(state.disk_enabled);

        state.set_visual_params(
            size_scale,
            mass_scale,
            ring_scale,
            disk_strength,
            disk_hue,
            disk_tilt,
            disk_span,
            disk_enabled,
        );
    }

    unsafe fn read_slider(parent: HWND, id: i32) -> i32 {
        GetDlgItem(Some(parent), id)
            .map(|hwnd| SendMessageW(hwnd, TBM_GETPOS_LOCAL, None, None).0 as i32)
            .unwrap_or(0)
            .clamp(0, 100)
    }

    unsafe fn update_settings_labels(hwnd: HWND, state: &OrnamentState) {
        set_child_text(hwnd, ID_SIZE_VALUE, &format!("{:.2}x", state.size_scale));
        set_child_text(hwnd, ID_MASS_VALUE, &format!("{:.2}x", state.mass_scale));
        set_child_text(hwnd, ID_RING_VALUE, &format!("{:.2}x", state.ring_scale));
        set_child_text(
            hwnd,
            ID_DISK_STRENGTH_VALUE,
            &format!("{:.0}%", state.disk_strength * 100.0),
        );
        set_child_text(hwnd, ID_DISK_COLOR_VALUE, hue_name(state.disk_hue));
        set_child_text(hwnd, ID_DISK_TILT_VALUE, &format!("{:.0}°", disk_tilt_degrees(state.disk_tilt)));
        set_child_text(hwnd, ID_DISK_SPAN_VALUE, &format!("{:.2}x", state.disk_span));
    }

    unsafe fn set_child_text(parent: HWND, id: i32, text: &str) {
        if let Ok(hwnd) = GetDlgItem(Some(parent), id) {
            let wide = wide_null(text);
            let _ = SetWindowTextW(hwnd, PCWSTR(wide.as_ptr()));
        }
    }

    fn value_to_pos(value: f32, min: f32, max: f32) -> i32 {
        (((value - min) / (max - min)) * 100.0).round().clamp(0.0, 100.0) as i32
    }

    fn pos_to_value(pos: i32, min: f32, max: f32) -> f32 {
        min + (max - min) * (pos.clamp(0, 100) as f32 / 100.0)
    }

    fn hue_name(hue: f32) -> &'static str {
        let h = hue.rem_euclid(1.0);
        if h < 0.08 || h >= 0.93 {
            "红"
        } else if h < 0.17 {
            "橙"
        } else if h < 0.29 {
            "黄"
        } else if h < 0.46 {
            "绿"
        } else if h < 0.62 {
            "青"
        } else if h < 0.78 {
            "蓝"
        } else {
            "紫"
        }
    }

    fn disk_tilt_degrees(tilt: f32) -> f32 {
        12.0 + tilt.clamp(0.0, 1.0) * 77.0
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_NCCREATE {
            let createstruct = lparam.0 as *const CREATESTRUCTW;
            if !createstruct.is_null() {
                let state = (*createstruct).lpCreateParams as *mut OrnamentState;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
            }
            return LRESULT(1);
        }

        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OrnamentState;
        if !state_ptr.is_null() {
            let state = &mut *state_ptr;
            let is_visual = state.visual_hwnd == Some(hwnd);
            let is_handle = state.handle_hwnd == Some(hwnd);
            let is_settings = state.settings_hwnd == Some(hwnd);
            match msg {
                WM_NCHITTEST => {
                    if is_visual {
                        return LRESULT(HTTRANSPARENT as isize);
                    }
                    if is_handle && !state.locked {
                        return LRESULT(HTCLIENT as isize);
                    }
                    return LRESULT(HTTRANSPARENT as isize);
                }
                WM_LBUTTONDOWN => {
                    if is_handle && state.begin_drag(hwnd) {
                        return LRESULT(0);
                    }
                }
                WM_MOUSEMOVE => {
                    if is_handle {
                        state.drag_to_cursor();
                        return LRESULT(0);
                    }
                }
                WM_LBUTTONUP => {
                    if is_handle {
                        state.end_drag();
                        return LRESULT(0);
                    }
                }
                WM_TRAYICON => {
                    let event = lparam.0 as u32;
                    if is_visual && (event == WM_RBUTTONUP || event == WM_LBUTTONDBLCLK) {
                        show_tray_menu(hwnd, state);
                        return LRESULT(0);
                    }
                }
                WM_HSCROLL => {
                    if is_settings {
                        read_settings_controls(hwnd, state);
                        return LRESULT(0);
                    }
                }
                WM_COMMAND => {
                    if is_visual {
                        let id = wparam.0 & 0xffff;
                        handle_menu_command(hwnd, state, id);
                        return LRESULT(0);
                    }
                    if is_settings {
                        let id = (wparam.0 & 0xffff) as i32;
                        if id == ID_DISK_ENABLE {
                            read_settings_controls(hwnd, state);
                            return LRESULT(0);
                        }
                    }
                }
                WM_CLOSE => {
                    if is_settings {
                        let _ = DestroyWindow(hwnd);
                        return LRESULT(0);
                    }
                    if is_visual {
                        state.running = false;
                        PostQuitMessage(0);
                        return LRESULT(0);
                    }
                }
                WM_DESTROY => {
                    if is_settings {
                        state.settings_hwnd = None;
                        return LRESULT(0);
                    }
                    if is_visual {
                        state.running = false;
                        PostQuitMessage(0);
                        return LRESULT(0);
                    }
                }
                _ => {}
            }
        }

        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    fn show_tray_menu(hwnd: HWND, state: &mut OrnamentState) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            append_menu_text(menu, MENU_SETTINGS, "参数面板...");
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            if state.locked {
                append_menu_text(menu, MENU_LOCK_TOGGLE, "解锁拖动");
            } else {
                append_menu_text(menu, MENU_LOCK_TOGGLE, "锁定拖动");
            }
            if state.screenshot_visible {
                append_menu_text(menu, MENU_SCREENSHOT_TOGGLE, "恢复实时捕获");
            } else {
                append_menu_text(menu, MENU_SCREENSHOT_TOGGLE, "截图可见");
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            append_menu_text(menu, MENU_EXIT, "退出");

            let mut pt = POINT { x: 0, y: 0 };
            if GetCursorPos(&mut pt).is_ok() {
                let _ = SetForegroundWindow(hwnd);
                let cmd = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_RIGHTBUTTON,
                    pt.x,
                    pt.y,
                    Some(0),
                    hwnd,
                    None,
                )
                .0 as usize;
                if cmd != 0 {
                    handle_menu_command(hwnd, state, cmd);
                }
            }
            let _ = DestroyMenu(menu);
        }
    }

    unsafe fn append_menu_text(
        menu: windows::Win32::UI::WindowsAndMessaging::HMENU,
        id: usize,
        text: &str,
    ) {
        let wide = wide_null(text);
        let _ = AppendMenuW(menu, MF_STRING, id, PCWSTR(wide.as_ptr()));
    }

    fn handle_menu_command(_hwnd: HWND, state: &mut OrnamentState, id: usize) {
        match id {
            MENU_SETTINGS => {
                unsafe {
                    open_settings_window(state);
                }
            }
            MENU_LOCK_TOGGLE => state.toggle_lock(),
            MENU_SCREENSHOT_TOGGLE => state.toggle_screenshot_visible(),
            MENU_EXIT => {
                state.running = false;
                unsafe {
                    PostQuitMessage(0);
                }
            }
            _ => {}
        }
    }

    fn cursor_uv(screen: ScreenRect) -> Option<[f32; 2]> {
        let mut pt = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut pt).ok()?;
        }
        Some(screen.screen_to_uv(pt))
    }

    fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> [f32; 3] {
        let h = hue.rem_euclid(1.0) * 6.0;
        let i = h.floor();
        let f = h - i;
        let p = value * (1.0 - saturation);
        let q = value * (1.0 - saturation * f);
        let t = value * (1.0 - saturation * (1.0 - f));

        match i as i32 {
            0 => [value, t, p],
            1 => [q, value, p],
            2 => [p, value, t],
            3 => [p, q, value],
            4 => [t, p, value],
            _ => [value, p, q],
        }
    }

    fn wide_null(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
}
