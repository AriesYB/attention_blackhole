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

## 七、提交记录

```
d917228 feat(renderer): Gargantua-style accretion disk (doppler beaming, spiral arms)
5431fa2 fix(renderer): black hole invisible — missing input layout was the root cause
9b00643 fix(renderer): black hole now visible (RTV bind + classic LAYERED window)
```

> 注：`9b00643` 的「black hole now visible」是早期（RTV 绑定）尝试，实际当时仍不可见；
> 真正解决可见性的是 `5431fa2`（input layout）。
