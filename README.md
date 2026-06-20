# Attention Black Hole

把“该休息了”变成一个真的会靠近你的黑洞。

Attention Black Hole 是一个 Windows 桌面实验：它根据键鼠活动、窗口切换、空闲时间和目标窗口状态估算注意力负荷，然后在工作窗口上方渲染一个逐渐变大的黑洞。负荷越高，事件视界越大，屏幕内容被引力透镜扭曲得越明显；达到上限时进入强制休息状态，让你停下来缓一口气。

当前仓库主要包含 Rust 核心模型、Windows 平台信号层、D3D11/HLSL 黑洞渲染器和若干 demo。托盘、设置面板、安装包等完整产品外壳还在后续规划中。

## 特性

- 注意力负荷模型：用键鼠计数、切窗次数、退格率、击键节奏、目标窗口命中和空闲时间推算 `load`。
- 三段状态机：`Working`、`Dimming`、`ForcedBreak`。
- 黑洞渲染：D3D11 + HLSL，包含事件视界、光子环、吸积盘和引力透镜扭曲。
- 真实桌面捕获：通过 Windows Graphics Capture 把桌面像素作为 shader 采样源，让黑洞真的“吞噬”窗口内容。
- 程序化降级：没有捕获权限或没有捕获目标时，仍可使用程序化星云背景演示黑洞。
- 隐私边界：键鼠 hook 只做事件计数，不记录按键内容；捕获像素只在显存内用于渲染，不落盘、不回传。

## 运行环境

- Rust stable MSVC toolchain，本仓库已通过 `rust-toolchain.toml` 固定为 `stable-x86_64-pc-windows-msvc`。
- Windows 10 2004+，推荐 Windows 11。
- Visual Studio Build Tools 或 Visual Studio，需包含 MSVC linker 和 Windows SDK。
- D3D11 GPU、DWM 桌面合成和真实桌面会话。

纯模型和控制器测试是平台无关的；`platform/` 与 `renderer/` 只在 Windows 上编译。

## 快速开始

```powershell
cargo test
```

运行纯命令行注意力模型 demo：

```powershell
cargo run --example cli_demo
```

运行程序化黑洞 overlay：

```powershell
cargo run --example overlay_demo
```

运行测地线黑洞 demo：

```powershell
cargo run --example geodesic_demo
```

运行真实桌面捕获 demo：

```powershell
cargo run --example capture_demo
```

`capture_demo` 会捕获主显示器画面并在显存内做透镜扭曲。请只在你确认当前屏幕内容适合被本地渲染进 demo 时运行。

验证 Windows 输入和窗口信号：

```powershell
cargo run --example platform_demo "Visual Studio Code"
```

诊断 overlay 窗口状态：

```powershell
cargo run --example overlay_diag
```

## 工作方式

模型以 10Hz tick 推进。每个 tick 会读取当前输入增量、目标窗口状态和空闲时长，再更新注意力负荷：

- 在目标窗口中持续输入时，`load` 增长。
- 分心信号变强时，例如频繁切窗、退格率升高、击键节奏抖动，增长会加速。
- 真正空闲超过 30 秒后，`load` 按更快速度下降。
- 离开目标窗口但仍活跃时，`load` 缓慢下降。
- 在目标窗口里短暂停顿阅读时，`load` 基本保持不变。

默认阈值：

| 参数 | 默认值 | 含义 |
| --- | ---: | --- |
| `growth_per_min` | `2.0` | 正常专注时的基础增长速度 |
| `idle_shrink_per_min` | `6.0` | 空闲时的下降速度 |
| `offtarget_shrink_per_min` | `4.0` | 离开目标窗口时的下降速度 |
| `idle_grace` | `30s` | 超过该时间才视为空闲 |
| `dim_threshold` | `80` | 进入压暗状态 |
| `forced_threshold` | `100` | 进入强制休息状态 |
| `unlock_threshold` | `70` | 强制休息后低于该值才自动解锁 |

满负荷进入 `ForcedBreak` 后，如果用户真的停止输入，`load` 会按 `6%/min` 下降，约 5 分钟后低于 `70` 并自动解锁。也可以通过控制器的 `reset()` 立即清零。

## 项目结构

```text
src/
  controller.rs        # 10Hz 编排：SignalProvider -> AttentionModel -> Renderer
  mock.rs              # 测试和 demo 用 mock provider/renderer
  model/               # 纯 Rust 注意力模型、状态机、信号缓冲
  platform/            # Windows 输入 hook、前台窗口跟踪、空闲检测
  renderer/            # Windows D3D11 overlay、WGC 捕获、shader pipeline
shaders/
  blackhole.hlsl       # 黑洞引力透镜 shader
examples/
  cli_demo.rs          # 纯模型时间线 demo
  platform_demo.rs     # Windows 信号层 demo
  overlay_demo.rs      # 程序化背景黑洞 overlay
  geodesic_demo.rs     # 测地线黑洞视觉 demo
  capture_demo.rs      # 真实桌面捕获透镜 demo
  overlay_diag.rs      # overlay 状态诊断
tests/
  integration.rs       # 控制器生命周期集成测试
docs/
  superpowers/         # 设计文档和阶段计划
```

## 架构边界

`model/` 和 `controller/` 是平台无关核心，不依赖 Windows API，也不做 I/O。它们通过 trait 消费外部能力：

- `SignalProvider` 提供每个 tick 的输入和窗口信号。
- `Renderer` 接收由控制器生成的 `Frame`。

Windows 相关能力被隔离在两个模块：

- `platform/` 负责全局键鼠计数、前台窗口匹配和空闲判断。
- `renderer/` 负责透明置顶 overlay、D3D11 设备、WGC 捕获和 HLSL 渲染。

这个分层让模型可以独立测试，也给未来移植到 macOS/Linux 留了替换边界。

## 隐私说明

这个项目故意把隐私规则写得很硬：

- 键鼠 hook 只统计事件数量和时间间隔，不保存按键内容。
- 退格只作为一个计数信号，用来估算疲劳，不记录上下文。
- 真实屏幕捕获只在显存里作为 shader 纹理使用。
- 捕获内容不拷回 RAM、不写文件、不上传网络。
- 当没有捕获目标或不同意捕获时，渲染器会回退到程序化背景。

## 开发状态

已完成：

- 注意力模型和状态机。
- 控制器 trait 编排。
- Windows 输入/窗口信号 provider。
- D3D11 overlay 和黑洞 shader 管线。
- 程序化背景、真实桌面捕获和视觉 demo。
- 单元测试与集成测试。

待完善：

- 托盘和设置 UI。
- 配置文件读写。
- 首次启动捕获同意流程。
- 完整强制休息交互。
- 安装包、自动启动和产品级错误提示。

## 许可证

当前仓库尚未声明许可证。发布或分发前建议补充 `LICENSE`。
