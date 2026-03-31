use anyhow::Result;
use core_graphics::base::kCGImageAlphaPremultipliedLast;
use core_graphics::color_space::CGColorSpace;
use core_graphics::context::CGContext;
use core_graphics::geometry::{CGPoint, CGSize};
use core_text::font as ct_font;
use core_text::font::CTFont;
use core_text::font_descriptor::kCTFontOrientationDefault;

/// A loaded monospace font with metrics.
pub struct FontInfo {
    pub font: CTFont,
    pub cell_width: f64,
    pub cell_height: f64,
    pub descent: f64,
    pub leading: f64,
}

impl FontInfo {
    /// Load a monospace font by name and size.
    pub fn load(font_name: &str, size: f64) -> Result<Self> {
        let font = ct_font::new_from_name(font_name, size)
            .map_err(|_| anyhow::anyhow!("Failed to load font: {}", font_name))?;

        let ascent = font.ascent();
        let descent = font.descent();
        let leading = font.leading();
        let cell_height = (ascent + descent + leading).ceil();

        // Get the width of 'M' as cell width for monospace.
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

        Ok(Self {
            font,
            cell_width,
            cell_height,
            descent,
            leading,
        })
    }

    /// Rasterize a single glyph to an RGBA bitmap. Returns (width, height, pixel_data).
    pub fn rasterize_glyph(&self, ch: char) -> Option<(u32, u32, Vec<u8>)> {
        let mut glyphs = [0u16; 1];
        let chars = [ch as u16];
        unsafe {
            self.font.get_glyphs_for_characters(chars.as_ptr(), glyphs.as_mut_ptr(), 1);
        }

        if glyphs[0] == 0 && ch != '\0' {
            return None; // Glyph not found in this font.
        }

        let w = self.cell_width.ceil() as u32;
        let h = self.cell_height.ceil() as u32;

        if w == 0 || h == 0 {
            return None;
        }

        let color_space = CGColorSpace::create_device_rgb();
        let mut ctx = CGContext::create_bitmap_context(
            None,
            w as usize,
            h as usize,
            8,
            (w as usize) * 4,
            &color_space,
            kCGImageAlphaPremultipliedLast,
        );

        // Clear to transparent.
        ctx.clear_rect(core_graphics::geometry::CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(w as f64, h as f64),
        ));

        // Draw white text (we'll tint with the terminal color in the shader).
        ctx.set_rgb_fill_color(1.0, 1.0, 1.0, 1.0);

        // Position: baseline is at descent from the bottom.
        let origin = CGPoint::new(0.0, self.descent);

        // Use the safe draw_glyphs API.
        self.font.draw_glyphs(&glyphs, &[origin], ctx.clone());

        let data = ctx.data();
        let pixels = data.to_vec();

        Some((w, h, pixels))
    }
}
