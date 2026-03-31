use std::ffi::c_void;

use anyhow::Result;
use core_graphics::base::kCGImageAlphaPremultipliedLast;
use core_graphics::color_space::CGColorSpace;
use core_graphics::context::CGContext;
use core_graphics::geometry::{CGPoint, CGSize};
use core_text::font as ct_font;
use core_text::font::CTFont;
use core_text::font_descriptor::kCTFontOrientationDefault;
use core_foundation::base::TCFType;
use foreign_types::ForeignType;

extern "C" {
    fn CTFontDrawGlyphs(
        font: core_text::font::CTFontRef,
        glyphs: *const u16,
        positions: *const CGPoint,
        count: usize,
        context: *mut c_void,
    );
}

/// A loaded monospace font with metrics (in logical/point units).
pub struct FontInfo {
    pub font: CTFont,
    pub cell_width: f64,
    pub cell_height: f64,
    pub descent: f64,
}

impl FontInfo {
    /// Load a monospace font by name and size (in points).
    pub fn load(font_name: &str, size: f64) -> Result<Self> {
        let font = ct_font::new_from_name(font_name, size)
            .map_err(|_| anyhow::anyhow!("Failed to load font: {}", font_name))?;

        let ascent = font.ascent();
        let descent = font.descent();
        let leading = font.leading();
        let cell_height = (ascent + descent + leading).ceil();

        // Measure advance width of 'M' for monospace cell width.
        let mut glyphs = [0u16; 1];
        let chars = ['M' as u16];
        unsafe {
            font.get_glyphs_for_characters(chars.as_ptr(), glyphs.as_mut_ptr(), 1);
        }

        let mut advances = [CGSize::new(0.0, 0.0)];
        unsafe {
            font.get_advances_for_glyphs(
                kCTFontOrientationDefault,
                glyphs.as_ptr(),
                advances.as_mut_ptr(),
                1,
            );
        }
        let cell_width = advances[0].width.ceil();

        Ok(Self { font, cell_width, cell_height, descent })
    }

    /// Rasterize a single glyph to an RGBA bitmap at the given scale factor.
    /// Returns (width, height, bytes_per_row, pixel_data) in physical pixels.
    pub fn rasterize_glyph(&self, ch: char, scale: f64) -> Option<RasterizedGlyph> {
        let mut glyphs = [0u16; 1];
        let chars = [ch as u16];
        unsafe {
            self.font.get_glyphs_for_characters(chars.as_ptr(), glyphs.as_mut_ptr(), 1);
        }

        if glyphs[0] == 0 && ch != '\0' {
            return None;
        }

        let w = (self.cell_width * scale).ceil() as u32;
        let h = (self.cell_height * scale).ceil() as u32;

        if w == 0 || h == 0 {
            return None;
        }

        let color_space = CGColorSpace::create_device_rgb();
        let mut ctx = CGContext::create_bitmap_context(
            None,
            w as usize,
            h as usize,
            8,
            0, // Let CoreGraphics choose optimal alignment.
            &color_space,
            kCGImageAlphaPremultipliedLast,
        );

        // Clear to transparent.
        ctx.clear_rect(core_graphics::geometry::CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(w as f64, h as f64),
        ));

        // Scale for Retina.
        ctx.scale(scale, scale);

        // White text, tinted by shader.
        ctx.set_rgb_fill_color(1.0, 1.0, 1.0, 1.0);

        // Draw glyph at baseline position (descent from bottom, in logical units).
        let origin = CGPoint::new(0.0, self.descent);
        unsafe {
            CTFontDrawGlyphs(
                self.font.as_concrete_TypeRef(),
                glyphs.as_ptr(),
                [origin].as_ptr(),
                1,
                ctx.as_ptr() as *mut c_void,
            );
        }

        // Read bitmap data. bytes_per_row may differ from w*4 due to alignment.
        let bytes_per_row = ctx.bytes_per_row();
        let data = ctx.data();

        // Copy data into a tightly packed RGBA buffer (w*4 per row).
        let tight_stride = (w * 4) as usize;
        let mut pixels = vec![0u8; tight_stride * h as usize];
        for row in 0..h as usize {
            let src_start = row * bytes_per_row;
            let dst_start = row * tight_stride;
            let src_end = src_start + tight_stride.min(bytes_per_row);
            let dst_end = dst_start + tight_stride.min(bytes_per_row);
            pixels[dst_start..dst_end].copy_from_slice(&data[src_start..src_end]);
        }

        Some(RasterizedGlyph {
            width: w,
            height: h,
            pixels,
        })
    }
}

pub struct RasterizedGlyph {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}
