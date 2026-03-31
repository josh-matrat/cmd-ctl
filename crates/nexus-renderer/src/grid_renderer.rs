use anyhow::Result;
use metal::*;

use crate::atlas::GlyphAtlas;
use crate::pipeline::{CellInstance, RenderPipelines, Uniforms};
use crate::text::FontInfo;

/// Default terminal colors (Solarized Dark inspired).
pub mod colors {
    pub const BG: [f32; 4] = [0.0, 0.168, 0.211, 1.0];       // #002b36
    pub const FG: [f32; 4] = [0.514, 0.580, 0.588, 1.0];     // #839496
    pub const CURSOR: [f32; 4] = [0.514, 0.580, 0.588, 1.0];

    /// ANSI color palette (normal + bright).
    pub const ANSI: [[f32; 4]; 16] = [
        [0.027, 0.211, 0.258, 1.0],   // 0  black    #073642
        [0.862, 0.196, 0.184, 1.0],   // 1  red      #dc322f
        [0.521, 0.600, 0.0, 1.0],     // 2  green    #859900
        [0.709, 0.537, 0.0, 1.0],     // 3  yellow   #b58900
        [0.149, 0.545, 0.823, 1.0],   // 4  blue     #268bd2
        [0.827, 0.211, 0.509, 1.0],   // 5  magenta  #d33682
        [0.164, 0.631, 0.596, 1.0],   // 6  cyan     #2aa198
        [0.933, 0.909, 0.835, 1.0],   // 7  white    #eee8d5
        [0.0, 0.168, 0.211, 1.0],     // 8  br black #002b36
        [0.796, 0.294, 0.086, 1.0],   // 9  br red   #cb4b16
        [0.345, 0.431, 0.458, 1.0],   // 10 br green #586e75
        [0.396, 0.482, 0.513, 1.0],   // 11 br yellow#657b83
        [0.514, 0.580, 0.588, 1.0],   // 12 br blue  #839496
        [0.423, 0.443, 0.768, 1.0],   // 13 br mag   #6c71c4
        [0.576, 0.631, 0.631, 1.0],   // 14 br cyan  #93a1a1
        [0.992, 0.964, 0.890, 1.0],   // 15 br white #fdf6e3
    ];
}

/// Renders a terminal grid to a Metal command buffer.
pub struct GridRenderer {
    pub pipelines: RenderPipelines,
    pub font: FontInfo,
    pub atlas: GlyphAtlas,
    instance_buffer: Buffer,
    max_cells: usize,
}

impl GridRenderer {
    pub fn new(device: &DeviceRef, font: FontInfo) -> Result<Self> {
        let pipelines = RenderPipelines::new(device)?;

        let cell_w = font.cell_width.ceil() as u32;
        let cell_h = font.cell_height.ceil() as u32;
        let mut atlas = GlyphAtlas::new(device, cell_w, cell_h)?;
        atlas.cache_ascii(&font);

        // Pre-allocate for a large terminal (200 cols x 60 rows).
        let max_cells = 200 * 60;
        let buffer_size = (max_cells * std::mem::size_of::<CellInstance>()) as u64;
        let instance_buffer = device.new_buffer(buffer_size, MTLResourceOptions::StorageModeManaged);

        Ok(Self {
            pipelines,
            font,
            atlas,
            instance_buffer,
            max_cells,
        })
    }

    /// Build instance data from a terminal grid and render it.
    ///
    /// `cells` is a callback that provides (col, row, char, fg_color, bg_color, is_cursor)
    /// for each cell in the grid.
    pub fn render(
        &mut self,
        encoder: &RenderCommandEncoderRef,
        viewport_width: f32,
        viewport_height: f32,
        _cols: usize,
        _rows: usize,
        cells: &[(usize, usize, char, [f32; 4], [f32; 4], bool)],
    ) {
        let cell_w = self.font.cell_width as f32;
        let cell_h = self.font.cell_height as f32;

        // Build instance data.
        let cell_count = cells.len().min(self.max_cells);
        let instances = unsafe {
            std::slice::from_raw_parts_mut(
                self.instance_buffer.contents() as *mut CellInstance,
                cell_count,
            )
        };

        let mut glyph_count = 0usize;

        for (i, &(col, row, ch, fg, bg, is_cursor)) in cells.iter().enumerate().take(cell_count) {
            let glyph_uv = if ch > ' ' {
                let uv = self.atlas.get_or_insert(ch, &self.font);
                [uv.u0, uv.v0, uv.u1, uv.v1]
            } else {
                [0.0, 0.0, 0.0, 0.0]
            };

            let has_glyph = ch > ' ';
            let flags = if has_glyph { 1u32 } else { 0u32 } | if is_cursor { 2u32 } else { 0u32 };

            instances[i] = CellInstance {
                grid_pos: [col as f32, row as f32],
                bg_color: bg,
                fg_color: fg,
                glyph_uv,
                flags,
                _padding: [0; 3],
            };

            if has_glyph {
                glyph_count += 1;
            }
        }

        // Flush buffer to GPU.
        let range = metal::NSRange::new(0, (cell_count * std::mem::size_of::<CellInstance>()) as u64);
        self.instance_buffer.did_modify_range(range);

        let uniforms = Uniforms {
            viewport_size: [viewport_width, viewport_height],
            cell_size: [cell_w, cell_h],
            grid_offset: [0.0, 0.0],
        };

        // Pass 1: Background quads.
        encoder.set_render_pipeline_state(&self.pipelines.bg_pipeline);
        encoder.set_vertex_buffer(0, Some(&self.instance_buffer), 0);
        encoder.set_vertex_bytes(
            1,
            std::mem::size_of::<Uniforms>() as u64,
            &uniforms as *const Uniforms as *const _,
        );
        encoder.draw_primitives_instanced(
            MTLPrimitiveType::Triangle,
            0,
            6,
            cell_count as u64,
        );

        // Pass 2: Glyph quads (same instances, different pipeline).
        if glyph_count > 0 {
            encoder.set_render_pipeline_state(&self.pipelines.glyph_pipeline);
            encoder.set_vertex_buffer(0, Some(&self.instance_buffer), 0);
            encoder.set_vertex_bytes(
                1,
                std::mem::size_of::<Uniforms>() as u64,
                &uniforms as *const Uniforms as *const _,
            );
            encoder.set_fragment_texture(0, Some(&self.atlas.texture));
            encoder.draw_primitives_instanced(
                MTLPrimitiveType::Triangle,
                0,
                6,
                cell_count as u64,
            );
        }
    }
}
