use std::error::Error;
use std::io;
use std::sync::Arc;

use wgpu::CurrentSurfaceTexture;
use winit::dpi::PhysicalSize;
use winit::event_loop::OwnedDisplayHandle;
use winit::window::Window;

use crate::Frame;
use crate::menu::BAR_HEIGHT;

const MATTE_RGB: [f64; 3] = [0.070, 0.082, 0.102];
const INSET: f64 = 16.0;

pub(crate) struct Renderer {
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    framebuffer: Option<FramebufferTexture>,
    overlay: Option<FramebufferTexture>,
    menu_visible: bool,
    size: PhysicalSize<u32>,
}

impl Renderer {
    pub(crate) async fn new(
        window: Arc<Window>,
        display: OwnedDisplayHandle,
    ) -> Result<Self, Box<dyn Error>> {
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(display)),
        );
        let surface = instance.create_surface(Arc::clone(&window))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await?;
        let size = window.inner_size();
        let config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| io::Error::other("GPU adapter cannot present to the Dosa window"))?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Dosa framebuffer shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("frame.wgsl").into()),
        });
        let (pipeline, overlay_pipeline) = (
            pipeline(&device, &shader, config.format, false),
            pipeline(&device, &shader, config.format, true),
        );

        let mut renderer = Self {
            instance,
            surface,
            device,
            queue,
            config,
            pipeline,
            overlay_pipeline,
            framebuffer: None,
            overlay: None,
            menu_visible: false,
            size: PhysicalSize::new(0, 0),
        };
        renderer.resize(size);
        Ok(renderer)
    }

    pub(crate) fn resize(&mut self, size: PhysicalSize<u32>) {
        if size == self.size {
            return;
        }
        self.size = size;
        if size.width > 0 && size.height > 0 {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    pub(crate) fn draw(
        &mut self,
        window: &Arc<Window>,
        frame: Frame<'_>,
        aspect: Option<f32>,
        overlay: Option<Frame<'_>>,
    ) -> Result<(), Box<dyn Error>> {
        self.menu_visible = overlay.is_some();
        if self.size.width == 0 || self.size.height == 0 {
            return Ok(());
        }

        let (output, reconfigure) = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(output) => (output, false),
            CurrentSurfaceTexture::Suboptimal(output) => (output, true),
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => return Ok(()),
            CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            CurrentSurfaceTexture::Lost => {
                self.surface = self.instance.create_surface(Arc::clone(window))?;
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            CurrentSurfaceTexture::Validation => {
                return Err(io::Error::other("GPU surface validation failed").into());
            }
        };
        let srgb = self.config.format.is_srgb();
        upload(
            &self.device,
            &self.queue,
            &self.pipeline,
            &mut self.framebuffer,
            &frame,
            srgb,
        )?;
        let overlay_height = if let Some(frame) = overlay {
            upload(
                &self.device,
                &self.queue,
                &self.overlay_pipeline,
                &mut self.overlay,
                &frame,
                srgb,
            )?;
            (f64::from(frame.height) * window.scale_factor())
                .round()
                .clamp(1.0, f64::from(self.size.height)) as f32
        } else {
            0.0
        };

        let target = output.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(matte(self.config.format.is_srgb())),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            let [x, y, width, height] = self.display_viewport(window, &frame, aspect);
            pass.set_viewport(x, y, width, height, 0.0, 1.0);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.framebuffer.as_ref().unwrap().binding, &[]);
            pass.draw(0..3, 0..1);
            if overlay_height > 0.0 {
                pass.set_viewport(0.0, 0.0, self.size.width as f32, overlay_height, 0.0, 1.0);
                pass.set_pipeline(&self.overlay_pipeline);
                pass.set_bind_group(0, &self.overlay.as_ref().unwrap().binding, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
        window.pre_present_notify();
        self.queue.present(output);
        if reconfigure {
            self.surface.configure(&self.device, &self.config);
        }
        Ok(())
    }

    pub(crate) fn display_viewport(
        &self,
        window: &Window,
        frame: &Frame<'_>,
        aspect: Option<f32>,
    ) -> [f32; 4] {
        if self.size.width == 0 || self.size.height == 0 || frame.width == 0 || frame.height == 0 {
            return [0.0; 4];
        }
        let scale = window.scale_factor();
        let bar = if self.menu_visible {
            ((f64::from(BAR_HEIGHT) * scale).round() as u32).min(self.size.height.saturating_sub(1))
        } else {
            0
        };
        let mut viewport = viewport(
            PhysicalSize::new(self.size.width, self.size.height - bar),
            (frame.width, frame.height),
            aspect,
            scale * INSET,
        );
        viewport[1] += bar as f32;
        viewport
    }
}

fn pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    overlay: bool,
) -> wgpu::RenderPipeline {
    let target = wgpu::ColorTargetState {
        format,
        blend: overlay.then_some(wgpu::BlendState::ALPHA_BLENDING),
        write_mask: wgpu::ColorWrites::ALL,
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Dosa GPU presentation"),
        layout: None,
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vertex"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(if overlay {
                "overlay_fragment"
            } else {
                "fragment"
            }),
            compilation_options: Default::default(),
            targets: &[Some(target)],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::RenderPipeline,
    texture: &mut Option<FramebufferTexture>,
    frame: &Frame<'_>,
    srgb: bool,
) -> io::Result<()> {
    let extent = wgpu::Extent3d {
        width: frame.width,
        height: frame.height,
        depth_or_array_layers: 1,
    };
    let device_max = device.limits().max_texture_dimension_2d;
    let valid_size = |dimension| (1..=device_max).contains(&dimension);
    if !valid_size(extent.width)
        || !valid_size(extent.height)
        || (extent.width as usize) * (extent.height as usize) * 4 != frame.rgba.len()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid RGBA8 framebuffer dimensions or byte count",
        ));
    }

    if texture
        .as_ref()
        .is_none_or(|current| current.extent != extent)
    {
        *texture = Some(FramebufferTexture::new(device, pipeline, extent, srgb));
    }
    let source = &texture.as_ref().unwrap().texture;
    queue.write_texture(
        source.as_image_copy(),
        frame.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(extent.width * 4),
            rows_per_image: Some(extent.height),
        },
        extent,
    );
    Ok(())
}

struct FramebufferTexture {
    texture: wgpu::Texture,
    binding: wgpu::BindGroup,
    extent: wgpu::Extent3d,
}

impl FramebufferTexture {
    fn new(
        device: &wgpu::Device,
        pipeline: &wgpu::RenderPipeline,
        extent: wgpu::Extent3d,
        srgb_target: bool,
    ) -> Self {
        let format = if srgb_target {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("DOS framebuffer"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("DOS framebuffer binding"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });

        Self {
            texture,
            binding,
            extent,
        }
    }
}

fn viewport(
    surface: PhysicalSize<u32>,
    frame: (u32, u32),
    aspect: Option<f32>,
    inset: f64,
) -> [f32; 4] {
    let outer_width = f64::from(surface.width);
    let outer_height = f64::from(surface.height);
    let width = (outer_width - 2.0 * inset).max(1.0);
    let height = (outer_height - 2.0 * inset).max(1.0);

    let frame_height = f64::from(frame.1);
    let frame_width = match aspect.filter(|value| value.is_finite() && *value > 0.0) {
        Some(aspect) => frame_height * f64::from(aspect),
        None => f64::from(frame.0),
    };
    let scale = (width / frame_width).min(height / frame_height);
    let width = (frame_width * scale).round().clamp(1.0, outer_width);
    let height = (frame_height * scale).round().clamp(1.0, outer_height);
    [
        ((outer_width - width) * 0.5).floor() as f32,
        ((outer_height - height) * 0.5).floor() as f32,
        width as f32,
        height as f32,
    ]
}

fn matte(srgb_target: bool) -> wgpu::Color {
    let [r, g, b] = if srgb_target {
        MATTE_RGB.map(|value| ((value + 0.055) / 1.055).powf(2.4))
    } else {
        MATTE_RGB
    };
    wgpu::Color { r, g, b, a: 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_keeps_pixel_aspect_and_tracks_surface_resize() {
        assert_eq!(
            viewport(PhysicalSize::new(832, 632), (320, 200), None, 16.0),
            [16.0, 66.0, 800.0, 500.0]
        );
        assert_eq!(
            viewport(
                PhysicalSize::new(832, 632),
                (320, 200),
                Some(4.0 / 3.0),
                16.0
            ),
            [16.0, 16.0, 800.0, 600.0]
        );
        assert_eq!(
            viewport(PhysicalSize::new(1, 1), (320, 200), None, 32.0),
            [0.0, 0.0, 1.0, 1.0]
        );
    }
}
