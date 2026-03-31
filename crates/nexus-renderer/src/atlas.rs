use std::collections::HashMap;
use metal::*;
use anyhow::Result;

use crate::text::FontInfo;

/// UV coordinates for a glyph in the atlas.
#[derive(Debug, Clone, Copy)]
pub struct GlyphUV {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

/// Manages a Metal texture atlas for rasterized glyphs.
pub struct GlyphAtlas {
    pub texture: Texture,
    atlas_width: u32,
    atlas_height: u32,
    cell_width: u32,
    cell_height: u32,
    /// Grid position for next glyph placement.
    next_col: u32,
    next_row: u32,
    cols_per_row: u32,
    max_rows: u32,
    glyph_map: HashMap<char, GlyphUV>,
}

impl GlyphAtlas {
    /// Create a new glyph atlas with space for many glyphs.
    pub fn new(device: &DeviceRef, cell_width: u32, cell_height: u32) -> Result<Self> {
        // Atlas: 32x32 grid of cells. Enough for ASCII + common unicode initially.
        let cols = 64u32;
        let rows = 64u32;
        let atlas_width = cols * cell_width;
        let atlas_height = rows * cell_height;

        let descriptor = TextureDescriptor::new();
        descriptor.set_pixel_format(MTLPixelFormat::RGBA8Unorm);
        descriptor.set_width(atlas_width as u64);
        descriptor.set_height(atlas_height as u64);
        descriptor.set_storage_mode(MTLStorageMode::Managed);
        descriptor.set_usage(MTLTextureUsage::ShaderRead);

        let texture = device.new_texture(&descriptor);

        Ok(Self {
            texture,
            atlas_width,
            atlas_height,
            cell_width,
            cell_height,
            next_col: 0,
            next_row: 0,
            cols_per_row: cols,
            max_rows: rows,
            glyph_map: HashMap::new(),
        })
    }

    /// Get or rasterize a glyph, returning its UV coordinates.
    pub fn get_or_insert(&mut self, ch: char, font: &FontInfo) -> GlyphUV {
        if let Some(&uv) = self.glyph_map.get(&ch) {
            return uv;
        }

        // Rasterize the glyph.
        let (w, h, pixels) = match font.rasterize_glyph(ch) {
            Some(result) => result,
            None => {
                // Return empty UV for missing glyphs.
                let uv = GlyphUV { u0: 0.0, v0: 0.0, u1: 0.0, v1: 0.0 };
                self.glyph_map.insert(ch, uv);
                return uv;
            }
        };

        if self.next_row >= self.max_rows {
            // Atlas is full — return empty for now.
            // TODO: grow atlas or evict LRU.
            tracing::warn!("Glyph atlas full, cannot add '{}'", ch);
            let uv = GlyphUV { u0: 0.0, v0: 0.0, u1: 0.0, v1: 0.0 };
            self.glyph_map.insert(ch, uv);
            return uv;
        }

        // Compute pixel position in the atlas.
        let px = self.next_col * self.cell_width;
        let py = self.next_row * self.cell_height;

        // Upload glyph pixels to the atlas texture.
        let region = MTLRegion::new_2d(px as u64, py as u64, w as u64, h as u64);
        self.texture.replace_region(
            region,
            0,
            pixels.as_ptr() as *const _,
            (w * 4) as u64,
        );

        // Compute UV coordinates.
        let uv = GlyphUV {
            u0: px as f32 / self.atlas_width as f32,
            v0: py as f32 / self.atlas_height as f32,
            u1: (px + self.cell_width) as f32 / self.atlas_width as f32,
            v1: (py + self.cell_height) as f32 / self.atlas_height as f32,
        };

        self.glyph_map.insert(ch, uv);

        // Advance to next slot.
        self.next_col += 1;
        if self.next_col >= self.cols_per_row {
            self.next_col = 0;
            self.next_row += 1;
        }

        uv
    }

    /// Pre-cache ASCII printable characters.
    pub fn cache_ascii(&mut self, font: &FontInfo) {
        for ch in 32u8..=126u8 {
            self.get_or_insert(ch as char, font);
        }
    }
}
