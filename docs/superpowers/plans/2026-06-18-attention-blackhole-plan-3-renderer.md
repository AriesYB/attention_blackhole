# 注意力黑洞 — Plan 3: Renderer（D3D11 overlay + WGC 捕获 + 黑洞 shader）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用 D3D11 实现渲染器栈：透明置顶 overlay 窗口 + GPU raymarching 黑洞 shader（移植 ShaderToy）+ WGC 真实窗口像素捕获（透镜扭曲采样「吞噬代码」）+ 程序化背景降级 + 独立 60fps 渲染线程。把这套组装成 `D3D11Renderer`，实现 Plan 1 定义的 `Renderer` trait。controller 与 model 一行不改。

**Architecture:** 新增 `src/renderer/` 模块，仅 `cfg(windows)`。与 Plan 2 一样遵守分层硬约束：`controller.rs` 不得 `use crate::renderer`——`D3D11Renderer` 只经 `&mut dyn Renderer` 进入（与 mock/CollectingRenderer 同路）。renderer 内部按 spec §4.1 拆为：`overlay`（透明窗口）/ `d3d`（设备+管线）/ `shader`（HLSL+常量）/ `capture`（WGC+降级）/ `painter`（编排）。渲染在独立线程，controller 通过 channel 把 `Frame` 推给它。

**Tech Stack:** Rust（edition 2021）。沿用 `windows` crate，新增 `Graphics_Capture` / `Graphics_DirectX_Direct3D11` / `Win32_Graphics_Direct3D11` / `Win32_Graphics_Direct3D11on12` / `Win32_Graphics_Dxgi` 等 feature。无新增第三方 crate（不引入 wgpu/gfx-hal 等，保持 spec §3 决策「原生 D3D11」）。

---

## 与 Plan 1/2 的接口契约（不可破坏）

`Renderer` trait（`src/controller.rs:14`）已固定：

```rust
pub trait Renderer {
    fn render(&mut self, frame: &Frame);
}
```

`Frame`（`src/controller.rs:25`）字段：

| 字段 | renderer 责任 |
|------|---------------|
| `load: f64` (0..100) | → `u_load` uniform（归一化到 0..1）：驱动事件视界半径（~2%→~35% 短边）、吸积盘亮度、暗角强度。 |
| `state: AppState` | → `u_dim` uniform 与施压模式：Working=0（纯视觉）/ Dimming=压暗+横幅 / ForcedBreak=深暗+模态层。 |
| `on_target: bool` | → 捕获门控：仅 `on_target` 时 WGC 捕获目标窗口像素喂透镜；切走立即停止捕获、清空纹理、回落程序化背景。 |

**关键不变量**（spec §9 隐私）：捕获的像素**只在显存**（`ID3D11Texture2D`），绝不拷回 RAM、绝不落盘。renderer 持有的捕获纹理仅作 shader 采样源，不导出、不读回。

**线程边界**：`Renderer::render` 在 controller 的 10Hz tick 线程被调用，但它**只负责把 Frame 推进 channel**；真正 D3D11 绘制在 renderer 自己的 60fps 线程。这样渲染阻塞不拖慢 tick，tick 频率不影响渲染平滑度（spec §8.3）。

---

## 依赖说明（windows crate 新增 feature）

```toml
[target.'cfg(windows)'.dependencies.windows]
version = "0.61"
features = [
    # Plan 2 既有
    "Win32_Foundation",
    "Win32_System_LibraryLoader",
    "Win32_System_Threading",
    "Win32_UI_WindowsAndMessaging",
    # Plan 3 新增 —— D3D11 设备与渲染
    "Win32_Graphics_Direct3D11",      # ID3D11Device/Context/Texture/RendertargetView
    "Win32_Graphics_Direct3D",         # D3D_FEATURE_LEVEL_* / D3D11CreateDevice
    "Win32_Graphics_Dxgi",             # IDXGIDevice / IDXGISwapChain / IDXGIFactory
    "Win32_Graphics_Direct3D11on12",   # （若需 D3D12 互操作；WGC 主路径不一定需要，先按需）
    # Plan 3 新增 —— overlay 透明窗口
    "Win32_Graphics_Gdi",              # GetWindowRect / 屏幕坐标 / DWM 合成
    # Plan 3 新增 —— WGC 捕获
    "Graphics_Capture",                # GraphicsCaptureItem / GraphicsCaptureSession / Direct3D11CaptureFramePool
    "Graphics_Capture_DirectX11",      # DirectX11CaptureFramePool 转换桥
    "Graphics_DirectX_Direct3D11",     # IDirect3DDevice (WinRT) 与 Win32 ID3D11Device 桥接
    "Graphics_DirectX_Direct3D",       # DirectX 通用接口
]
```

> **feature 取舍**：以上是 D3D11 渲染 + WGC 捕获的最小集合。实现中若编译报某 API 未找到，按「逐个加、不加整块」原则补。`Win32_Graphics_Direct3D11on12` 标注「按需」——若 WGC 主路径不要求 D3D12 互操作则删掉，避免拉无用代码。

> **D3D11on12 互操作说明**：WGC 返回的 `IDirect3DDevice`（WinRT）需与渲染用的 Win32 `ID3D11Device` 桥接。标准做法：渲染设备 `IDXGIDevice` → `CreateDirect3D11DeviceFromDXGIDevice` → 得到 `IInspectable` 取 `IDirect3DDevice`，WGC `Direct3D11CaptureFramePool::Create` 用它。捕获的帧是 `IDirect3DSurface`，通过 `CreateDirect3D11SurfaceFromDXGISurface`/`IDirect3DDxgiInterfaceAccess::GetInterface` 取回 `ID3D11Texture2D`，喂 shader。这条桥接链是 Plan 3 最易踩坑处，Task 4 单独验证。

---

## File Structure（本计划产出）

- `Cargo.toml` — **Modify**：加 D3D11/WGC feature。
- `src/lib.rs` — **Modify**：`#[cfg(windows)] pub mod renderer;`。
- `src/renderer/mod.rs` — **Create**：子模块导出 + `D3D11Renderer` 主体（独立渲染线程 + channel + impl Renderer）。
- `src/renderer/overlay.rs` — **Create**：透明置顶 overlay 窗口（`WS_EX_LAYERED|WS_EX_TRANSPARENT|WS_EX_TOPMOST`，DWM 逐像素 alpha）。
- `src/renderer/d3d.rs` — **Create**：D3D11 设备 + SwapChain + 全屏三角形 vertex shader（覆盖屏幕）+ pixel shader 调度。纯设备/管线骨架。
- `src/renderer/shader.rs` — **Create**：HLSL 源码内嵌（或 `include_str!` 加载 .hlsl）+ 常量缓冲（u_load/u_dim/u_time/u_resolution）+ 编译。
- `src/renderer/capture.rs` — **Create**：`CaptureSource` trait + `WgcCaptureSource`（WGC 真实捕获）+ `ProceduralCaptureSource`（降级：shader 程序化星云）+ `on_target` 门控。
- `src/renderer/painter.rs` — **Create**：编排——每帧把 {Frame, 捕获纹理/程序化背景} 组合成 shader uniform，画全屏三角形。
- `examples/overlay_demo.rs` — **Create**：跑 30 秒，用 mock provider 让 load 从 0 扫到 100 再回落，肉眼验证黑洞涨缩+施压+（若同意）真实捕获透镜。
- `tests/renderer_contract.rs` — **Create**：renderer 不做真实绘制单测（无法在无头 CI），只验证 `Renderer` trait 契约与降级路径可构造。
- `shaders/blackhole.hlsl` — **Create**：移植自 ShaderToy 的黑洞 raymarching HLSL（注明出处与许可证）。

**非产出**：Tauri 外壳（Plan 5）、配置 TOML 加载（Plan 5）、托盘（Plan 5）、首次同意对话框（Plan 5，但 capture 门控逻辑在本 plan 实现，Plan 5 只接 UI）。

---

## 关键设计决策（实现值）

- **全屏 overlay**：overlay 是覆盖主显示器全屏的透明窗口，黑洞画在其上。**不在本 plan 做目标窗口精确对齐**（避免 GetWindowRect/DPI/多显示器复杂度）；spec §8.3 的「只画目标窗口大小」留作优化。全屏 overlay 实现最简，填充率靠 GPU 承担。
- **点击穿透**：`WS_EX_TRANSPARENT` 让鼠标点击穿透到下层；Working/Dimming 状态全程穿透。ForcedBreak 的「模态捕获输入」留 Plan 5（需切 overlay 为非透明+捕获，涉及输入路由，超出渲染 plan 范围）。本 plan ForcedBreak 仅做深暗视觉，不做真实输入捕获。
- **shader 移植来源**：从公开 ShaderToy 黑洞 demo 移植 HLSL。候选：经典的 [Shadertoy black hole](https://www.shadertoy.com/) 类 demo（实现时选一个许可证允许的，在文件头注明出处+作者+链接）。移植要点：GLSL→HLSL 语法（`vec3`→`float3`、`mix`→`lerp`、`fract`、`mod`）、坐标系一致。
- **shader 入口**：pixel shader 全屏三角形，输入 `SV_Position` + UV。主循环 raymarch ~64 步（中等画质，可配）。uniform 通过 cbuffer 每帧更新。
- **降级触发**：WGC 不可用（OS < Win10 2004、CreateFromWindow 失败、用户未同意）→ `ProceduralCaptureSource`：shader 内程序化生成星云/噪点背景替代 `u_capturedTexture`。用一个 `u_has_capture` uniform 切换 shader 分支。
- **捕获门控**：`on_target=false` 时立即停止 `GraphicsCaptureSession`、释放捕获纹理、`u_has_capture=0` 切程序化背景。`on_target=true` 且已同意才创建/恢复会话。
- **同意门控**：本 plan 用一个 `capture_consent: bool` 运行时参数（Plan 5 从配置/UI 来）。`false` 时永远走程序化背景，连 WGC 会话都不创建。
- **渲染线程**：`D3D11Renderer::new` 起 60fps 线程，循环 `painter.paint()`；`render(&Frame)` 只把 Frame 经 `crossbeam`/`std::sync::mpsc` 推进 channel（非阻塞，drop 旧的）。线程退出靠 Drop 发送 `None`。
- **D3D11 设备**：硬件 feature level 11_0，失败回 9_3。`D3D11_CREATE_DEVICE_BGRA_SUPPORT`（WGC 互操作要求）。

---

## Task 1: 依赖 + 模块骨架 + D3D11 设备/管线

**Files:** Modify `Cargo.toml`, `src/lib.rs`; Create `src/renderer/{mod,overlay,d3d,shader,capture,painter}.rs`（后五个先占位，本 task 只填 d3d.rs）

负责：建立 renderer 模块树，创建 D3D11 设备 + SwapChain，能清屏到一个 HWND。验证设备创建这条最易踩坑的路走通。

- [ ] **Step 1: 改 Cargo.toml 加 D3D11/Gdi feature**（先只加 d3d.rs 需要的，后续 task 按需补 capture/overlay 的）

```toml
# 在既有 windows features 列表追加：
    "Win32_Graphics_Direct3D",
    "Win32_Graphics_Direct3D11",
    "Win32_Graphics_Dxgi",
    "Win32_Graphics_Gdi",
```

- [ ] **Step 2: 改 src/lib.rs**

```rust
#[cfg(windows)]
pub mod renderer;
```

- [ ] **Step 3: src/renderer/mod.rs（占位 + 子模块）**

```rust
//! D3D11 渲染器（spec §4.1 renderer/）。仅 cfg(windows)。
//! controller 经 &mut dyn Renderer 消费，不反向依赖。
mod d3d;
mod overlay;     // Task 2
mod shader;      // Task 3
mod capture;     // Task 4
mod painter;     // Task 5
```

- [ ] **Step 4: src/renderer/d3d.rs —— 设备 + SwapChain（无窗口版先编译通过）**

实现 `D3D11Context`：`new()` 创建 `ID3D11Device` + `IMMDeviceContext`（`D3D11CreateDevice`，flag `BGRA_SUPPORT`），创建 `IDXGISwapChain`（需 HWND，本步先接受 hwnd 参数）。

```rust
//! D3D11 设备与交换链骨架。
use windows::Win32::Graphics::Direct3D::{D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_9_3};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
    ID3D11Device, ID3D11DeviceContext,
};
use windows::Win32::Graphics::Dxgi::{IDXGISwapChain, DXGI_SWAP_CHAIN_DESC, ...};

pub struct D3D11Context {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub swapchain: IDXGISwapChain,
}

impl D3D11Context {
    /// hwnd：渲染目标窗口句柄；width/height：客户区像素。
    pub fn new(hwnd: HWND, width: u32, height: u32) -> Result<Self, ...> { ... }
}
```

> 实现细节由执行者按 windows-rs 0.61 API 填充（`D3D11CreateDevice` 返回 Result、SwapChain desc 字段名）。本步**编译通过即过**，不跑绘制（无窗口）。

- [ ] **Step 5: 占位文件**（overlay/shader/capture/painter 各一行注释）
- [ ] **Step 6: `cargo build` 通过**
- [ ] **Step 7: 提交** `feat(renderer): scaffold module + d3d11 device/swapchain`

---

## Task 2: 透明置顶 overlay 窗口

**Files:** `src/renderer/overlay.rs`

负责：创建全屏透明置顶窗口（`WS_EX_LAYERED|WS_EX_TRANSPARENT|WS_EX_TOPMOST`|`WS_POPUP`），用 DWM `DwmExtendFrameIntoClientArea` + margin -1 实现逐像素 alpha 合成，返回 HWND 给 d3d.rs。每帧从 SwapChain 取 backbuffer 用 alpha 预乘 blend 输出。

- [ ] **Step 1: 注册窗口类 + CreateWindowExW**（全屏，覆盖主显示器 `GetSystemMetrics(SM_CXSCREEN/SM_CYSCREEN)`）
- [ ] **Step 2: DWM 透明合成**（`DwmExtendFrameIntoClientArea` margin={-1}，`DWMWA_COLORADVOCATION`/层级正确）
- [ ] **Step 3: overlay 窗口的消息泵**（renderer 线程跑 `PeekMessage` + `DispatchMessage`，与 Plan 2 hook 线程同理）
- [ ] **Step 4: `cargo build` 通过**（真实显示靠 Task 6 demo 验证）
- [ ] **Step 5: 提交** `feat(renderer): transparent topmost fullscreen overlay window`

---

## Task 3: 黑洞 HLSL shader（移植 ShaderToy）

**Files:** `shaders/blackhole.hlsl`, `src/renderer/shader.rs`

负责：移植一个公开 ShaderToy 黑洞 raymarching demo 为 HLSL；内嵌或 `include_str!`；编译为 vertex（全屏三角形）+ pixel shader；常量缓冲 `cbuffer FrameConstants { float u_load; float u_dim; float u_time; float2 u_resolution; uint u_has_capture; }`。

- [ ] **Step 1: 选定 shader 来源 + 记录出处**（在 blackhole.hlsl 文件头注明：原 ShaderToy 链接、作者、许可证状态——多数 ShaderToy 内容按 CC BY-NC-SA，需确认或选 MIT/公共域等效实现）
- [ ] **Step 2: 移植 HLSL**（GLSL→HLSL：`vec3`→`float3`、`mix`→`lerp`、`iTime`→`u_time`、UV 坐标、`u_resolution` 归一化）
- [ ] **Step 3: shader.rs 编译**（`D3DCompile` from `d3dcompiler_47`——需 `Win32_Graphics_Direct3D11` 含 `D3DCompile`？实际 D3DCompile 在 `Win32_Graphics_Direct3D11` 不在，它在 `Win32_Graphics_Direct3D`/单独的 d3dcompiler。实现时确认 feature：可能需运行时 `LoadLibrary("d3dcompiler_47.dll")` + `D3DCompile`，或预编译 blob 内嵌。**决策：运行时 D3DCompile**，加 feature `Win32_Graphics_Direct3D11` 的 `D3DCompile` 别名，或用 `windows` 的 `Windows::Win32::Graphics::Direct3D::D3DCompile`——确认后加 feature）
- [ ] **Step 4: 创建 vertex/pixel shader + input layout + cbuffer**
- [ ] **Step 5: `cargo build` 通过**
- [ ] **Step 6: 提交** `feat(renderer): blackhole raymarching HLSL shader (ported)`

---

## Task 4: WGC 捕获 + D3D11 互操作 + 降级

**Files:** `src/renderer/capture.rs`

负责：`CaptureSource` trait + `WgcCaptureSource`（从目标 HWND 创建 `GraphicsCaptureItem` + `Direct3D11CaptureFramePool`，D3D11 设备互操作取回 `ID3D11Texture2D` 喂 shader）+ `ProceduralCaptureSource`（降级）+ `on_target`/`consent` 门控。**这是 Plan 3 最复杂的 task。**

- [ ] **Step 1: CaptureSource trait**

```rust
pub trait CaptureSource {
    /// 返回当前捕获帧的 shader resource view（或 None 表示无可用帧→用程序化背景）。
    fn current_frame_srv(&mut self) -> Option<ID3D11ShaderResourceView>;
    /// on_target 或 consent 变化时调用：启停捕获会话。
    fn set_active(&mut self, active: bool);
}
```

- [ ] **Step 2: D3D11 互操作辅助**（`CreateDirect3D11DeviceFromDXGIDevice` 把渲染设备转 `IDirect3DDevice`；`IDirect3DDxgiInterfaceAccess::GetInterface` 把捕获帧 `IDirect3DSurface` 转 `ID3D11Texture2D`）。**单独验证这步编译+不 panic**（真实捕获 Task 6 验证）。
- [ ] **Step 3: WgcCaptureSource**

```rust
pub struct WgcCaptureSource {
    item: GraphicsCaptureItem,
    session: GraphicsCaptureSession,
    frame_pool: Direct3D11CaptureFramePool,
    latest: Mutex<Option<ID3D11Texture2D>>,  // 捕获回调更新，painter 读
    ...
}
impl WgcCaptureSource {
    pub fn try_new(device: &ID3D11Device, target_hwnd: HWND) -> Result<Self, CaptureError> { ... }
}
```

> `GraphicsCaptureItem::CreateFromWindow` 在 Win10 2004+ 才有，失败时 `try_new` 返回 Err，上层回落 `ProceduralCaptureSource`。需 `GetWindowIdFromWindow(hwnd)` 转 `WindowId`。

- [ ] **Step 4: ProceduralCaptureSource**（不持有真实纹理，painter 据 `u_has_capture=0` 走 shader 内程序化分支）
- [ ] **Step 5: 门控**：`WgcCaptureSource::set_active(false)` 调 `session.Stop()` + 清 `latest`；`set_active(true)` 调 `session.Start()`。consent=false 的 WgcCaptureSource 根本不构造。
- [ ] **Step 6: `cargo build` 通过**
- [ ] **Step 7: 提交** `feat(renderer): WGC capture with D3D11 interop + procedural fallback`

---

## Task 5: Painter + D3D11Renderer（组装 + 独立渲染线程 + impl Renderer）

**Files:** `src/renderer/painter.rs`, `src/renderer/mod.rs`

负责：painter 每帧把 {Frame→uniform, 捕获纹理/程序化} 组合画全屏三角形；D3D11Renderer 起独立 60fps 线程，`render(&Frame)` 经 channel 推 Frame，线程循环 `painter.paint()`。impl `Renderer` trait。

- [ ] **Step 1: painter.rs** —— `Painter::paint(&mut self, frame: &Frame, time: f32)`：更新 cbuffer（u_load=load/100, u_dim=按 state, u_time, u_has_capture）、绑定 capture srv（或 None）、画 3 顶点全屏三角形、SwapChain Present（`DXGI_PRESENT_...`，vsync on）。
- [ ] **Step 2: D3D11Renderer 主体**

```rust
pub struct D3D11Renderer {
    tx: std::sync::mpsc::Sender<Option<Frame>>,  // None = 退出
    handle: Option<JoinHandle<()>>,
}
impl D3D11Renderer {
    /// consent：是否允许真实捕获（Plan 5 从配置来，本 plan 测试时传 false 走降级）。
    /// target_hwnd：捕获目标窗口（None = 纯程序化）。
    pub fn new(consent: bool, target_hwnd: Option<HWND>) -> Result<Self, RenderError> {
        // 起渲染线程：建 overlay HWND + D3D11Context + shader + capture + painter
        // 循环 60fps：recv 最新 Frame（非阻塞，drop 旧的）+ painter.paint
    }
}
impl Renderer for D3D11Renderer {
    fn render(&mut self, frame: &Frame) {
        let _ = self.tx.send(Some(*frame));  // 非阻塞，drop 旧的靠 try_recv
    }
}
impl Drop for D3D11Renderer { ... 发 None + join ... }
```

- [ ] **Step 3: channel 同步策略**：`render()` 用 `send` 覆盖（换 mpsc `try_iter` 取最后）；渲染线程 `try_recv` 取最新，无则用上一帧（保证 60fps 不依赖 10Hz tick）。
- [ ] **Step 4: `cargo build` + `cargo test` 通过**
- [ ] **Step 5: 提交** `feat(renderer): D3D11Renderer with dedicated render thread + Renderer impl`

---

## Task 6: overlay_demo（端到端视觉验证）

**Files:** `examples/overlay_demo.rs`

负责：用 `ScriptedProvider`（Plan 1 mock）让 load 0→100→0 扫描，喂给 `Controller` + `D3D11Renderer`，肉眼验证黑洞涨缩 + Dimming 压暗 + 降级背景。可选接真实 platform provider + WGC 捕获（传 target_hwnd）。

- [ ] **Step 1: example 主体**（ScriptedProvider 扫描 load；D3D11Renderer::new(consent=false, target_hwnd=None) 走程序化降级，确保任何机器能跑）
- [ ] **Step 2: 运行验证（前台）**：`cargo run --example overlay_demo`，肉眼 QA：load 0→黑洞小、load↑→黑洞涨大+盘亮、Dimming→背景压暗、load↓→缩小。
- [ ] **Step 3: 可选 WGC 验证**：传真实 target_hwnd + consent=true，验证透镜扭曲真实窗口像素（需 Win10 2004+ + 用户同意；记录在 demo 注释里如何开）。
- [ ] **Step 4: 提交** `examples: overlay_demo visual verification of blackhole grow/shrink`

---

## Task 7: renderer 契约测试 + 文档

**Files:** `tests/renderer_contract.rs`

负责：renderer 真实绘制无法在 CI 测，但验证：① `D3D11Renderer` 可构造（降级模式，consent=false）；② 它 impl 了 `Renderer`，能被 `Controller` 当 `&mut dyn Renderer` 使用；③ 降级路径不 panic。

- [ ] **Step 1: 契约测试**（构造 D3D11Renderer 降级模式 + Controller + 跑若干 tick 不 panic + Drop 干净）
- [ ] **Step 2: 提交** `test(renderer): contract + degraded-mode construction`

---

## 完成标准（Plan 3 Definition of Done）

- `cargo test` 全绿（Plan 1+2 的 34 + renderer 契约测试；真实 D3D11/WGC 不进 CI 单测）。
- `cargo run --example overlay_demo`（前台）肉眼可见：黑洞随 load 涨缩、Dimming 压暗、降级程序化背景；WGC 模式下（Win10 2004+ + 同意）可见透镜扭曲真实窗口像素。
- `controller.rs` **未引入 `use crate::renderer`**（`D3D11Renderer` 经 `&mut dyn Renderer` 接入）。
- 隐私硬规则：捕获纹理只在显存，不拷回 RAM、不落盘、不导出；切走目标立即停止捕获。代码注释显式声明。
- `windows` 依赖新增 feature 按「逐个加」原则，无整块冗余。
- D3D11 渲染在独立线程，`render()` 非阻塞，渲染卡顿不拖慢 10Hz tick。

---

## 终审遗留项（final review 后填写，格式参考 Plan 1）

> 占位：Plan 3 终审后记录 GO/NO-GO 与留给后续 plan 的 should-fix。预期候选：
> 1. ForcedBreak 模态输入捕获未实现（本 plan 仅深暗视觉）—— Plan 5 切 overlay 为非透明+捕获。
> 2. overlay 全屏而非目标窗口精确对齐 —— 填充率优化，等真实负载评估。
> 3. shader 移植许可证 —— 确认 ShaderToy 源许可证或替换为公共域实现。
> 4. WGC 捕获黄边/独占全屏黑帧 —— spec §10 边界处理，Plan 5/6 补。
> 5. 多显示器/DPI —— 本 plan 单显示器全屏，多显示器扩展留后续。
