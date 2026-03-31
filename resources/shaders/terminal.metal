#include <metal_stdlib>
using namespace metal;

// Per-instance data for a terminal cell.
// Uses packed types to match Rust's #[repr(C)] layout (no alignment padding).
struct CellInstance {
    packed_float2 grid_pos;   // 8 bytes,  offset 0
    packed_float4 bg_color;   // 16 bytes, offset 8
    packed_float4 fg_color;   // 16 bytes, offset 24
    packed_float4 glyph_uv;   // 16 bytes, offset 40
    uint          flags;       // 4 bytes,  offset 56
    uint          _pad[3];     // 12 bytes, offset 60  -> total 72 bytes
};

struct Uniforms {
    packed_float2 viewport_size;
    packed_float2 cell_size;
    packed_float2 grid_offset;
};

struct BgVertexOut {
    float4 position [[position]];
    float4 color;
};

struct GlyphVertexOut {
    float4 position [[position]];
    float2 tex_coord;
    float4 fg_color;
};

// Background pass: colored rectangle for each cell.
vertex BgVertexOut bg_vertex(
    uint vertex_id [[vertex_id]],
    uint instance_id [[instance_id]],
    const device CellInstance* instances [[buffer(0)]],
    constant Uniforms& uniforms [[buffer(1)]]
) {
    const device CellInstance& cell = instances[instance_id];

    float2 positions[6] = {
        float2(0, 0), float2(1, 0), float2(0, 1),
        float2(1, 0), float2(1, 1), float2(0, 1),
    };

    float2 pos = positions[vertex_id];
    float2 gp = float2(cell.grid_pos);
    float2 cs = float2(uniforms.cell_size);
    float2 vs = float2(uniforms.viewport_size);
    float2 go = float2(uniforms.grid_offset);

    float2 pixel_pos = go + (gp + pos) * cs;
    float2 clip = (pixel_pos / vs) * 2.0 - 1.0;
    clip.y = -clip.y;

    BgVertexOut out;
    out.position = float4(clip, 0.0, 1.0);
    out.color = float4(cell.bg_color);
    return out;
}

fragment float4 bg_fragment(BgVertexOut in [[stage_in]]) {
    return in.color;
}

// Glyph pass: textured quad for each cell with a visible character.
vertex GlyphVertexOut glyph_vertex(
    uint vertex_id [[vertex_id]],
    uint instance_id [[instance_id]],
    const device CellInstance* instances [[buffer(0)]],
    constant Uniforms& uniforms [[buffer(1)]]
) {
    const device CellInstance& cell = instances[instance_id];

    float2 positions[6] = {
        float2(0, 0), float2(1, 0), float2(0, 1),
        float2(1, 0), float2(1, 1), float2(0, 1),
    };

    float2 tex_coords[6] = {
        float2(0, 0), float2(1, 0), float2(0, 1),
        float2(1, 0), float2(1, 1), float2(0, 1),
    };

    float2 pos = positions[vertex_id];
    float2 tc = tex_coords[vertex_id];
    float2 gp = float2(cell.grid_pos);
    float2 cs = float2(uniforms.cell_size);
    float2 vs = float2(uniforms.viewport_size);
    float2 go = float2(uniforms.grid_offset);
    float4 guv = float4(cell.glyph_uv);

    float2 pixel_pos = go + (gp + pos) * cs;
    float2 clip = (pixel_pos / vs) * 2.0 - 1.0;
    clip.y = -clip.y;

    float2 uv = float2(
        mix(guv.x, guv.z, tc.x),
        mix(guv.y, guv.w, tc.y)
    );

    GlyphVertexOut out;
    out.position = float4(clip, 0.0, 1.0);
    out.tex_coord = uv;
    out.fg_color = float4(cell.fg_color);
    return out;
}

fragment float4 glyph_fragment(
    GlyphVertexOut in [[stage_in]],
    texture2d<float> atlas [[texture(0)]]
) {
    constexpr sampler s(mag_filter::nearest, min_filter::nearest);
    float4 tex_sample = atlas.sample(s, in.tex_coord);

    float alpha = tex_sample.a;
    if (alpha < 0.01) {
        discard_fragment();
    }
    return float4(in.fg_color.rgb, in.fg_color.a * alpha);
}
