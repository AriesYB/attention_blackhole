# 注意力黑洞（Attention Black Hole）— 设计文档

- **日期**：2026-06-17
- **状态**：已通过设计评审，待用户复核 → 进入实现计划
- **作者**：brainstorming 会话产出
- **灵感来源**：[s13k/blackhole](https://s13k.dev/blackhole/)（终端内随 AI 上下文增长的黑洞）

---

## 1. 愿景

一个常驻 Windows 的桌面工具：通过监听用户的键鼠活动与窗口切换，估算「持续专注带来的累积负荷」，并在用户正在使用的目标程序窗口上渲染一个**真实引力透镜效果的黑洞**。负荷越高，黑洞越大，逐渐扭曲并「吞噬」屏幕上的代码；满负荷时强制一个休息模态，逼迫用户放空大脑。休息（离开 / 空闲 / 手动重置）让黑洞缩小。

核心价值：把抽象的「该休息了」变成一个**有压迫感、可见、会逼近你的视觉实体**，比百分比数字或弹窗更触发行动。

---

## 2. 目标与非目标

### v1 目标（MVP）
1. Windows 平台（Win10 2004+，推荐 Win11）。
2. 单一可配置目标 App（默认 VS Code，按窗口标题匹配）。
3. 真实捕获 + HLSL raymarching 黑洞，对**目标窗口真实像素**做引力透镜扭曲（即「吞噬你的代码」）。
4. 注意力模型驱动黑洞涨缩：累积 + 专注减速 + 疲劳加速 + 空闲/离开缩小 + 手动重置。
5. 阶梯式施压：<80 纯视觉、80–99 压暗+横幅、100 强制休息模态（可配置关闭）。
6. 满负荷重置：手动「我休息好了」按钮 或 连续空闲 5 分钟自动解锁。
7. 托盘 + TOML 配置 + 首次启动明示同意。
8. 捕获不可用 / 拒绝同意时，回落到程序化背景黑洞，App 仍可用。

### v1 非目标（架构预留，不在 MVP 实现）
- 多 App 工作集（多目标窗口同时累积）。
- 历史统计 / 使用报告 / 游戏化。
- macOS / Linux 支持（架构分层预留，renderer 与 platform 均需重写）。
- 黑洞皮肤 / 自定义视觉主题。
- 模型权重的个性化自调（从个人数据学习）。
- 对捕获内容做 OCR 或任何内容理解（绝不）。

---

## 3. 关键设计决策

| # | 决策点 | 选择 | 理由 |
|---|--------|------|------|
| 1 | 目标平台 | Windows 优先，分层架构留跨平台扩展 | 先跑通再扩展，hook 层是唯一平台边界 |
| 2 | 驱动模型 | 累积为主 + 稳定专注减速 + 疲劳信号加速；空闲/离开缩小；手动重置 | 既有清晰主线，又用上「监听行为判断注意力」的构想，且好调参 |
| 3 | 目标窗口 | 单一目标 App 的 overlay；切走即「离开」触发缩小 | 行为最清晰、最契合「逼迫休息」意图 |
| 4 | 渲染精度 | 满血 GPU raymarching 黑洞（真实引力透镜、吸积盘、光子环） | 视觉压迫感是产品灵魂 |
| 5 | 技术栈 | Tauri 外壳 + 原生 D3D11 renderer（P2） | 真捕获透镜必须显存内直采，WebGL 跨 IPC 不可行；Tauri 继续干外壳 |
| 6 | 施压强度 | 阶梯式（<80 视觉 / 80–99 压暗 / 100 模态强制）+ 可关 | 既实现「逼迫」又不被卸载 |
| 7 | 满负荷重置 | 手动按钮 或 连续空闲 5 分钟自动解锁 | 信任与强制并存 |

---

## 4. 架构

### 4.1 模块拆分

```
Rust 单一二进制
├─ shell/        (Tauri 外壳)
│   • 设置窗口(webview): 阈值/速率滑杆、目标串、shader 画质
│   • 托盘: Pause/Reset/Settings/Quit（+可选 load% 徽标）
│   • AutoStart / 单实例 / Config(TOML) 读写
│   • 转发命令 reset/leave/close/toggle_force → controller
│
├─ controller/   (编排胶水，平台无关)
│   • 10Hz tick（独立线程，不碰 UI）
│   • SignalProvider → AttentionModel → StateMachine 串联
│   • 把 load/state/rect 推给 renderer + shell
│
├─ model/        ★纯 Rust 无 I/O，单测主力
│   • SignalBuffer(60s 滚动统计)
│   • AttentionModel(涨/缩公式 + 系数)
│   • StateMachine(Working/Dimming/ForcedBreak)
│
├─ platform/     ★Windows 耦合、可替换
│   • InputHook    SetWindowsHookEx WH_KEYBOARD_LL/WH_MOUSE_LL（只计数）
│   • WindowTracker GetForegroundWindow + GetWindowRect + 标题匹配
│   • IdleDetector  由最后输入时间戳派生
│
└─ renderer/     ★Windows 耦合(D3D11)、可替换
    • OverlayWindow  WS_EX_LAYERED|WS_EX_TRANSPARENT 逐像素 alpha，置顶
    • CaptureSource  WGC 按 targetHwnd 建 GraphicsCaptureItem + FramePool
    • ShaderPipeline HLSL raymarching，直接采样捕获纹理
    • Compositor     alpha 合成黑洞 → DWM
```

### 4.2 分层纪律（跨平台扩展点）
- `model/` 与 `controller/`：**纯平台无关** Rust，无 I/O，可独立单测。
- `platform/`：**唯一的输入/窗口耦合点**。未来 macOS 替换为 CGEventTap + AXUI。
- `renderer/`：**渲染耦合点**。未来 macOS 替换为 Metal + ScreenCaptureKit。
- 未来加 macOS = 同时换 `platform/` + `renderer/`，`model/`/`controller/` 一行不改。
- 抽象 trait：`SignalProvider`（platform 实现）、`Renderer`（renderer 实现），controller 依赖 trait 而非具体类型。

### 4.3 单一二进制内的双窗口
同一 Rust 进程内：Tauri 提供设置窗口（webview）+ 托盘；一个独立的原生 D3D11 overlay 窗口做渲染器。两者由同一个 controller 驱动，共享 `model/` + `platform/`。

---

## 5. 数据流（每个 tick）

后端 **10Hz tick**（Δt = 100ms，独立线程）：

1. `platform/` 上报本 tick 信号：
   - 键盘事件计数、鼠标事件计数（仅计数，无内容）。
   - 前台窗口：句柄、rect（物理像素）、是否匹配目标 App（标题包含目标串）。
   - 空闲时长（距离最后一次输入的时间）。
2. `model/` 消费信号：
   - 推入 `SignalBuffer`（60s 滚动窗口）更新统计量。
   - `AttentionModel` 计算 ΔL，夹到 [0,100]。
   - `StateMachine` 按阈值推进状态。
3. `controller/` emit 事件：把 `{load, state, on_target, rect}` 推给 renderer（经 channel 跨线程传递，renderer 线程消费）与 shell（Tauri 事件，刷新托盘徽标/设置面板）。
4. `renderer/`：overlay 对齐到 `rect`（带插值平滑），shader 按 `load` 缩放黑洞、按 `state` 切换压暗/横幅/模态。

**手动命令**（shell → controller）：
- `reset()`：用户点「我休息好了」→ L=0，退出 ForcedBreak。
- `leave()`：等价于切走目标窗口（触发离开缩小）。
- `close()`：隐藏 overlay（暂停）。
- `toggle_force()`：切换 `force_enabled`。

---

## 6. 注意力模型

### 6.1 符号与默认值（全部可在 config.toml 调）
- 负荷 `L ∈ [0,100]`；tick `Δt = 0.1s`。
- 阈值：`DIM = 80`，`FORCED = 100`，`UNLOCK = 70`。
- `IDLE_GRACE = 30s`（超过即视为空闲）。

### 6.2 增长（在目标窗口前台 且 有输入 且 非空闲）
```
ΔL = GROWTH × focus_factor × fatigue_factor × (Δt / 60)
GROWTH         = 2.0 %/min   → 正常专注约 50 分钟填满
focus_factor   ∈ [0.4, 1.0]  输入越平稳稳定 → 越低（专注减速）
fatigue_factor ∈ [1.0, 2.0]  切窗频率↑/击键节奏方差↑/退格率↑ → 越高（疲劳加速）
```

疲劳三信号（在 60s 滚动窗口内计算）：
- `switch_rate`：窗口切换次数 / 分钟。
- `typing_jitter`：击键间隔的标准差。
- `error_rate`：退格键占比。

`focus_factor` 与 `fatigue_factor` 的具体映射函数在实现时确定，默认中点取值使「正常专注」≈ 50 分钟填满。

### 6.3 缩小
- **空闲**（无输入 > `IDLE_GRACE`，不论在不在目标窗口）：
  `ΔL -= IDLE_SHRINK × (Δt/60)`，`IDLE_SHRINK = 6.0 %/min`。
  → 满负荷(100) 经连续空闲约 5 分钟降到 UNLOCK(70) 以下，对齐「5 分钟自动解锁」：`(100−70)/6 = 5 min`。
- **离开目标**（前台切到别的窗口但仍活跃）：
  `ΔL -= OFFTARGET_SHRINK × (Δt/60)`，`OFFTARGET_SHRINK = 4.0 %/min`，且停止增长。
  → 去查文档/看视频不会快速清零，但持续离开会缓慢消解。
- **手动**：`reset` → L=0 立即生效。

### 6.4 状态机
```
            L ≥ 80                  L ≥ 100 (且 force_enabled)
 Working  ─────────►  Dimming  ─────────────────────►  ForcedBreak
   ▲                   ▲                                    │
   │ L < 80            │ L < 80                             │ (L < UNLOCK=70) 经空闲衰减
   │                   │                                    │      或 手动 reset (→L=0)
   └───────────────────┴────────────────────────────────────┘
```
- `Working`(<80)：纯视觉，黑洞渐长。
- `Dimming`(80 ≤ L < 100)：背景压暗 + 顶部「该放空了」横幅。
- `ForcedBreak`(L ≥ 100，`force_enabled=true`)：overlay 切为模态（input-capturing，点击不穿透），必须手动 reset 或连续空闲 5 分钟自动解锁。
- `force_enabled = false`：视觉封顶在 99% 等效，**永不模态**。

### 6.5 进入 ForcedBreak 后的解锁
- 路径 A（手动）：用户点「我休息好了」→ L=0 → 退回 Working。
- 路径 B（自动）：进入 ForcedBreak 后，持续空闲使 L 按 `IDLE_SHRINK=6 %/min` 衰减；当 L < `UNLOCK=70` 时退回 Dimming/Working。按数学保证，连续空闲约 5 分钟必解锁。

**空闲判定规则（消除歧义）**：空闲基于**全局输入**——任何键盘/鼠标活动都重置空闲计时。ForcedBreak 模态中：
- 点「我休息好了」按钮 = 走路径 A（手动立即重置）。
- 其余模态交互（如乱点试图逃出）**不**触发手动重置，也**不**重置空闲计时——只有真正不动才能走路径 B 自动解锁。
- 即「逼真休息」：要么主动承认休息（按钮），要么真的停手 5 分钟。

---

## 7. 施压模型（黑洞的可视行为）

| 状态 | overlay 行为 | 输入 |
|------|--------------|------|
| Working (<80) | 黑洞随 `load` 渐大（视界半径 ~2%→~20% 短边），盘渐亮 | 点击穿透 |
| Dimming (80–99) | 黑洞继续长（视界→~35%），背景整体压暗，顶部横幅 | 点击穿透 |
| ForcedBreak (100) | 黑洞最大，背景深暗，模态层 + 「我休息好了」按钮 | **捕获输入**（强制） |

`force_enabled=false` 时永不进入 ForcedBreak 列，封顶 Dimming 视觉。

---

## 8. 渲染器（D3D11 + HLSL，P2）

### 8.1 黑洞渲染（如何「吞噬代码」）
对 overlay 每个像素，从相机出发 march 一条光子，每步按 ∝ 1/r² 朝奇点弯曲速度（简化测地线积分，ShaderToy 黑洞通用做法，视觉逼近 GR）：
- 光线进入**事件视界** → 纯黑。
- 掠过视界附近 → **光子环**（亮圈）。
- 用弯曲后的坐标采样**捕获的目标窗口纹理** → 实现「扭曲并吞噬你的代码」。
- 光线穿过赤道面 → 采样**吸积盘**（发光、多普勒偏移、随 `u_time` 旋转）。

### 8.2 驱动 uniform（每 tick 由 controller 更新）
- `u_load`(0..1)：事件视界半径、盘亮度、暗角强度。
- `u_dim`(0..1，来自 Dimming/ForcedBreak)：整体背景压暗系数。
- `u_time`：盘旋转动画。
- `u_capturedTexture`：WGC 捕获帧（sRGB）。
- `u_targetRect` / overlay↔capture UV 映射。

涨大映射：`u_load`↑ → 视界半径从 ~2% 长到 ~35% 短边、盘变亮、背景压暗 → 越来越压迫。

### 8.3 捕获管线（零 CPU 拷贝）
- WGC `GraphicsCaptureItem.CreateFromWindow(targetHwnd)`（**需 Win10 2004+**，推荐 Win11 无捕获黄边）。
- `Direct3D11CaptureFramePool` 与 renderer 共享同一 D3D11 设备 → 捕获的 `ID3D11Texture2D` **直接喂 shader，不拷回 RAM**。
- 背景纹理按 ~30fps 捕获（透镜「后面」的内容不需要 60fps）；透镜 shader 跑 60fps。
- overlay 只画目标窗口大小（不铺全屏）→ 填充率有界。
- 渲染在独立线程，与 controller tick 分离，互不阻塞。

### 8.4 降级（程序化背景兜底）
当 WGC 不可用或用户拒绝捕获同意时，`u_capturedTexture` 替换为 shader 程序化生成的星云/噪点背景。`CaptureSource` 抽象为 trait：`Win32CaptureSource`（真实）与 `ProceduralCaptureSource`（兜底），renderer 不感知差异。

---

## 9. 隐私

捕获真实像素使隐私含义显著高于「键鼠计数」，硬规则：
1. 捕获帧**只在显存**，绝不拷回 RAM、绝不落盘、绝不外传。
2. **仅当 `on_target` 时捕获**；切走立刻停止捕获并清空纹理。
3. **首次启动明示同意**对话框，文案明确：
   > 「本程序在显存内捕获目标窗口像素以渲染透镜效果；不存储、不传输任何屏幕内容。键鼠监听仅用于统计事件次数，不记录按键内容。」
   需用户主动确认；附简短隐私说明链接。
4. **降级开关**：拒绝同意 → 回落程序化背景（§8.4），App 仍可用。
5. 企业用户提示：屏幕捕获可能被 EDR 标记；后续对二进制做代码签名。
6. 键鼠 hook 回调只做计数器自增，**不保留任何按键数据**。

---

## 10. 错误处理与边界情况

| 场景 | 行为 |
|------|------|
| 目标窗口关闭/找不到 | overlay 隐藏，model 暂停（不涨不缩），托盘提示「目标未找到」 |
| 目标窗口最小化 | 暂停捕获，隐藏 overlay |
| 多显示器 / 窗口移动 | 每 tick 用 `GetWindowRect` 对齐 overlay（带插值平滑） |
| DPI 缩放 | 用物理像素，per-monitor DPI aware |
| 锁屏 / RDP 断开 | 全暂停，解锁/重连后恢复 |
| WGC 不可用（OS 过旧） | 检测后回落程序化背景（§8.4）并提示 |
| 独占全屏目标返回黑帧 | 检测黑帧，提示「该应用不支持捕获」，回落或暂停 |
| `force_enabled=false` | 视觉封顶 99% 等效，永不模态 |
| 多实例启动 | 单实例守护，第二个实例唤醒已运行实例的设置窗口 |

---

## 11. 测试策略

- **`model/`（主力，纯 Rust，TDD）**：覆盖涨缩数学、状态机迁移、所有信号组合、空闲/离开/边界条件。正确性主要落在这里。
- **`controller/`**：用 mock `SignalProvider` / `Renderer` 做集成测试，验证编排与生命周期。
- **`platform/` + `renderer/`**：藏在 trait 后；真实 OS/DX 路径用「预览模式」（load 0→100 扫描）+ 手工 QA 清单验证。
- **shader**：预览模式在多个 load 档位做视觉 QA；后续可选参考图比对。

---

## 12. 配置（config.toml）

默认放在 `%APPDATA%\attention-blackhole\config.toml`（首次启动生成）。可配置项：
- `target_window_title_contains`（默认 `"Visual Studio Code"`）
- 增长/缩小参数：`growth`、`idle_shrink`、`offtarget_shrink`、`idle_grace`
- 阈值：`dim_threshold`、`forced_threshold`、`unlock_threshold`
- `force_enabled`（默认 `true`）、`idle_auto_release_minutes`（默认 `5`）
- shader 画质档（低/中/高，影响 raymarch 步数与捕获分辨率）
- `capture_consent`（首次同意后写入，记录同意状态；含版本号便于文案变更后重新征求）

---

## 13. v1 实现顺序（建议，供实现计划参考）

1. 纯 Rust 骨架：`model/`（含完整单测）→ `controller/`（mock platform/renderer 跑通 tick）。
2. `platform/` Windows 实现：InputHook + WindowTracker + IdleDetector。
3. `renderer/` 最小可用：透明 overlay 窗口 + **程序化背景**黑洞 shader（先不接捕获），验证涨缩视觉。
4. 接入 WGC 真实捕获，替换为透镜真实像素。
5. Tauri 外壳：托盘 + 设置窗口 + 配置 + 首次同意。
6. 阶梯施压（Dimming 横幅、ForcedBreak 模态、解锁逻辑）。
7. 边界处理、降级、QA 清单。

---

## 14. 待定 / 未来

- `focus_factor` / `fatigue_factor` 的具体映射函数在实现时确定并经实测调参。
- Win11 专属捕获能力（无边框等）的利用。
- macOS/Linux 移植（需 Metal + ScreenCaptureKit / X11 + PipeWire）。
- 模型权重个性化自调。
