# Renderer：黑洞可见性修复 + 视觉效果

> 日期：2026-06-19  
> 关联：plan-3-renderer、design §4.1/§8.1/§8.2  
> 状态：已完成，黑洞在真实桌面会话可见

## 背景

Plan 3 renderer 端到端打通后，`overlay_demo` 运行无报错（Present 全程 `S_OK`，
渲染线程正常推帧），但**屏幕上看不到黑洞**——任务栏出现进程，却无可视窗口呈现。
本文件记录从「完全不可见」到「卡冈图雅式可见黑洞」的完整排查与修复过程，
供后续回归调试参考。

---

## 一、环境（首次运行前置）

项目 pin `stable-x86_64-pc-windows-msvc`（见 `rust-toolchain.toml`），需要：

| 组件 | 安装方式 |
|------|---------|
| Rust 工具链 | rustup（`rustup-init.exe -y --default-toolchain stable-x86_64-pc-windows-msvc`） |
| VS Build Tools | winget `Microsoft.VisualStudio.2022.BuildTools` + `VCTools` 工作负载（提供 `link.exe` + Windows SDK） |

cargo 装在 `%USERPROFILE%\.cargo\bin`，需加入用户 PATH（`SetEnvironmentVariable(...,'User')`）。

**已知拦截**：360 安全卫士（`360tray` + `ZhuDongFangYu` 主动防御）会锁定 Rust
编译生成的 build-script / proc-macro exe，导致 `error: could not run build script
(os error 5)`。解决：退出 360 主程序，或将项目目录加入 360 信任区。

---

## 二、根因：缺少 input layout（黑洞不可见的真正原因）

### 症状

- `overlay_render_diag`（用 `ClearRenderTargetView` 画半透明红）→ **可见**
- `overlay_demo`（用 `Draw` + 黑洞 shader）→ **不可见**

两者共用同一套 overlay 窗口 + D3D11 设备 + SwapChain，唯一差异是**绘制方式**。

### 排查路径（每一步都用实测推翻/确认假设）

| 假设 | 验证 | 结论 |
|------|------|------|
| shader 太暗 | 改成全屏不透明绿 `return float4(0,1,0,1)` | 仍不可见 → 不是亮度 |
| 后台线程结构问题 | 主线程跑黑洞（临时 `overlay_bh_diag`） | 仍不可见 → 不是线程 |
| 背面剔除 cull | 加 `RasterizerState(CULL_NONE)` | 仍不可见 → 不是 cull |
| **Draw 没写进 RTV** | **CopySubresourceRegion 读 backbuffer 中心像素** | **每帧 `(0,0,0,0)`** → Draw 产生零几何体 |

读回像素那一刻锁定：`Draw` 调用没有把任何像素写进 backbuffer。

### 根因

`blackhole.hlsl` 的 `vs_main` 用 `SV_VertexID` 生成全屏三角形（无需顶点缓冲，
3 顶点覆盖 NDC）。但 D3D11 有一条规则：**即使没有顶点缓冲，也必须调用
`IASetInputLayout` 绑定一个 input layout**（用 VS 编译产物 `CreateInputLayout`
创建，可为空元素数组）。否则 Input Assembler 不向 Vertex Shader 供应顶点，
`Draw` 产生零几何体 → backbuffer 全空 → DWM 合成出空（透明）画面。

这正是「`Clear` 可见、`Draw` 不可见」的原因：`ClearRenderTargetView` 绕过整个
draw 管线直接写 RTV，不受 input layout 影响。

### 修复（commit `5431fa2`）

| 文件 | 改动 |
|------|------|
| `src/renderer/shader.rs` | `Shaders` 加 `input_layout: ID3D11InputLayout` 字段；`new()` 用 VS blob `CreateInputLayout(&[], blob)` 创建空 layout |
| `src/renderer/painter.rs` | `PaintContext` 加 `input_layout` 字段；`paint_frame` 绑 `IASetInputLayout` + `IASetPrimitiveTopology(TRIANGLE_LIST)` |
| `src/renderer/mod.rs` | `render_loop` 构造 `PaintContext` 时传 `input_layout` |

修复后读回像素变为 `(0,255,0,255)`（全屏绿），铁证 Draw 已写入。

### 附带修复

`paint_frame` 每帧先 `ClearRenderTargetView(alpha=0)`：`DXGI_SWAP_EFFECT_DISCARD`
不保证 backbuffer 内容，shader 用预乘 alpha 输出（视界外 alpha≈0），未清时残留
不透明像素会破坏「桌面透出」效果。

---

## 三、视觉效果：卡冈图雅式吸积盘（commit `d917228`）

input layout 修复让黑洞可见后，吸积盘从简单环带升级为电影《星际穿越》
「卡冈图雅」式实现。改动仅在 `shaders/blackhole.hlsl` 的 `ps_main`：

### 视觉元素（对照 spec §8.1）

| 元素 | spec 要求 | 实现 |
|------|----------|------|
| 事件视界 | 纯黑 | `if (r < r_eh) { col=0; alpha=1 }`（不变） |
| 吸积盘 | 发光、多普勒偏移、随 u_time 旋转 | 卡冈图雅式（见下） |
| 光子环 | 亮圈 | 爱因斯坦环：视界边缘极亮细环（衰减 `*30`） |

### 卡冈图雅式吸积盘细节

- **多普勒相对论性 beaming**：盘朝观察者运动一侧显著增亮（`beaming` 2.2）、
  远离一侧压暗（0.35）——「半亮半暗」标志特征。
- **螺旋旋臂**：极角与半径耦合的密度波（`spin_phase = angle - t*6 + u_time*0.9`），
  粗旋臂（3 臂）× 细条纹调制，随时间旋转。
- **内热外冷色温**：蓝白（高温内）→ 黄 → 橙 → 暗红（低温外）多段渐变。
- **多普勒色移**：亮侧蓝移、暗侧红移。
- **内锐外柔**：内边缘 sharp，外边缘 `smoothstep` 渐隐，融入桌面。

### alpha / 透明度策略

远处桌面完全透出（无全屏蒙版），可见性集中在黑洞本体：
- 视界 / 盘 / 光子环：高 alpha
- 视界外一圈局部光晕 `exp(-(r-r_eh)*7)`：勾勒轮廓，远处快速衰减到 0

---

## 四、关于「放大缩小」「时空弯曲」的说明

- **放大缩小**：`overlay_demo` 演示脚本用正弦让 load 在 0–100 振荡，演示黑洞涨缩。
  真实运行时 load 由注意力监测驱动（Plan 5），自然变化。
- **时空弯曲**：shader 含引力透镜 `lens_warp`，但它扭曲**背景**。`overlay_demo`
  默认 `consent=false`（spec §9 隐私默认安全）走程序化星云背景，星点扭曲不明显。
  捕获真实窗口（`consent=true` + 目标 HWND）时，透镜扭曲捕获的桌面/代码，
  「时空弯曲吞噬代码」效果才显现。

---

## 五、验证

- `cargo test`：44 单元 + 5 集成全过
- 人眼 QA（`cargo run --example overlay_demo`）：屏幕中央可见纯黑视界 +
  发光吸积盘（半亮半暗、旋转旋臂、色温渐变）+ 爱因斯坦环；load 升高时变大变亮；
  远处桌面透出无蒙版。

---

## 六、保留的诊断工具

- `examples/overlay_diag.rs`：创建 OverlayWindow 后打印窗口真实状态
  （`IsWindowVisible` / EX style / WindowRect / 屏幕尺寸），回归「看不见」类问题时
  用于区分窗口问题 vs 渲染问题。**保留**。
- `examples/overlay_render_diag.rs`：画半透明红验证 D3D11+overlay 管线。根因已解决，
  **已删除**（如需回归，可按本文件第二节步骤重建：Clear vs Draw 对比 + 读像素）。

---

## 六·补、物理测地线升级（geodesic shader）

卡冈图雅式「手绘」盘只是视觉近似——`lens_warp` 把像素朝外推、椭圆盘 + 高斯光子环
三者互不关联，没有真正的光线追踪。本次把它升级为**基于物理的测地线追踪**，
移植自参考项目 `ghostty-blackhole`（s13k，MIT）的 `blackhole.glsl`，其方法又源自
Eric Bruneton 的非旋转黑洞渲染论文。

### 移植来源与许可证

- **参考项目**：`D:\OtherCode\ghostty-blackhole\blackhole.glsl`，s13k，MIT License
  (Copyright (c) 2026 s13k)。方法基于 Eric Bruneton,
  *Real-time High-Quality Rendering of Non-Rotating Black Holes*
  (https://ebruneton.github.io/black_hole_shader/)。
- 本项目（attention_blackhole）同为 MIT，MIT↔MIT 兼容。`shaders/blackhole.hlsl`
  文件头已重写许可证声明，注明移植来源并保留上游版权。
- **未移植**：ghostty 版的 Lissajous 漂移 / pomodoro / token / demo preset tour
  （终端场景特有，本项目用 `u_load` 注意力模型驱动）。

### 物理核心

每个**近场**像素（impact parameter `b < bmax`）逐帧 leapfrog 积分自己的零测地线：

```
x'' = -(3/2) h² x / r⁵        （精确 Schwarzschild 光子弯曲，Binet 形式；h=|x×v| 守恒）
```

下列现象全部是积分的**涌现结果**，非手画：
- **事件视界阴影**：`b < b_crit = 2.598 r_s` 的射线旋进视界 → 黑（连背景光都吞噬）。
- **引力透镜**：逃逸射线投影回 capture 平面，捕获的目标窗口像素弯曲、放大、
  在爱因斯坦环内出现镜像次像——这就是 spec §8.1 的「吞噬你的代码」。
- **光子环**：绕 `r = 1.5 r_s` 光子球多圈的射线聚焦出的极亮细环。光子环是否正确
  是物理正确性的**强指示器**。
- **卡冈图雅式吸积盘**：开普勒薄盘，射线可多次穿越盘面——远侧弧越过视界上方/下方
  （星际穿越式）。色温用 Shakura–Sunyaev 温度剖面 → 黑体色，相对论多普勒+引力频移
  `g = √(1−1.5 r_s/r)/(1−β·k̂)` 与 beaming 强度 `g^N`，引力时间膨胀让内圈图案冻结。

**远场**（`b >= bmax`）用解析弱场偏折 `α ≈ 2r_s/b`（有限相机拟合，与测地线在 `b=bmax`
边界偏差 <1%，消除圆形接缝）。

### 本项目 overlay 适配（与 ghostty 终端版的差异）

1. **预乘 alpha 包络**（新增层，原 shader 无）：overlay 是悬浮透明窗，远场须 → alpha=0
   （桌面透出），视界附近 → alpha=1（遮挡）。`a = window*vis`，捕获射线 `a=vis`，
   盘遮挡/盘光强的位置 `a` 抬升。输出 `float4(col*a, a)` 与 painter 现状一致。
2. `u_load`（注意力负荷）→ 主强度 `I` + 影子半径 `rh`（2%→35% 短边，spec §7），
   替代 ghostty 的 pomodoro/token 调度。
3. `u_has_capture` 分支：=1 采样捕获的目标窗口纹理（lensed sky plane），=0 走程序化
   星云降级（spec §8.4）。
4. `u_dim`（Dimming/ForcedBreak）乘进 `col`。

### `N_STEPS` 旋钮

每像素测地线积分步数，`shaders/blackhole.hlsl` 顶部 `#define N_STEPS 32`（默认）。
仅视界附近的像素付出此开销（远场走解析弱场）。按目标 GPU 调：

| 档 | N_STEPS | 适用 |
|----|---------|------|
| 保守 | 20 | 核显 / 4K，最保险稳 60fps |
| **平衡（默认）** | **32** | 核显 1080p 约 8-12ms/帧，独显轻松 60fps |
| 满血 | 48 | 与 ghostty 原版一致，物理最精确，核显可能 30-45fps |

### 线性采样器（plan-3 should-fix #4 闭合）

本次一并加了线性采样器（`shader.rs::create_linear_sampler`，`FILTER_MIN_MAG_MIP_LINEAR`
+ `ADDRESS_MIRROR`），painter 每帧 `PSSetSamplers(s0)`。让透镜扭曲后的捕获纹理边缘平滑、
不边缘涂抹（`ADDRESS_MIRROR` 对应 shader 的 `mirrorUV` 语义）。plan-3 终审的 should-fix #4
闭合。

### GLSL→HLSL 可移植性陷阱（移植时逐项核对，留此供回归参考）

- `mod(a,b)`（GLSL）vs `fmod(a,b)`（HLSL）对负数语义不同 → `vnoiseWrapY` 的无缝包裹
  依赖 GLSL 语义，定义 `#define glmod(x,y) ((x)-(y)*floor((x)/(y)))` 复刻。
- `atan(y,x)`→`atan2`；`inversesqrt`→`rsqrt`；`mix`→`lerp`；`fract`→`frac`；
  `vec3`→`float3`；`texture(iChannel0,uv)`→`u_capturedTexture.Sample(u_sampler,uv)`。

### 验证

- `cargo test`：49 passed（44 单元 + 5 集成），与升级前一致——shader.rs 的 `FrameConstants`
  32 字节契约不变，无测试回归。
- `cargo run --example geodesic_demo`（真实桌面会话）：D3D11Renderer 构造成功（=shader
  经 `D3DCompile` 编译通过 + sampler + overlay + 渲染线程就绪），load 0→100→0 推帧无 panic。
  光子环 / 透镜扭曲 / 卡冈图雅盘的视觉正确性需人眼 QA。

### 盘参数对齐 tuner Defaults（质感修复）

初版移植直接用了 `blackhole.glsl` 文件内的常量，那是 **Inferno 预设**（厚重熔岩观感），
渲染出来偏暖、过曝、纹理粗。后对照 `tuner/Sources/BlackHoleTuner/ParamSpec.swift` 的
`def` 字段（s13k 打磨的**电影级 Defaults**）重调，差异最大的几项：

| 参数 | 初版(Inferno) | **Defaults(电影级)** | 影响 |
|------|--------------|---------------------|------|
| `DISK_TEMP` | 5500（暖） | **8500**（白热偏蓝） | 内圈色温层次 |
| `DISK_INNER` | 1.8 | **3.0**（ISCO） | 干净内边缘 |
| `DISK_OPACITY` | 0.90（厚） | **0.65** | 不挡死背景 |
| `DISK_GAIN` | 2.20（过曝） | **1.00** | 不过曝 |
| `DOPPLER_MIX` | 0.60 | **1.00**（满物理） | 强多普勒半亮半暗 |
| `DISK_CONTR` | 1.60（糙） | **0.90**（平滑） | 丝缕不粗糙 |
| `LENS_DEPTH` | 13.0 | **4.00** | 弯曲适中不过度 |

密度项保持与参考 `blackhole.glsl:555` 完全一致（`gain*2.2*density*tprof²*boost`）——
`tprof²` 是色温渐变的关键，不能简化。

### Dimming 压暗掩盖盘质的踩坑

QA 时发现盘偏暗/发灰，根因**不在 shader**：demo 在 `load>80` 时设 `AppState::Dimming`，
painter 据 `dimming_for_state`（painter.rs:142-146）派生 `u_dim=0.6`，shader 全局
`col *= (1-u_dim*0.7)` 把渲染压到 58% —— 正好把白热内圈压成米色。`geodesic_demo` 改为
全程 `AppState::Working`（load 峰值停在 78 < 80 阈值），确保观察的是真实物理盘光。
**教训：质感 QA 必须在 `u_dim=0` 下做，Dimming 是独立功能不应混入画质评估。**

### 透镜畸变不可见（两个叠加根因）

QA 报「黑洞周围没有畸变效果」，诊断出两个独立的叠加 bug：

1. **程序化背景太平滑**（主因）：`u_has_capture=0` 时走 `procedural_background`，原版只有
   深紫蓝平滑渐变 + 稀疏星点——透镜位移在平滑渐变上**根本看不出来**。参考版把终端文字
   当 lensed sky，文字天然有高频细节。修正：给 `procedural_background` 加细密网格（每屏
   48×27 格的亮线）+ 稠密星点，让任何位移都能看出弯曲（畸变的视觉锚点）。
2. **alpha 直接用 lens window**：原移植 `a = window * vis`，而 `window = exp(-(plen/7rh)²)`
   在参考版里**只衰减透镜位移幅度**（不衰减颜色/alpha，参考输出恒 `alpha=1`）。把它直接
   当 alpha → 远场（plen>7rh）整片透明，连畸变带背景一起消失。修正：新增**独立的更宽可见
   包络** `visEnv = vis*exp(-(plen/6.5rh)²)` 作 alpha，`window` 保留只衰减位移幅度——与参考
   解耦，让整个被弯曲的背景区域都可见（不透明遮挡桌面），再往外才淡出。

修正后网格线在洞周明显弯曲、汇聚，爱因斯坦环区域背景被拉伸成切向 smear，畸变清晰可见。
**真实捕获模式（u_has_capture=1，终端/代码）天然有细节，无需网格——网格只服务程序化兜底。**

### 真实屏幕捕获 + overlay 自排除（「扭曲真实屏幕内容」）

前述网格只是程序化兜底的畸变锚点。用户要的是**黑洞扭曲真实的桌面内容**。这需要 WGC
捕获真实屏幕像素喂 shader（spec §8.1/§8.3「吞噬你的代码」）。本次接通真实捕获，支持
两种范围（`CaptureTarget` 枚举）：

- `Window(HWND)`：捕获单个目标窗口像素（spec 原设计「吞噬你的代码」），走 `CreateForWindow`。
- `Monitor(HMONITOR)`：捕获整个显示器的真实桌面（「扭曲整个屏幕」），走 `CreateForMonitor`。

两者共用同一套 WGC FramePool/Session/零拷贝管线（`WgcCaptureSource`），仅创建 item 时分支。

**核心难点：反馈环。** overlay 是全屏置顶窗，直接捕获它所在的显示器会把 overlay 自身
（黑洞渲染结果）也捕获进来 → 无限自指。WGC API **无 exclude-window 能力**（已查证）。
**解法**：`OverlayWindow::new()` 建窗后立即 `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`
（`Win32_UI_WindowsAndMessaging`，已启用 feature），把 overlay 标记为「从捕获排除」。WGC
捕获该显示器时，overlay 区域在捕获纹理里为透明，shader 采样到的就是 overlay **背后**的
真实桌面。这是 Electron/OBS 排除自身窗口的标准手法（Win10 2004+，本项目已要求）；旧 OS
退化为黑块（不致命，记日志继续）。

**接线**：`D3D11Renderer::new(capture_target: Option<CaptureTarget>, consent: bool)`。
HWND/HMONITOR 非 Send，跨线程用 `(kind:u8, raw:usize)` 编码传递，线程内转回。
`Some(target)+consent=true` → WGC 捕获（失败回落程序化）；其余 → 程序化降级（不变）。
门控逻辑保留：`on_target=false` → `set_active(false)` 停捕获清纹理（spec §9）。

**验证**（`cargo run --example capture_demo`，真实桌面会话）：WGC `StartCapture OK`，
截图确认背景是**真实桌面内容**（菜单栏、窗口标题、图标）而非程序化网格，且在洞周**明显
弯曲**。反馈环已破（overlay 不捕获自身）。隐私硬规则遵守：捕获像素只在显存，零拷贝不落盘。

涉及：`overlay.rs`（自排除）、`capture.rs`（`CaptureTarget` + `CreateForMonitor`）、
`mod.rs`（`new` 签名 + 接线）、`examples/capture_demo.rs`（真捕获 demo）。

### 转速 + 帧率（用户反馈「转的但可以更快、帧数可以更高」）

两个独立问题：

1. **盘转速太慢**：`DISK_SPEED` 从 Defaults 3.6 调到 7.0，螺旋纹丝旋转约 2 倍速。
   `DISK_CONTR` 也从 0.9 调到 2.2——Defaults 的 0.9 在多普勒明暗差异下条纹太淡，
   盘看起来像一整块纯色（用户原话「吸积盘一个颜色没花纹」），调高后纹丝明显，旋转随之可见。

2. **帧率被 `thread::sleep` 拖垮**：渲染循环原每帧末尾 `thread::sleep(16ms-elapsed)`，
   受 Windows 默认定时器粒度（15.6ms）影响，容易睡过头（16→31ms），把帧率从 60 拖到 ~30-45fps。
   `Present(1)` 本就对齐 VSync（阻塞到下个刷新周期），sleep 多余且有害。去掉 sleep、仅留
   `yield_now` 让出，渲染线程以显示器刷新率节拍运行——实测从 ~30-45fps 提到 **144fps**（144Hz 屏）。
   盘旋转因此明显更流畅。

**踩坑**：用 `CopyFromScreen`（GDI 截图）诊断 overlay 帧率/旋转是**不可靠**的——GDI 无法捕获
LAYERED + DWM 硬件合成的 overlay 窗口，返回冻结/陈旧帧，会误判「渲染冻结」。改用 CPU 端
渲染迭代计数测帧率（不受截图工具影响）才得到真实 144fps。overlay 视觉正确性只能靠肉眼。

### 渐显盘 + 随大小变色 + 透镜渐强（用户需求增强，相对 ghostty 原版）

用户要的演化观感是：**小洞只是阴影 → 盘逐渐产生 → 洞越大越扭曲周围 → 颜色随大小变化**。
经核对 ghostty 原版（`blackhole.glsl`），原版**只满足「随大小涨缩」**，其余三项是本项目增强：

| 行为 | ghostty 原版 | 本项目实现 |
|------|-------------|-----------|
| 随大小涨缩 | ✅ `rh`∝`I` | ✅ 保留 |
| 盘渐显 | ❌ 第一帧就完整 | ✅ `diskPresence = smoothstep(0.10,0.55,I)`：小洞无盘→中段渐显→大洞满盘 |
| 随大小变色 | ❌ 恒温 5500K | ✅ `tempOf = lerp(TEMP*0.5, TEMP*1.3, I)`：小洞暗红余烬→大洞白蓝核心 |
| 透镜渐强 | ⚠️ 恒定 LENS_DEPTH=13 | ✅ `effectiveLensDepth = lerp(2.5, 6.0, I)`：小洞温和→大洞剧烈 |

- `diskPresence` 同时乘进 `emitc`（盘光强度）和 `trans`（盘遮挡），让小洞阶段只有视界阴影、
  无盘光、不挡背景；中段盘逐渐浮现发光、开始遮挡；大洞满盘。
- `tempOf` 喂 `blackbody(tempOf * tprof * gfac)`：低温黑体色偏暗红橙，高温偏白蓝。
- `effectiveLensDepth` 同时驱动弱场偏折幅度（`(1.29u+0.07)*(lensDepth-2.14u+0.75)`）和
  天平面投影距离（`tpl = (-lensDepth - x.z)/d.z`）——洞越大，周围代码/窗口弯曲越剧烈，
  盘内（测地线区）与盘外（弱场区）的扭曲天然不同（边界 `bmax=rout+3`）。

`LENS_DEPTH` 的 `#define` 保留作参考基准，实际透镜强度现由 `effectiveLensDepth` 按大小驱动。

### 黑雾修复 + 真实桌面透镜（用户反馈「黑雾太大、没扭曲真实背景」）

两个症状同根：旧的 alpha 合成**把透镜区涂成半透明黑**而非显示扭曲后的真实桌面。

**根因 1：alpha 用 visEnv 把一大圈涂半透明黑。** 旧 `a = vis * exp(-(plen/6.5rh)²)`：透镜区
（plen<6.5rh）alpha 一片半透明 → overlay 输出半透明黑色 → 与桌面混合 = 黑雾。但透镜区
`col` 本就是**扭曲后的真实桌面采样**，该完全替换桌面（alpha=1）才能看见扭曲。
**修正**：`lensEdge = smoothstep(3.6rh, 3.0rh, plen)`——plen<3.0rh alpha=1（完全替换，
看到扭曲桌面），3.0→3.6rh 窄带淡出到 0（远场透出真实桌面）。透镜区不再半透明，黑雾消失。

**根因 2：近场逃逸射线的 `toward` 门控提前掐断捕获采样。** 旧
`toward = smoothstep(0.05,0.35,-d.z)`：强弯曲射线（d.z 接近 0，即爱因斯坦环区域）
`toward→0` → 不采样捕获纹理 → bg=0（黑）。但这正是该看到「扭曲/镜像的真实桌面」的区域。
**修正**：去掉 toward 门控，只要射线朝远方（d.z<-0.02）且投影点在天平面后方（tpl>0）就
直接 `u_capturedTexture.Sample(suv)`——suv 是弯曲后的坐标，采到的就是扭曲的真实桌面像素。
弱场路径（b≥bmax）本就直接采样，无需改。

**结果**：透镜区显示**扭曲后的真实桌面**（alpha=1 完全替换），黑雾消除；黑洞后方/侧方的
爱因斯坦环区域也能看到镜像翻转的扭曲桌面，而非黑色。

### 天平面投影 bug（用户反馈「只剩一个大黑圆、周围没扭曲也没光线」）

alpha 修复后透镜区仍是一片黑，根因是**测地线逃逸射线的天平面投影算出 tpl<0**，导致
捕获纹理采样被 `w = tpl>0?1:0` 掐成 0 → bg=黑 → 整个透镜区黑。

几何：射线从 `z=+Z0`（Z0=max(14,rout+5)≈14）出发朝 -z，逃逸条件 `x.z<-Z0`（穿过 z=-14 平面）。
天平面投影 `tpl=(-skyZ-x.z)/d.z`。旧代码用 `skyZ=effectiveLensDepth`（随大小 2.5→6.0），
但逃逸时 `x.z<-14` 已在天平面（z=-2.5..-6）**之后**，`(-6-(-14))/d.z = 8/negative < 0` → tpl<0。
参考版 `Z0=max(14,..)` + `LENS_DEPTH=13` 之所以正常，是因为天平面 z=-13 **紧贴**逃逸平面 z=-14。

**修正**：`skyZ = Z0 - 1`（天平面紧贴逃逸平面内侧，与参考同构），确保 tpl>0；`effectiveLensDepth`
只控制**弱场偏折幅度**，不挪天平面。这是「只剩黑圆」的真正根因——背景采样几何算错了。

### 光子环区黑圆 + lensEdge 范围（用户反馈「黑洞被黑色圆圈包住、盘后黑色背景」）

天平面投影修好后，黑洞**紧邻**的环形区仍黑。两个叠加根因：

**根因 1：强弯曲射线被 `d.z < -0.02` 挡掉，bg 留黑。** 光子环/爱因斯坦环区域的射线被弯到
`d.z >= -0.02`（朝侧面/相机），够不到洞后天平面 → 旧代码直接跳过采样 → bg=黑。这正是「黑色圆圈」
和「盘后黑色背景」的来源（参考版留黑，但终端文字天然黑不明显；overlay 留黑就成了黑圈）。
**修正**：为这类射线加 fallback——按射线最终方向 d.xy 在屏幕上做弱场式扭曲采样，让光子环区
也显示（扭曲/镜像的）桌面而非黑。

**根因 2：lensEdge 范围比近场区窄，把光子环区 alpha 藏成 0。** 近场（测地线）区延伸到
`bmax=rout+3≈12r_s`，映射屏幕 ~4.6rh；旧 `lensEdge=smoothstep(3.6rh,3.0rh,plen)` 在 3.6rh 外
alpha=0，导致 3.6→4.6rh 那段（光子环）虽采到了背景但 alpha=0 不显示。
**修正**：`lensEdge=smoothstep(6.0rh,5.0rh,plen)`，覆盖整个近场区，光子环区也 alpha=1 显示。

### 视界透明化（用户反馈「黑洞还是包裹在黑圆里，能不能弄成透明的」）

之前落入视界的射线画成**纯黑不透明圆盘**（`col=0, a=vis`）——这是物理上的事件视界，但用户
觉得碍眼（一个死黑圆盘）。修正：视界不再画死黑，而是采样**镜像+强扭曲的桌面**（朝洞心反方向、
放大位移的坐标），模拟光线绕黑洞旋进后的二次像（爱因斯坦环内侧），视觉上像「洞内透出扭曲的
对面桌面」。盘光（emitc，视界附近也累积，光子环明亮）仍叠在上面。视界区 alpha 走 lensEdge（≈1），
与透镜区连续，不再有「黑圆 vs 透镜区」的硬边。`lensEdge` 同时从 `smoothstep(6rh,5rh)` 放宽到
`smoothstep(7rh,6rh)` 确保整个近场区（含光子环）都 alpha=1 显示。

**注意**：这是相对物理正确性的**主观观感调整**——真实黑洞事件视界是死黑的，但用户要「透明」效果，
故用镜像扭曲背景填充视界。若要恢复死黑视界，把 `captured` 分支改回 `col=0; a=vis;` 即可。

### capture_demo 卡死修复（用户反馈「运行后它就不动了」）

用户跑 `capture_demo` 发现画面**完全冻结**。诊断：渲染循环只跑 2 帧（frame#0 0.1ms、
frame#1 1.5ms）后 paint_frame **无限阻塞**——`has_capture` 第 2 帧 true（捕获纹理建好了）。

**根因：CopyResource 死锁。** 之前的 `rebuild_srv` 每帧在 immediate context 上
`CopyResource(staging, src)` 一个 WGC 拥有的 2MP 纹理，同时 Draw 采样它，与 WGC 的
FrameArrived 回调线程（free-threaded pool）争用 GPU 资源 → 卡死。

**修正**：去掉 CopyResource + owned_staging，回退到**直接 SRV 采样 WGC 纹理**：
WGC CreateFreeThreaded FramePool 用 2 个缓冲区轮换，回调里取出的 `ID3D11Texture2D` 我们持
COM 引用，WGC 不会在我们用时释放（会用另一缓冲区）。SRV 直接建在 latest 纹理上，仅当纹理
**对象指针变化**（轮到另一个缓冲区）时重建 SRV，否则复用。帧切换是原子的（整帧替换指针），
采样到的是旧帧或新帧，不撕裂。

**验证**：修复后 `~144 iters/s, has_frame=true, has_capture=true`——循环流畅、捕获实时、
不再卡死。这才是「真实桌面被扭曲」该有的状态：你动鼠标/移动窗口，捕获纹理实时更新，
黑洞透镜区随之扭曲新内容。

### 视界改回黑色 + 黑圈修复 + shader 优化（用户反馈「中间改黑、外有黑圈、很卡」）

**视界改回黑色**：之前的透明化（采样镜像扭曲桌面填充视界）用户不要了，改回 `col=0`（死黑视界），
alpha 走 lensEdge 与周围透镜区连续。

**黑圈修复**：透镜区背景采样（`bg`）的权重从 `* window * vis`（随距洞心衰减）改成**全权**
（真实捕获模式不衰减）——旧衰减让透镜区边缘的扭曲桌面变暗，视觉上像一圈黑边。真实捕获内容
天然有细节，不需要防闪烁衰减；程序化背景仍保留 `window*vis` 衰减。光子环区（强弯曲，d.z≥-0.02）
的 fallback 也改成全权采样。

**shader 优化**：`D3DCompile` 的 flags1 从默认 `0`（=LEVEL1，调试友好优化少）改成
`D3DCOMPILE_OPTIMIZATION_LEVEL3`。32 步测地线 + 捕获纹理采样的全屏 PS 在 LEVEL1 下明显卡顿，
LEVEL3 显著提速。用户反馈的「很卡」主要源于此。

### 透镜区压缩（用户反馈「透镜区域半径太大，压缩小一点」）

透镜区原来延伸到 ~7rh（屏幕半径），用户觉得太散。整体收紧，让扭曲紧贴黑洞：
- `DISK_OUTER` 9.0 → **6.0** r_s：盘外缘收窄，屏幕半径 3.46rh → 2.3rh。
- `bmax` `rout+3` → **`rout+1.5`**：测地线近场区边界收紧，~4.6rh → ~2.9rh。
- `lensEdge` `smoothstep(7rh,6rh)` → **`smoothstep(4.5rh,3.5rh)`**：alpha 边界收紧到 3.5-4.5rh。
- `window` `exp(-(plen/7rh)²)` → **`exp(-(plen/4.5rh)²)`**：偏折衰减收紧。

效果：透镜效果紧贴黑洞（3-4 倍影子半径内），不再一大圈散开；扭曲更集中、更像紧凑的引力透镜。

### 消除黑环 B（用户反馈「外→内：透镜环A、黑背景B、吸积盘C、高扭曲环D、黑圆E；不要 B」）

用户精确描述了渲染分层：A（外层轻微扭曲）→ B（**黑背景，bug**）→ C（吸积盘）→ D（高扭曲光子环）
→ E（黑视界）。B 不该存在——它在吸积盘外的近场区，本应显示扭曲的桌面。

**根因**：近场测地线区里 b 在 rout..bmax 之间的射线（本应弱弯曲逃逸、显示扭曲背景）被测地线
积分**误判为 captured**（步长/步数误差把弱弯曲射线过度弯曲），于是画成黑（视界）→ 形成黑环 B。

**修正**：逃逸射线的背景采样末尾加兜底——若 `bg` 仍接近黑（`luminance < 0.01`，即上述路径都没
采到有效背景），用一个弱场式扭曲采样强制填充：`defl2 = (2/W²)/plen * lensDepth*0.5 * window`，
采捕获纹理。保证**透镜区绝不留黑环**。

理想分层（修复后）：A（外轻微扭曲）→ D（高扭曲光子环，紧贴盘）→ C（吸积盘，从 D 中渐显）
→ E（黑视界），无 B。D 始终在黑洞周围（光子环），C 从 D 渐显（diskPresence），C 长成后 D 仍
在盘边缘环绕。

---

## 七、提交记录

```
d917228 feat(renderer): Gargantua-style accretion disk (doppler beaming, spiral arms)
5431fa2 fix(renderer): black hole invisible — missing input layout was the root cause
9b00643 fix(renderer): black hole now visible (RTV bind + classic LAYERED window)
```

> 注：`9b00643` 的「black hole now visible」是早期（RTV 绑定）尝试，实际当时仍不可见；
> 真正解决可见性的是 `5431fa2`（input layout）。
