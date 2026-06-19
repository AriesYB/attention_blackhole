// =============================================================================
// 注意力黑洞 —— 黑洞 raymarching HLSL shader
// =============================================================================
//
// **原创实现**（非 ShaderToy 移植）。本文件为 attention_blackhole 项目自有代码，
// 按 MIT 许可证随项目发布（见 LICENSE）。事件视界 + 引力透镜 + 吸积盘的简化数学模型，
// 由 spec §4.1 的「黑洞隐喻」视觉需求驱动：load 越高 → 视界半径越大、吸积盘越亮、
// 暗角越深，营造「吞噬注意力」的压迫感。
//
// 数学要点（简化，非物理精确）：
// - 事件视界半径 r_eh = lerp(0.02, 0.35, u_load) * min(u_resolution)，load 归一化 0..1。
// - 引力透镜：光线在视界附近被弯折，弯折量随 1/r 增长；r < r_eh 视为落入视界 → 黑。
// - 吸积盘：视界外、倾斜平面上的发光环带，亮度随 u_load 与距视界的距离调制。
// - 背景：u_has_capture=1 时采样捕获纹理（经透镜扭曲），否则程序化星云。
//
// 入口：VS = vs_main（全屏三角形），PS = ps_main。
// cbuffer FrameConstants 每帧由 painter 更新。

cbuffer FrameConstants : register(b0)
{
    float u_load;        // 注意力负荷，归一化 0..1（Frame.load / 100）
    float u_dim;         // 压暗强度 0..1（Dimming/ForcedBreak 时增大）
    float u_time;        // 渲染时间秒（驱动吸积盘旋转/闪烁）
    float _pad;          // cbuffer 16 字节对齐填充
    float2 u_resolution; // backbuffer 像素尺寸
    uint  u_has_capture; // 1 = 用捕获纹理背景，0 = 程序化背景
    float _pad2;         // 对齐
};

Texture2D    u_capturedTexture : register(t0);
SamplerState u_sampler         : register(s0);

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
// 工具：简单 hash 噪声（程序化星云用，避免引入噪声库）。
float hash21(float2 p)
{
    p = frac(p * float2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return frac(p.x * p.y);
}

// 程序化星云背景：u_has_capture=0 时用。低饱和深色 + 星点。
float3 procedural_background(float2 uv, float time)
{
    // 深紫蓝渐变底
    float3 col = lerp(float3(0.02, 0.02, 0.05), float3(0.05, 0.03, 0.10), uv.y);

    // 缓慢流动的星云团（低频噪声）
    float2 q = uv * 3.0 + float2(time * 0.02, time * 0.01);
    float n = hash21(floor(q)) * 0.5 + hash21(floor(q * 2.0)) * 0.25;
    col += float3(0.10, 0.05, 0.15) * smoothstep(0.6, 1.0, n) * 0.3;

    // 星点：高频稀疏亮点
    float2 sid = floor(uv * float2(160.0, 90.0));
    float star = hash21(sid);
    float twinkle = 0.5 + 0.5 * sin(time * 2.0 + star * 6.28);
    if (star > 0.985)
    {
        col += float3(0.8, 0.85, 1.0) * twinkle * (star - 0.985) * 60.0;
    }
    return col;
}

// -----------------------------------------------------------------------------
// 引力透镜坐标映射：把屏幕 uv 映射到「弯曲后的采样坐标」。
// 距视界中心 r 处，向外推开 = bend_strength / r。越近视界弯得越狠。
float2 lens_warp(float2 centered, float r_eh)
{
    float r = length(centered) + 1e-5;
    // 弯折量随 load 增大；r_eh 外一定范围内显著，远处衰减。
    float bend = (r_eh * 1.8) / (r + r_eh * 0.5);
    // 向远离中心方向推开（模拟光线绕过视界）。
    return centered + normalize(centered) * bend * 0.5;
}

// -----------------------------------------------------------------------------
// pixel shader 主入口。
float4 ps_main(VSOut i) : SV_Target
{
    // 屏幕中心化坐标（短边归一化到 -1..1，保持圆形）。
    float2 res = u_resolution;
    float aspect = res.x / max(res.y, 1.0);
    float2 uv = i.uv;
    float2 centered = (uv - 0.5) * float2(aspect, 1.0) * 2.0;

    // 事件视界半径：load 驱动涨缩。
    float r_eh = lerp(0.02, 0.35, clamp(u_load, 0.0, 1.0));

    // 背景：捕获纹理（经透镜扭曲）或程序化。
    float2 warped_uv = lens_warp(centered, r_eh);
    // warped 坐标转回 0..1 采样坐标。
    float2 sample_uv = warped_uv / float2(aspect, 1.0) / 2.0 + 0.5;
    float3 bg;
    if (u_has_capture == 1)
    {
        // 越界采样钳到边缘，避免黑框。
        sample_uv = clamp(sample_uv, 0.0, 1.0);
        bg = u_capturedTexture.Sample(u_sampler, sample_uv).rgb;
    }
    else
    {
        bg = procedural_background(sample_uv, u_time);
    }

    // 距视界中心距离。
    float r = length(centered);

    float3 col = bg;

    // alpha：决定该像素对桌面的遮挡程度（NOREDIRECTIONBITMAP + DWM 逐像素 alpha）。
    // - 视界内/吸积盘/光子环：高 alpha（遮挡注意力）。
    // - 远离视界的程序化背景：低 alpha（让桌面透出，黑洞「悬浮」感）。
    // 随 load 上升，整体 alpha 圈外扩 + 加深，强化「吞噬」。
    float alpha = 0.0;

    // 1) 落入事件视界：纯黑（连背景光都吞噬）+ 全不透明。
    if (r < r_eh)
    {
        col = float3(0.0, 0.0, 0.0);
        alpha = 1.0;
    }
    else
    {
        // 2) 吸积盘：视界外的发光环带，倾斜（椭圆）投影。
        //    椭圆：y 方向压扁 0.3，模拟从侧面看圆盘。
        float2 disk = centered;
        disk.y *= 3.3; // 反压扁到圆
        float rd = length(disk);
        // 盘范围：视界外 ~3.2 倍视界半径（加宽，盘更醒目）。
        float disk_inner = r_eh * 1.02;
        float disk_outer = r_eh * 3.2;
        if (rd > disk_inner && rd < disk_outer && centered.y > -r_eh * 0.15)
        {
            // 距视界越近越亮（引力红移反之简化）+ 旋转条纹。
            float t = (rd - disk_inner) / (disk_outer - disk_inner);
            float angle = atan2(disk.y, disk.x);
            float spin = sin(angle * 8.0 + u_time * 1.5) * 0.5 + 0.5;
            // 亮度基线抬高（1.0 + 0.5*spin），任何桌面背景下一眼可见。
            float brightness = (1.0 - t) * (1.0 + 0.5 * spin);
            // 盘色：高温内圈白蓝（更亮） → 外圈橙红。
            float3 hot = lerp(float3(1.0, 0.95, 0.8), float3(1.0, 0.45, 0.1), t);
            col += hot * brightness * (0.6 + u_load * 0.8);
            alpha = max(alpha, 0.95); // 盘接近不透明（提亮后更扎实）
        }

        // 3) 光子环：视界边缘极亮环（爱因斯坦环简化）。衰减变缓（40→22）使亮环更宽更醒目。
        float ring = exp(-pow((r - r_eh * 1.02) * 22.0, 2.0));
        col += float3(1.0, 0.85, 0.6) * ring * (1.0 + u_load * 0.5);
        alpha = max(alpha, ring * 0.95);

        // 4) 暗角：视界外一圈渐变压暗，强化「被吸引」感。随 load 加深。
        float vignette = smoothstep(r_eh * 1.2, r_eh * 0.4, r);
        col *= lerp(1.0, 0.3, vignette * u_load);

        // 5) 局部光晕：仅视界外一小圈有 alpha，远处快速衰减到 0，桌面完全透出。
        //    （原为全屏 halo 暗色蒙版，会遮住桌面——改为紧贴黑洞的局部光晕，
        //    既勾勒出黑洞轮廓/压迫感，又不霸屏。随 load 加深。）
        float local_glow = exp(-(r - r_eh) * 8.0) * (0.4 + u_load * 0.4);
        alpha = max(alpha, local_glow);
    }

    // 全局压暗（u_dim：Dimming/ForcedBreak 状态施加）。
    col *= (1.0 - u_dim * 0.7);

    // 预乘 alpha 输出（DWM 按 premultiplied alpha 合成）。
    return float4(col * alpha, alpha);
}
