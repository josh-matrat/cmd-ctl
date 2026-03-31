#include <metal_stdlib>
using namespace metal;

// Per-instance data for a terminal cell.
struct CellInstance {
    // Grid position (column, row) as floats.
    float2 grid_pos    [[attribute(0)]];
    // Background color (RGBA).
    float4 bg_color    [[attribute(1)]];
    // Foreground color (RGBA).
    float4 fg_color    [[attribute(2)]];
    // UV coordinates in the glyph atlas (u0, v0, u1, v1).
    float4 glyph_uv   [[attribute(3)]];
    // Flags: bit 0 = has glyph, bit 1 = cursor.
    uint   flags       [[attribute(4)]];
};

struct Uniforms {
    float2 viewport_size;   // In pixels.
    float2 cell_size;       // In pixels.
    float2 grid_offset;     // Pixel offset of the grid origin.
};

// Vertex output for background quad pass.
struct BgVertexOut {
    float4 position [[position]];
    float4 color;
};

// Vertex output for glyph pass.
struct GlyphVertexOut {
    float4 position [[position]];
    float2 tex_coord;
    float4 fg_color;
};

// Background pass: renders colored rectangles for each cell.
vertex BgVertexOut bg_vertex(
    uint vertex_id [[vertex_id]],
    uint instance_id [[instance_id]],
    const device CellInstance* instances [[buffer(0)]],
    constant Uniforms& uniforms [[buffer(1)]]
) {
    CellInstance cell = instances[instance_id];

    // Unit quad: 2 triangles = 6 vertices.
    float2 positions[6] = {
        float2(0, 0), float2(1, 0), float2(0, 1),
        float2(1, 0), float2(1, 1), float2(0, 1),
    };

    float2 pos = positions[vertex_id];

    // Convert grid position to pixel position.
    float2 pixel_pos = uniforms.grid_offset + (cell.grid_pos + pos) * uniforms.cell_size;

    // Convert to clip space (-1..1).
    float2 clip = (pixel_pos / uniforms.viewport_size) * 2.0 - 1.0;
    clip.y = -clip.y; // Flip Y (Metal's clip space has Y up, we want Y down).

    BgVertexOut out;
    out.position = float4(clip, 0.0, 1.0);
    out.color = cell.bg_color;
    return out;
}

fragment float4 bg_fragment(BgVertexOut in [[stage_in]]) {
    return in.color;
}

// Glyph pass: renders textured quads for each cell that has a glyph.
vertex GlyphVertexOut glyph_vertex(
    uint vertex_id [[vertex_id]],
    uint instance_id [[instance_id]],
    const device CellInstance* instances [[buffer(0)]],
    constant Uniforms& uniforms [[buffer(1)]]
) {
    CellInstance cell = instances[instance_id];

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

    float2 pixel_pos = uniforms.grid_offset + (cell.grid_pos + pos) * uniforms.cell_size;
    float2 clip = (pixel_pos / uniforms.viewport_size) * 2.0 - 1.0;
    clip.y = -clip.y;

    // Map unit tex coords to the glyph's UV rect in the atlas.
    float2 uv = float2(
        mix(cell.glyph_uv.x, cell.glyph_uv.z, tc.x),
        mix(cell.glyph_uv.y, cell.glyph_uv.w, tc.y)
    );

    GlyphVertexOut out;
    out.position = float4(clip, 0.0, 1.0);
    out.tex_coord = uv;
    out.fg_color = cell.fg_color;
    return out;
}

fragment float4 glyph_fragment(
    GlyphVertexOut in [[stage_in]],
    texture2d<float> atlas [[texture(0)]]
) {
    constexpr sampler s(mag_filter::linear, min_filter::linear);
    float4 tex_sample = atlas.sample(s, in.tex_coord);

    // The atlas stores white glyphs on transparent background.
    // Use the alpha from the texture and tint with foreground color.
    float alpha = tex_sample.a;
    if (alpha < 0.01) {
        discard_fragment();
    }
    return float4(in.fg_color.rgb, in.fg_color.a * alpha);
}
