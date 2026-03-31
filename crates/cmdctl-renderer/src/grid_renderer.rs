use anyhow::Result;
use metal::*;

use crate::atlas::GlyphAtlas;
use crate::pipeline::{CellInstance, RenderPipelines, Uniforms};
use crate::text::FontInfo;

/// Terminal color palette — black, red, gold glass theme.
pub mod colors {
    pub const BG: [f32; 4] = [0.05, 0.03, 0.03, 0.92];       // near-black, subtle glass
    pub const FG: [f32; 4] = [0.78, 0.75, 0.70, 1.0];        // warm light gray
    pub const CURSOR: [f32; 4] = [0.85, 0.65, 0.13, 1.0];    // gold cursor

    /// ANSI color palette (normal + bright).
    pub const ANSI: [[f32; 4]; 16] = [
        [0.08, 0.05, 0.05, 1.0],      // 0  black
        [0.85, 0.15, 0.15, 1.0],      // 1  red       — crimson
        [0.55, 0.60, 0.30, 1.0],      // 2  green     — muted olive
        [0.85, 0.65, 0.13, 1.0],      // 3  yellow    — gold
        [0.45, 0.50, 0.60, 1.0],      // 4  blue      — muted steel
        [0.75, 0.25, 0.35, 1.0],      // 5  magenta   — warm rose
        [0.65, 0.65, 0.60, 1.0],      // 6  cyan      — warm silver
        [0.90, 0.87, 0.82, 1.0],      // 7  white     — warm white
        [0.25, 0.22, 0.20, 1.0],      // 8  br black  — dark gray
        [0.95, 0.30, 0.25, 1.0],      // 9  br red    — bright crimson
        [0.45, 0.42, 0.38, 1.0],      // 10 br green  — dim warm gray
        [0.95, 0.75, 0.25, 1.0],      // 11 br yellow — bright gold
        [0.55, 0.52, 0.50, 1.0],      // 12 br blue   — medium gray
        [0.60, 0.45, 0.65, 1.0],      // 13 br mag    — muted violet
        [0.70, 0.68, 0.63, 1.0],      // 14 br cyan   — light warm
        [0.95, 0.92, 0.87, 1.0],      // 15 br white  — cream
    ];
}

/// Renders a terminal grid to a Metal command buffer.
pub struct GridRenderer {
    pub pipelines: RenderPipelines,
    pub font: FontInfo,
    pub atlas: GlyphAtlas,
    bg_buffer: Buffer,
    glyph_buffer: Buffer,
    max_cells: usize,
}

impl GridRenderer {
    pub fn new(device: &DeviceRef, font: FontInfo, scale: f64) -> Result<Self> {
        let pipelines = RenderPipelines::new(device)?;

        // Rasterize glyphs at Retina resolution.
        let cell_w = (font.cell_width * scale).ceil() as u32;
        let cell_h = (font.cell_height * scale).ceil() as u32;
        let mut atlas = GlyphAtlas::new(device, cell_w, cell_h)?;
        atlas.cache_ascii(&font, scale);

        let max_cells = 320 * 100;
        let buffer_size = (max_cells * std::mem::size_of::<CellInstance>()) as u64;
        let bg_buffer = device.new_buffer(buffer_size, MTLResourceOptions::StorageModeManaged);
        let glyph_buffer = device.new_buffer(buffer_size, MTLResourceOptions::StorageModeManaged);

        Ok(Self {
            pipelines,
            font,
            atlas,
            bg_buffer,
            glyph_buffer,
            max_cells,
        })
    }

    /// Render the terminal grid.
    /// `viewport_width`/`viewport_height` are in physical pixels.
    /// `scale` is the Retina scale factor.
    pub fn render(
        &mut self,
        encoder: &RenderCommandEncoderRef,
        viewport_width: f32,
        viewport_height: f32,
        scale: f32,
        cells: &[(usize, usize, char, [f32; 4], [f32; 4], bool)],
    ) {
        // Cell size in physical pixels (what the shader works in).
        let cell_w = self.font.cell_width as f32 * scale;
        let cell_h = self.font.cell_height as f32 * scale;

        let total = cells.len().min(self.max_cells);

        let bg_instances = unsafe {
            std::slice::from_raw_parts_mut(
                self.bg_buffer.contents() as *mut CellInstance,
                total,
            )
        };
        let glyph_instances = unsafe {
            std::slice::from_raw_parts_mut(
                self.glyph_buffer.contents() as *mut CellInstance,
                total,
            )
        };

        let mut glyph_count = 0usize;

        for (i, &(col, row, ch, fg, bg, _is_cursor)) in cells.iter().enumerate().take(total) {
            // Background instance for every cell.
            bg_instances[i] = CellInstance {
                grid_pos: [col as f32, row as f32],
                bg_color: bg,
                fg_color: fg,
                glyph_uv: [0.0; 4],
                flags: 0,
                _padding: [0; 3],
            };

            // Glyph instance only for visible characters.
            if ch > ' ' {
                let uv = self.atlas.get_or_insert(ch, &self.font, scale as f64);
                glyph_instances[glyph_count] = CellInstance {
                    grid_pos: [col as f32, row as f32],
                    bg_color: bg,
                    fg_color: fg,
                    glyph_uv: [uv.u0, uv.v0, uv.u1, uv.v1],
                    flags: 1,
                    _padding: [0; 3],
                };
                glyph_count += 1;
            }
        }

        // Flush both buffers.
        let bg_range = metal::NSRange::new(0, (total * std::mem::size_of::<CellInstance>()) as u64);
        self.bg_buffer.did_modify_range(bg_range);

        if glyph_count > 0 {
            let glyph_range = metal::NSRange::new(0, (glyph_count * std::mem::size_of::<CellInstance>()) as u64);
            self.glyph_buffer.did_modify_range(glyph_range);
        }

        let uniforms = Uniforms {
            viewport_size: [viewport_width, viewport_height],
            cell_size: [cell_w, cell_h],
            grid_offset: [0.0, 0.0],
        };

        // Pass 1: Background quads (all cells).
        encoder.set_render_pipeline_state(&self.pipelines.bg_pipeline);
        encoder.set_vertex_buffer(0, Some(&self.bg_buffer), 0);
        encoder.set_vertex_bytes(
            1,
            std::mem::size_of::<Uniforms>() as u64,
            &uniforms as *const Uniforms as *const _,
        );
        encoder.draw_primitives_instanced(
            MTLPrimitiveType::Triangle,
            0,
            6,
            total as u64,
        );

        // Pass 2: Glyph quads (only cells with visible characters).
        if glyph_count > 0 {
            encoder.set_render_pipeline_state(&self.pipelines.glyph_pipeline);
            encoder.set_vertex_buffer(0, Some(&self.glyph_buffer), 0);
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
                glyph_count as u64,
            );
        }
    }
}
