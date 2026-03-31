use anyhow::{Result, Context};
use metal::*;

/// Holds the compiled Metal render pipeline state objects.
pub struct RenderPipelines {
    pub bg_pipeline: RenderPipelineState,
    pub glyph_pipeline: RenderPipelineState,
}

/// Uniform buffer layout matching the shader.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Uniforms {
    pub viewport_size: [f32; 2],
    pub cell_size: [f32; 2],
    pub grid_offset: [f32; 2],
}

/// Per-instance cell data matching the Metal shader's packed struct layout.
/// The Metal shader uses packed_floatN types to avoid alignment padding,
/// so this #[repr(C)] struct maps 1:1 to the GPU struct.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CellInstance {
    pub grid_pos: [f32; 2],   // 8 bytes,  offset 0
    pub bg_color: [f32; 4],   // 16 bytes, offset 8
    pub fg_color: [f32; 4],   // 16 bytes, offset 24
    pub glyph_uv: [f32; 4],   // 16 bytes, offset 40
    pub flags: u32,            // 4 bytes,  offset 56
    pub _padding: [u32; 3],    // 12 bytes, offset 60
}
// Compile-time check: must match Metal's CellInstance (72 bytes).
const _: () = assert!(std::mem::size_of::<CellInstance>() == 72);

impl RenderPipelines {
    pub fn new(device: &DeviceRef) -> Result<Self> {
        let shader_source = include_str!("../../../resources/shaders/terminal.metal");
        let library = device
            .new_library_with_source(shader_source, &CompileOptions::new())
            .map_err(|e| anyhow::anyhow!("Failed to compile Metal shaders: {}", e))?;

        let bg_pipeline = Self::create_pipeline(
            device,
            &library,
            "bg_vertex",
            "bg_fragment",
            false,
        )?;

        let glyph_pipeline = Self::create_pipeline(
            device,
            &library,
            "glyph_vertex",
            "glyph_fragment",
            true, // Enable blending for glyphs.
        )?;

        Ok(Self { bg_pipeline, glyph_pipeline })
    }

    fn create_pipeline(
        device: &DeviceRef,
        library: &LibraryRef,
        vertex_fn: &str,
        fragment_fn: &str,
        blend: bool,
    ) -> Result<RenderPipelineState> {
        let vert = library
            .get_function(vertex_fn, None)
            .map_err(|e| anyhow::anyhow!("Missing vertex function '{}': {}", vertex_fn, e))?;
        let frag = library
            .get_function(fragment_fn, None)
            .map_err(|e| anyhow::anyhow!("Missing fragment function '{}': {}", fragment_fn, e))?;

        let desc = RenderPipelineDescriptor::new();
        desc.set_vertex_function(Some(&vert));
        desc.set_fragment_function(Some(&frag));

        let attachment = desc
            .color_attachments()
            .object_at(0)
            .context("No color attachment")?;
        attachment.set_pixel_format(MTLPixelFormat::BGRA8Unorm);

        if blend {
            attachment.set_blending_enabled(true);
            attachment.set_rgb_blend_operation(MTLBlendOperation::Add);
            attachment.set_alpha_blend_operation(MTLBlendOperation::Add);
            attachment.set_source_rgb_blend_factor(MTLBlendFactor::SourceAlpha);
            attachment.set_destination_rgb_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
            attachment.set_source_alpha_blend_factor(MTLBlendFactor::One);
            attachment.set_destination_alpha_blend_factor(MTLBlendFactor::OneMinusSourceAlpha);
        }

        device
            .new_render_pipeline_state(&desc)
            .map_err(|e| anyhow::anyhow!("Failed to create pipeline '{}': {}", vertex_fn, e))
    }
}
