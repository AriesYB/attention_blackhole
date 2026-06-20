// =============================================================================
// 注意力黑洞 —— 物理 geodesic-traced 黑洞 HLSL shader
// =============================================================================
//
// **移植自 s13k 的 `blackhole.glsl`（ghostty-blackhole，MIT License,
// Copyright (c) 2026 s13k <s13k@pm.me>，https://github.com/.../blackhole.glsl）**，
// 其物理方法又源自 Eric Bruneton 的 "Real-time High-Quality Rendering of
// Non-Rotating Black Holes"（https://ebruneton.github.io/black_hole_shader/）。
// 本文件按 MIT 许可证随 attention_blackhole 项目发布（见 LICENSE），上游版权声明保留。
//
// 每个近场像素逐帧积分自己的零测地线 x'' = -(3/2) h² x / r⁵（精确的
// Schwarzschild 光子弯曲，Binet 形式）。下列现象全部是积分的**涌现结果**，
// 而非手画：
//   - 事件视界阴影：impact parameter < b_crit = (3√3/2) r_s 的射线旋进视界 → 黑。
//   - 引力透镜：逃逸射线投影回 capture 平面，文字/代码弯曲、放大、在爱因斯坦环
//     内出现镜像次像（「吞噬你的代码」——spec §8.1）。
//   - 光子环：绕 r = 1.5 r_s 光子球多圈的射线聚焦出的极亮细环。
//   - 吸积盘：开普勒薄盘，射线可多次穿越盘面（远侧弧越过/越过视界上下方，
//     即星际穿越/卡冈图雅式）；Shakura–Sunyaev 温度 → 黑体色，相对论多普勒+
//     引力频移 g = √(1−1.5 r_s/r)/(1−β·k̂)，beaming 强度 ∝ g^N。
//
// 单位：r_s（Schwarzschild 半径）= 1。屏幕映射把影子半径 b_crit 绑到
// HOLE_RADIUS*sz 屏幕高度，使 u_load 驱动的涨缩保持工作。
//
// ---- 本项目 overlay 适配（与原 ghostty 终端版的差异）----
// 1. 输出预乘 alpha（painter.rs + overlay DwmExtendFrameIntoClientArea 逐像素
//    alpha 合成）。原 shader 输出不透明（画在终端文字上）；overlay 是悬浮透明窗，
//    故补一个 alpha 包络：远场 → 0（桌面透出），视界附近 → 1（遮挡注意力）。
// 2. u_load（注意力负荷 0..1）→ 主强度 I + 尺寸 sz，替代原 shader 的
//    pomodoro/token 调度（本项目用注意力模型驱动，不移植 Lissajous 漂移）。
// 3. 背景：u_has_capture=1 采样捕获的目标窗口纹理（lensed sky plane = spec §8.1
//    「吞噬代码」），=0 走程序化星云（spec §8.4 降级兜底）。
// 4. u_dim（Dimming/ForcedBreak 压暗）乘进 col。
//
// cbuffer FrameConstants 每帧由 painter 更新（32 字节契约，与 shader.rs 镜像）。

cbuffer FrameConstants : register(b0)
{
    float u_load;        // 注意力负荷，归一化 0..1（Frame.load / 100）
    float u_dim;         // 压暗强度 0..1（Dimming/ForcedBreak 时增大）
    float u_time;        // 渲染时间秒（驱动吸积盘旋转/时间膨胀）
    float _pad0;         // cbuffer 16 字节对齐填充
    float2 u_resolution; // backbuffer 像素尺寸
    uint  u_has_capture; // 1 = 用捕获纹理背景，0 = 程序化背景
    float _pad1;         // 对齐
    float2 u_ornament_center; // 摆件模式由宿主传入的中心点（UV）
    float u_size_scale;       // 摆件/外部模式尺寸倍率；<=0 时按 1.0
    uint  u_mode_flags;       // bit0 = 使用外部中心点
    float4 u_visual_params;   // x=质量/弯曲 y=爱因斯坦环范围 z=吸积盘强度 w=透镜强度
    float4 u_disk_tint;       // rgb=吸积盘颜色乘子
    float4 u_disk_params;     // x=倾角 y=范围 z=活跃度 w=相对论强度（<=0 用默认）
};

Texture2D    u_capturedTexture : register(t0);
SamplerState u_sampler         : register(s0);

// Host-controlled render modes.
#define MODE_ORNAMENT_CENTER 0x1u

// -----------------------------------------------------------------------------
// 旋钮（顶部 #define，便于按目标 GPU 调）。
// -----------------------------------------------------------------------------

// 每像素测地线积分步数。从 32 降到 20：大黑洞时近场覆盖大半屏，32 步全屏 PS 会卡；
// 20 步在物理细节（光子环/盘像）与帧率间平衡。低端机可再降到 16。
#define N_STEPS 64

// Schwarzschild 黑洞的临界 impact parameter（r_s 单位）：b < b_crit 的射线落入视界，
// 远看即影子半径。物理常数（非可调），#define 以免被误改。
#define B_CRIT 2.5980762

// GLSL `mod(a,b)` 对负数的行为（结果与 b 同号）。HLSL `fmod` 是截断除法（与 a 同号）。
#define glmod(x, y) ((x) - (y) * floor((x) / (y)))

// 盘视觉参数 —— 对齐 ghostty-blackhole **tuner Defaults**（电影级卡冈图雅观感）。
// 这套值是 s13k 调到「干净、明亮、白热内圈、强多普勒半亮半暗」的打磨默认值，与
// blackhole.glsl 文件内的 Inferno 预设常量不同（那个是厚重熔岩观感）。值取自
// tuner/Sources/BlackHoleTuner/ParamSpec.swift 的 def 字段。
#define DISK_INNER   1.00   // 内边缘 r_s：3 = ISCO（最内稳定圆轨道），干净内缘的关键
#define DISK_OUTER   8.00   // 外边缘 r_s（ghostty 默认值）
#define DISK_INCL    1.45   // 倾角（弧度）：0 正视，π/2≈1.57 侧视（星际穿越感）
#define DISK_ROLL    0.15   // 整体在屏幕平面内的旋转
#define DISK_GAIN    1.00   // 盘发射亮度（Defaults 1.0，避免过曝）
#define DISK_OPACITY 0.65   // 近侧盘对后景的遮挡程度（Defaults 0.65，不过厚）
#define DISK_TEMP    8500.0 // 最热环带色温（K）：8500 = 白热偏蓝内圈，5500 偏暖
#define DOPPLER_MIX  1.00   // 0=无相对论明暗/色偏，1=满物理（Defaults 满效应）
#define REDSHIFT_TINT 0.55  // 额外可见化引力红移色偏；不改测地线，只让内圈更暖一点。
#define DISK_BEAM    3.00   // beaming 指数：观测强度 ∝ g^N（3≈光子计数，4≡热辐射）
#define DISK_SPEED   9.00   // 磨砂颗粒流动速度；本端口正值表示屏幕左侧朝向观察者。
#define DISK_WIND    6.50   // 磨砂云雾随半径的轻微流动量
#define DISK_CONTR   2.80   // 磨砂颗粒对比度；过高会重新显出采样噪点。
#define EXPOSURE     1.00   // 盘光的 tonemap 曝光（捕获纹理不受影响）
#define LENS_DEPTH   13.00  // 从洞到 capture「天」平面的距离（r_s）。ghostty 默认 13，越大内容弯得越狠。
#define LENS_STRENGTH 0.68  // overlay 适配：整体透镜位移强度。低一些会降低“放大镜倍率”。
#define LENS_FLAT_START 0.35 // overlay 适配：从可见半径的 35% 开始逐步压平边缘位移。
#define CAPTURE_SHARPEN 0.12 // 捕获纹理 bicubic 后轻微锐化，补偿透镜放大时的线性采样糊感。
#define DILATION_MIN 0.20   // 引力时间膨胀：洞满负荷时盘图案速率衰减到 0.2（ghostty 默认）。
#define STAR_GAIN    0.00   // 透镜星场亮度（0 = 关，Defaults 关）

// -----------------------------------------------------------------------------
// 全屏三角形 vertex shader。3 个顶点覆盖整个 NDC 空间（无需 vertex buffer）。
// 输出 UV（0..1）供 pixel shader 用。vertex_id 决定顶点位置。
struct VSOut
{
    float4 pos : SV_POSITION;
    float2 uv  : TEXCOORD0;
};

VSOut vs_main(uint vertex_id : SV_VertexID)
{
    VSOut o;
    // 经典全屏三角形：vertex_id ∈ {0,1,2} 映射到 (-1,-1),(3,-1),(-1,3)。
    // 超出 NDC 的部分被光栅器裁剪，覆盖全屏只需 3 顶点（比 quad 少一个）。
    o.pos = float4(float2((vertex_id << 1) & 2, vertex_id & 2) * 2.0 - 1.0, 0.0, 1.0);
    o.pos.y = -o.pos.y; // UV 原点左上，与屏幕一致
    o.uv = o.pos.xy * 0.5 + 0.5;
    return o;
}

// -----------------------------------------------------------------------------
// 噪声 / 工具

// 简单 hash 噪声（程序化星云 + 吸积盘磨砂颗粒用，避免引入噪声库）。
float hash21(float2 p)
{
    p = frac(p * float2(234.34, 435.345));
    p += dot(p, p + 34.23);
    return frac(p.x * p.y);
}

float vnoise(float2 p)
{
    float2 i = floor(p), f = frac(p);
    f = f * f * (3.0 - 2.0 * f);
    return lerp(lerp(hash21(i),                         hash21(i + float2(1.0, 0.0)), f.x),
                lerp(hash21(i + float2(0.0, 1.0)),       hash21(i + float2(1.0, 1.0)), f.x),
                f.y);
}

float matteNoise(float2 p)
{
    float n0 = vnoise(p);
    float n1 = vnoise(p * 1.75 + float2(13.1, 5.7));
    float n2 = vnoise(p * 2.65 + float2(-2.8, 17.2));
    return n0 * 0.56 + n1 * 0.30 + n2 * 0.14;
}

// 镜像重复：让透镜后的纹理采样不出界又不边缘涂抹（对应 GLSL 版 mirrorUV）。
float2 mirrorUV(float2 u) { return 1.0 - abs(1.0 - glmod(u, 2.0)); }

// 本 shader 的几何/物理坐标沿用最初端口：i.uv.y=0 在屏幕底部。
// D3D/WGC 纹理采样坐标则是 v=0 在纹理顶部。只在采样真实捕获纹理时翻转 V，
// 避免把黑洞相机、吸积盘倾角和漂移路径一起翻转。
float2 captureUV(float2 u) { return float2(u.x, 1.0 - u.y); }

float catmull(float x)
{
    x = abs(x);
    float x2 = x * x;
    float x3 = x2 * x;
    return x < 1.0
        ? 1.5 * x3 - 2.5 * x2 + 1.0
        : (x < 2.0 ? -0.5 * x3 + 2.5 * x2 - 4.0 * x + 2.0 : 0.0);
}

float2 rot(float2 v, float a)
{
    float c = cos(a), s = sin(a);
    return float2(c * v.x - s * v.y, s * v.x + c * v.y);
}

// 由开氏温度求黑体色（Tanner Helland 拟合，归一化到 0..1）。
float3 blackbody(float T)
{
    float t = clamp(T, 1500.0, 40000.0) / 100.0;
    float r = t <= 66.0 ? 1.0
                        : clamp(1.292936 * pow(max(t - 60.0, 1e-3), -0.1332047), 0.0, 1.0);
    float g = t <= 66.0 ? clamp(0.3900816 * log(max(t, 1e-3)) - 0.6318414, 0.0, 1.0)
                        : clamp(1.1298909 * pow(max(t - 60.0, 1e-3), -0.0755148), 0.0, 1.0);
    float b = t >= 66.0 ? 1.0
                        : (t <= 19.0 ? 0.0
                                     : clamp(0.5432068 * log(max(t - 10.0, 1e-3)) - 1.1962540, 0.0, 1.0));
    return float3(r, g, b);
}

// 稀疏程序化星场，按射线方向索引——因采样用的是**弯曲后**的射线，星点在洞周围
// 自然抹成弧。STAR_GAIN=0 时关闭。
float3 stars(float3 d)
{
    float2 sph = float2(atan2(d.x, -d.z), asin(clamp(d.y, -1.0, 1.0)));
    float2 g   = sph * 40.0;
    float2 id  = floor(g);
    float h    = hash21(id);
    if (h < 0.92) return float3(0.0, 0.0, 0.0);
    float2 f   = frac(g) - 0.5;
    float2 off = (float2(hash21(id + 17.3), hash21(id + 31.7)) - 0.5) * 0.7;
    float spark = smoothstep(0.10, 0.0, length(f - off));
    float tw    = 0.7 + 0.3 * sin(u_time * (0.5 + 2.0 * hash21(id + 5.1)) + 40.0 * h);
    float3 tint = lerp(float3(1.0, 0.82, 0.60), float3(0.75, 0.85, 1.0), hash21(id + 2.9));
    return tint * spark * tw * ((h - 0.92) / 0.08);
}

// 程序化星云背景：u_has_capture=0 时用（spec §8.4 降级兜底）。
// 关键：背景必须含**高频细节**（细密网格 + 稠密星点），否则透镜畸变在平滑渐变上
// 根本看不出来——这是「黑洞周围没有畸变」的主因。真实捕获（终端/代码）天然有细节，
// 但程序化兜底也必须自带可被弯曲的纹理。
float3 procedural_background(float2 uv, float time)
{
    // 深空底色：垂直渐变 + 微弱星云团。
    float3 col = lerp(float3(0.015, 0.018, 0.035), float3(0.04, 0.025, 0.07), uv.y);
    float2 q = uv * 3.0 + float2(time * 0.02, time * 0.01);
    float n = hash21(floor(q)) * 0.5 + hash21(floor(q * 2.0)) * 0.25;
    col += float3(0.10, 0.05, 0.15) * smoothstep(0.6, 1.0, n) * 0.3;

    // 细密网格（亮线）：透镜弯曲时网格线会明显变形/汇聚——畸变的视觉锚点。
    // 频率约每屏 48 格，线宽细，明度低不抢戏。
    float2 grid = abs(frac(uv * float2(48.0, 27.0)) - 0.5);
    float gridline = smoothstep(0.48, 0.50, max(grid.x, grid.y));
    col += float3(0.10, 0.14, 0.22) * gridline * 0.5;

    // 稠密星点（密度远高于纯星场 stars()，让透镜在程序化模式也明显）。
    float2 sid = floor(uv * float2(160.0, 90.0));
    float star = hash21(sid);
    float twinkle = 0.5 + 0.5 * sin(time * 2.0 + star * 6.28);
    if (star > 0.975)
    {
        col += float3(0.8, 0.85, 1.0) * twinkle * (star - 0.975) * 60.0;
    }
    return col;
}

float3 sample_capture_bicubic(float2 u)
{
    uint texW;
    uint texH;
    u_capturedTexture.GetDimensions(texW, texH);
    float2 texSize = max(float2((float)texW, (float)texH), float2(1.0, 1.0));
    float2 uv = saturate(captureUV(u));
    float2 pos = uv * texSize - 0.5;
    float2 base = floor(pos);
    float2 f = pos - base;

    float3 sum = float3(0.0, 0.0, 0.0);
    float wsum = 0.0;
    [unroll]
    for (int y = -1; y <= 2; y++)
    {
        float wy = catmull(float(y) - f.y);
        [unroll]
        for (int x = -1; x <= 2; x++)
        {
            float wx = catmull(float(x) - f.x);
            float w = wx * wy;
            float2 tc = (base + float2(x, y) + 0.5) / texSize;
            sum += u_capturedTexture.SampleLevel(u_sampler, saturate(tc), 0.0).rgb * w;
            wsum += w;
        }
    }
    float3 bicubic = sum / max(wsum, 1e-5);

    // 很轻的 unsharp mask：只补偿双线性/透镜放大带来的软化，避免文字边缘发虚。
    float2 px = 1.0 / texSize;
    float3 linear_sample = u_capturedTexture.SampleLevel(u_sampler, uv, 0.0).rgb;
    float3 blur = (
        u_capturedTexture.SampleLevel(u_sampler, saturate(uv + float2(px.x, 0.0)), 0.0).rgb +
        u_capturedTexture.SampleLevel(u_sampler, saturate(uv - float2(px.x, 0.0)), 0.0).rgb +
        u_capturedTexture.SampleLevel(u_sampler, saturate(uv + float2(0.0, px.y)), 0.0).rgb +
        u_capturedTexture.SampleLevel(u_sampler, saturate(uv - float2(0.0, px.y)), 0.0).rgb
    ) * 0.25;
    return saturate(bicubic + (linear_sample - blur) * CAPTURE_SHARPEN);
}

float3 sample_scene(float2 u, float time)
{
    if (u_has_capture == 1)
    {
        return sample_capture_bicubic(u);
    }
    return procedural_background(mirrorUV(u), time);
}

float sample_scene_channel(float2 u, float time, int ci)
{
    if (u_has_capture == 1)
    {
        float3 c = sample_capture_bicubic(u);
        return ci == 0 ? c.r : (ci == 1 ? c.g : c.b);
    }
    return procedural_background(mirrorUV(u), time)[ci];
}

// -----------------------------------------------------------------------------
// pixel shader 主入口：忠实移植 ghostty blackhole.glsl 的 mainImage，
// 唯一差异是本项目 overlay 需要的 alpha 通道（ghostty 输出恒 alpha=1）。
// 不臆造扭曲公式——透镜效果完全来自 ghostty 的 sky-plane 弱场偏折 + 测地线积分。
float4 ps_main(VSOut i) : SV_Target
{
    float2 res    = u_resolution;
    float2 uv     = i.uv;
    float aspect  = res.x / max(res.y, 1.0);

    // u_load → 主填充 g（0..1）→ 强度 I 与影子半径 rh。
    // rh 上限调小到 0.15（从 0.35）：用户要「黑洞最大时透镜占满屏，而非黑洞占满」。
    // 用 pow(g_load, 0.7) 让涨缩更缓（前期慢长、后期才明显），避免「变大太快」。
    float sizeScale = clamp(u_size_scale > 0.0 ? u_size_scale : 1.0, 0.35, 2.0);
    float massScale = clamp(u_visual_params.x > 0.0 ? u_visual_params.x : 1.0, 0.35, 2.0);
    float ringScale = clamp(u_visual_params.y > 0.0 ? u_visual_params.y : 1.0, 0.45, 2.2);
    float diskUser = saturate(u_visual_params.z);
    float lensScale = clamp(u_visual_params.w > 0.0 ? u_visual_params.w : 1.0, 0.25, 2.0);
    float3 diskTint = max(u_disk_tint.rgb, float3(0.0, 0.0, 0.0));
    float diskTilt = saturate(u_disk_params.x);
    float diskSpan = clamp(u_disk_params.y > 0.0 ? u_disk_params.y : 1.0, 0.60, 1.80);
    float diskActivity = clamp(u_disk_params.z > 0.0 ? u_disk_params.z : 1.0, 0.05, 1.40);
    float relativityMix = clamp(u_disk_params.w > 0.0 ? u_disk_params.w : DOPPLER_MIX, 0.0, 1.0);
    float g_load = clamp(u_load, 0.0, 1.0);
    float sizeGrowth = smoothstep(0.35, 1.30, sizeScale);
    float growth = ((u_mode_flags & MODE_ORNAMENT_CENTER) != 0) ? sizeGrowth : g_load;
    float massDisk = smoothstep(0.45, 1.75, massScale);
    float I = lerp(0.10, 1.0, growth);
    float rh = lerp(0.015, 0.15, pow(growth, 0.7)); // 屏幕高度单位，1.5%→15%，黑洞本身不大
    rh *= sizeScale;

    // 黑洞在屏幕上漂移（忠实 ghostty pomodoro 模式的双尺度 Lissajous，blackhole.glsl:336-349）。
    // 两层叠加：(1) 慢的大幅度漂移（0.21/0.083 频率，0.24/0.05 幅度）——洞「游走」；
    //          (2) 快的小幅度抖动（0.83/1.31 频率，0.040/0.030 幅度 ×I）——洞「颤动」。
    // 速度随 I：spd = mix(0.35,1.0,I)，小洞慢、大洞快。快抖动幅度 ×I，小洞几乎不动。
    // 用 incommensurate 频率让轨道永不重复。本项目 overlay 全屏无 work-area 限制，
    // 故中心绕 (0.5,0.5)，幅度按 ghostty 的比例缩到合适范围。
    float spd = lerp(0.35, 1.0, I);
    float2 center = float2(0.5, 0.5);
    if ((u_mode_flags & MODE_ORNAMENT_CENTER) != 0)
    {
        center = saturate(u_ornament_center);
    }
    else
    {
    // 慢漂移（×0.12 控制整体幅度，比 ghostty 的 0.24 略小，避免洞跑太远出屏）。
    center += float2(0.12 * sin(u_time * 0.21) + 0.025 * sin(u_time * 0.083),
                     0.10 * sin(u_time * 0.157 + 2.0) + 0.02 * sin(u_time * 0.117)) * spd;
    // 快抖动（×I，小洞几乎不颤，大洞明显颤）。
    center += I * float2(0.040 * sin(u_time * 0.83) + 0.020 * sin(u_time * 1.31),
                         0.030 * sin(u_time * 1.03 + 1.0));
    }

    // vis：I 极小时整体淡出（种子洞几乎不可见）。
    float vis = smoothstep(0.0, 0.10, I);
    if (vis <= 0.0)
    {
        // 无黑洞：完全透明（透出桌面）。
        return float4(0.0, 0.0, 0.0, 0.0);
    }

    // 吸积盘是否出现只由宿主的开关/强度控制；黑洞大小只影响盘半径和运动尺度。
    float diskPresence = diskUser;
    float diskOn = diskUser > 0.001 ? 1.0 : 0.0;
    float diskShape = diskOn * saturate(0.72 + 0.28 * diskActivity);
    float diskRadiusGrowth = lerp(0.72, 1.34, saturate(growth))
                           * lerp(0.92, 1.20, massDisk);

    // 引力时间膨胀：洞越重盘图案越慢（ghostty 主题特征）。
    float dil = lerp(1.0, DILATION_MIN, saturate(diskShape * lerp(0.75, 1.25, massDisk)));
    float t = u_time;

    // 盘 extent（r_s），sanitize：内边缘留在光子球外。
    float rin  = max(lerp(2.2, DISK_INNER, diskShape), 1.45);
    float rout = max(lerp(4.8, DISK_OUTER * diskSpan * diskRadiusGrowth, diskShape), rin + 0.8);
    float diskGainScale = lerp(0.86, 1.24, diskActivity) * lerp(0.90, 1.24, massDisk)
                        * lerp(0.95, 1.12, saturate(growth));
    float diskOpacityScale = lerp(0.48, 1.0, diskActivity);
    float diskTempScale = lerp(0.90, 1.10, diskActivity) * lerp(0.88, 1.20, massDisk);
    float diskGrainScale = lerp(0.92, 1.55, diskActivity) * lerp(0.85, 1.18, diskSpan)
                         * lerp(0.78, 1.12, saturate(growth));
    float diskIncl = lerp(0.20, 1.55, diskTilt);

    // aspect 校正、以洞为中心（y 用屏幕高度单位）。
    float2 p    = (uv - center) * float2(aspect, 1.0);
    float plen  = length(p);

    // overlay 边界控制：alpha 可以裁出可见范围，但**位移必须先归零**。
    // 如果只把颜色/alpha 淡掉，边缘仍在采样被透镜偏移后的屏幕位置，而 overlay 外面
    // 是未偏移的真实桌面，于是会像放大镜边缘一样断开。这里让 warpMask 在 alpha 淡出
    // 之前归零：边缘处采样坐标回到当前像素，和透明后的桌面连续。
    // radialFlatten 从中段就开始降低位移，并平方压低边缘梯度，让越靠边越平。
    float lensReach = lerp(0.3, 0.5, I) * sizeScale * ringScale;
    float radialFlatten = 1.0 - smoothstep(lensReach * LENS_FLAT_START, lensReach, plen);
    float warpMask = radialFlatten * radialFlatten * vis;
    float lensVis = (1.0 - smoothstep(lensReach, lensReach * 1.15, plen)) * vis;

    // 屏幕↔世界映射：影子角大小 = B_CRIT r_s，占 rh 屏幕单位 → 1 屏幕单位 = W r_s。
    float W  = B_CRIT / max(rh, 1e-4);
    float2 pr = rot(float2(p.x, -p.y), DISK_ROLL) * W;
    float b  = length(pr); // 射线 impact parameter（r_s）

    // 距离窗口（忠实 ghostty）：透镜偏折幅度随 7rh 衰减——只衰减**位移幅度**，
    // 不衰减颜色/alpha。这是 ghostty 让远处文字稳定、近处弯曲的关键。
    float window = exp(-pow(plen / (7 * rh), 2.0));
    float warp = window * warpMask * LENS_STRENGTH * lensScale;

    float bmax = rout + 3.0;            // 超过此 b 的射线碰不到盘（弱场区起点）
    float Z0   = max(14.0, rout + 5.0); // 相机距离

    // 累积盘光（HDR）与背景透射率。
    float3 emitc = float3(0.0, 0.0, 0.0);
    float trans  = 1.0;
    float3 bg    = float3(0.0, 0.0, 0.0);
    bool  captured = false;

    // ================= 远场：解析弱场偏折（忠实 ghostty:453-474）=================
    if (b >= bmax)
    {
        float u    = Z0 * rsqrt(Z0 * Z0 + b * b);
        // 有限相机拟合偏折（与测地线在边界偏差<1%，消除圆形接缝）。
        float defl = (2.0 / (W * W)) / max(plen, 1e-4)
                   * (1.29 * u + 0.07) * max(LENS_DEPTH - 2.14 * u + 0.75, 0.0)
                   * warp * massScale;
        float2 dir = p / max(plen, 1e-5);
        // 微弱色差：蓝比红弯得多一点，远离交接圆淡出。
        float ab = 0.035 * smoothstep(1.0, 2.0, b / bmax);
        float3 term = float3(0.0, 0.0, 0.0);
        if (u_has_capture == 1)
        {
            float2 sp = p - dir * defl;
            term = sample_scene(center + sp / float2(aspect, 1.0), t);
        }
        else
        {
            for (int ci = 0; ci < 3; ci++)
            {
                float k   = 1.0 + (float(ci) - 1.0) * ab;
                float2 sp  = p - dir * defl * k;
                term[ci] = sample_scene_channel(center + sp / float2(aspect, 1.0), t, ci);
            }
        }
        float3 d = normalize(float3(-(pr / b) * (2.0 / b), -1.0));
        bg = term + stars(d) * STAR_GAIN * warp;
    }
    else
    {
        // ================= 近场：测地线积分（忠实 ghostty:476-561）=================
        // 平行射线从 +z 远处相机出发，洞在原点 r_s=1。积分 x''=-(3/2)h²x/r⁵。
        float3 x = float3(pr, Z0);
        float3 v = float3(0.0, 0.0, -1.0);
        float h2 = dot(pr, pr);

        // 盘平面法向量绕屏幕 x 轴倾斜。D3D 这里的屏幕 y 已经按物理相机方向处理，
        // 因此倾角符号相对 ghostty GLSL 需要反过来，避免吸积盘近/远侧上下颠倒。
        float ci2 = cos(diskIncl), si2 = sin(diskIncl);
        float3 nrm = float3(0.0, -si2, ci2);
        float3 e2  = float3(0.0, ci2, si2);
        // 倾角翻转后，轨道方向也要随端口约定翻转；正值时左侧气体朝向观察者，
        // 因而左侧得到多普勒蓝移和 beaming 增亮。
        float sdir = DISK_SPEED < 0.0 ? 1.0 : -1.0;
        float spd  = abs(DISK_SPEED);

        float sPrev = dot(x, nrm);
        float3 xPrev = x;
        bool escaped = false;

        [loop]
        for (int i = 0; i < N_STEPS; i++)
        {
            float r2 = dot(x, x);
            if (r2 < 1.0) { captured = true; break; }            // 穿过视界
            if (x.z < -Z0 && v.z < 0.0) { escaped = true; break; } // 从背面逃出
            if (r2 > 4.0 * Z0 * Z0) { escaped = true; break; }     // 被甩到远侧
            float r = sqrt(r2);
            float dt = clamp(0.16 * r, 0.03, 1.5);
            // leapfrog（kick-drift-kick）。
            float3 a = -1.5 * h2 * massScale * x / (r2 * r2 * r);
            v += a * (0.5 * dt);
            x += v * dt;
            r2 = dot(x, x);
            r  = sqrt(r2);
            a  = -1.5 * h2 * massScale * x / (r2 * r2 * r);
            v += a * (0.5 * dt);

            // 薄盘穿越。
            float s = dot(x, nrm);
            if (s * sPrev < 0.0 && trans > 0.02)
            {
                float tc = sPrev / (sPrev - s);
                float3 xc = lerp(xPrev, x, tc);
                float rc = length(xc);
                if (rc > rin && rc < rout)
                {
                    float rcWidth = max(W / max(res.y, 1.0) * 2.0, 0.015);
                    float band = smoothstep(rin, rin * 1.16 + rcWidth * 1.5, rc)
                               * (1.0 - smoothstep(rout * 0.78 - rcWidth * 2.0, rout, rc));
                    float kep   = pow(rin / rc, 1.5);
                    float gloc  = sqrt(max(1.0 - 1.5 / rc, 0.02));
                    float2 diskLocal = float2(xc.x, dot(xc, e2));
                    float orbit = t * kep * spd * gloc * dil * sdir;
                    float wind = rc * DISK_WIND * 0.055 * diskShape;
                    float2 fastFlow = rot(diskLocal, -orbit + wind);
                    float2 slowFlow = rot(diskLocal, -orbit * 0.42 + wind * 0.65);
                    float phi = atan2(diskLocal.y, diskLocal.x);
                    float flowPhase = phi * 5.0 + orbit * 2.2 + rc * DISK_WIND * 0.18;
                    float flowBand = 0.5 + 0.5 * sin(flowPhase + 1.2 * sin(rc * 1.1 - orbit * 0.65));
                    flowBand = smoothstep(0.18, 0.92, flowBand);

                    float2 grainCoord = fastFlow * diskGrainScale * float2(1.28, 0.72)
                                      + float2(orbit * 0.10, rc * 0.06) * diskShape;
                    float grain = matteNoise(grainCoord * 1.18);
                    float softCloud = matteNoise(slowFlow * diskGrainScale * 0.54 + float2(5.1, -3.7));
                    float fine = matteNoise(fastFlow * diskGrainScale * 2.15 + float2(-2.4, 1.7));
                    float curl = matteNoise(fastFlow * diskGrainScale * 0.22 + float2(orbit * 0.018, rc * 0.025));
                    float matte = softCloud * 0.30 + grain * 0.46 + fine * 0.24
                                + (curl - 0.5) * 0.08 * diskShape;
                    float dustContrast = min(DISK_CONTR * lerp(0.055, 0.105, diskShape), 0.30);
                    float dust = 0.82 + (matte - 0.5) * dustContrast
                               + (flowBand - 0.5) * 0.18 * diskShape;
                    dust = clamp(dust, lerp(0.70, 0.62, diskShape), lerp(0.98, 1.08, diskShape));

                    float3 gasdir = normalize(cross(nrm, xc)) * sdir;
                    float beta    = clamp(rsqrt(max(2.0 * (rc - 1.0), 0.2)), 0.0, 0.99);
                    float approach = dot(gasdir, normalize(-v));
                    float doppler = 1.0 / max(1.0 - beta * approach, 0.05);
                    float gphys = gloc * doppler;
                    float gfac = lerp(1.0, gphys, relativityMix);

                    float xpr   = max(1.0 - sqrt(rin / rc), 0.0);
                    float tprof = pow(rin / rc, 0.75) * pow(xpr, 0.25) / 0.488;
                    float shift01 = saturate((gfac - 0.72) / 0.86);
                    float3 dopplerTint = lerp(float3(1.16, 0.78, 0.58),
                                              float3(0.72, 0.90, 1.30),
                                              shift01);
                    float gravWarm = saturate((1.0 - gloc) * 1.8) * REDSHIFT_TINT;
                    float3 gravTint = lerp(float3(1.0, 1.0, 1.0),
                                           float3(1.10, 0.84, 0.66),
                                           gravWarm);
                    float3 cbb  = blackbody(DISK_TEMP * diskTempScale * tprof * gfac) * diskTint;
                    cbb *= lerp(float3(1.0, 1.0, 1.0), dopplerTint * gravTint, relativityMix * 0.42);
                    float boost = pow(max(gfac, 1e-3), DISK_BEAM);

                    float density = band * dust * lerp(0.94, 1.12, flowBand);
                    // 盘光只由开关/强度决定是否出现；尺寸增长只改变盘半径与流速。
                    emitc += trans * cbb * (DISK_GAIN * diskGainScale * 2.45 * density * tprof * tprof * boost * diskPresence);
                    trans *= 1.0 - clamp(DISK_OPACITY * diskOpacityScale * density * diskPresence, 0.0, 1.0);
                }
            }
            sPrev = s;
            xPrev = x;
        }
        if (!captured && !escaped && dot(x, x) < 4.0) captured = true;

        // ---- 背景：逃逸射线投影回天平面（忠实 ghostty:566-586）----
        if (!captured)
        {
            float3 d = normalize(v);
            bg += stars(d) * STAR_GAIN * warp;
            if (d.z < -0.05)
            {
                // 出射射线投影到 z=-LENS_DEPTH 天平面，映回屏幕。位移被 window 衰减。
                float tpl = (-LENS_DEPTH - x.z) / d.z;
                float3 hp = x + d * tpl;
                float2 q  = rot(hp.xy, -DISK_ROLL) / W;
                float2 sp = float2(q.x, -q.y);
                float2 suv = center + (p + (sp - p) * warp) / float2(aspect, 1.0);

                float toward = smoothstep(0.05, 0.35, -d.z);
                bg += sample_scene(suv, t) * toward;
            }
        }
    }

    // ---- 合成（忠实 ghostty:589）----
    // 盘光是 HDR，tonemap 叠在（未触动的）背景之上。
    float3 col = bg * trans + (float3(1.0, 1.0, 1.0) - exp(-emitc * EXPOSURE));
    // 全局压暗（u_dim：Dimming/ForcedBreak）。
    col *= (1.0 - u_dim * 0.7);

    // ---- alpha（本项目 overlay 唯一非 ghostty 的部分）----
    // ghostty 输出恒 alpha=1（画在终端文字上）。overlay 需要远场透明、近场不透明。
    // lensVis 只负责 overlay 可见范围；warpMask 已在上面负责让位移先归零。
    float a;
    if (captured)
    {
        // 落入视界：纯黑（连背景光都吞噬）。
        col = float3(0.0, 0.0, 0.0);
        a = lensVis;
    }
    else
    {
        // 透镜区：显示扭曲背景 + 盘光。
        a = lensVis;
        // 盘光强/盘遮挡的位置 alpha 抬到 1（盘光区盖住背景）。
        float disklight = dot(emitc, float3(0.299, 0.587, 0.114));
        a = max(a, lensVis * smoothstep(0.05, 0.6, disklight));
        a = max(a, lensVis * (1.0 - trans));
        a = clamp(a, 0.0, 1.0);
    }

    // 预乘 alpha 输出（DWM 按 premultiplied alpha 合成）。
    return float4(col * a, a);
}

