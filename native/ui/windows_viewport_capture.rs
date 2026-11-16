//! Read the actual client frame on DirectX, including when a WARP swap chain
//! returns a desktop-sized texture or an immediate child has no screenshot event.
use eframe::{
    egui,
    egui_wgpu::{RenderState, ScreenDescriptor},
};
use std::{
    error::Error,
    sync::{Arc, mpsc},
    time::Duration,
};

#[derive(Default)]
pub struct ViewportFrame {
    requested: Option<[u32; 2]>,
    frame: Option<(ScreenDescriptor, Vec<egui::epaint::ClippedShape>)>,
}

impl egui::Plugin for ViewportFrame {
    fn debug_name(&self) -> &'static str {
        "DirectX viewport capture"
    }

    fn output_hook(&mut self, output: &mut egui::FullOutput) {
        if let Some(size_in_pixels) = self.requested.take() {
            self.frame = Some((
                ScreenDescriptor {
                    size_in_pixels,
                    pixels_per_point: output.pixels_per_point,
                },
                output.shapes.clone(),
            ));
        }
    }
}

impl ViewportFrame {
    pub fn request(ctx: &egui::Context) {
        let size = (ctx.viewport_rect().size() * ctx.pixels_per_point()).round();
        ctx.plugin_or_default::<Self>().lock().requested = Some([size.x as u32, size.y as u32]);
    }

    /// Read after eframe has uploaded this viewport's textures and finished its pass.
    pub fn read(
        ctx: &egui::Context,
        state: &RenderState,
    ) -> Result<Option<Arc<egui::ColorImage>>, Box<dyn Error>> {
        let Some(plugin) = ctx.plugin_opt::<Self>() else {
            return Ok(None);
        };
        let Some((screen, shapes)) = plugin.lock().frame.take() else {
            return Ok(None);
        };
        let [width, height] = screen.size_in_pixels;
        let bgra = match state.target_format {
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
            format => return Err(format!("Unsupported capture pixel format {format:?}").into()),
        };
        let device = &state.device;
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("QuotaBar viewport capture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: state.target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let row_bytes = width * 4;
        let stride = wgpu::util::align_to(row_bytes, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("QuotaBar viewport pixels"),
            size: u64::from(stride) * u64::from(height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        let jobs = ctx.tessellate(shapes, screen.pixels_per_point);
        let callbacks = state.renderer.write().update_buffers(
            device,
            &state.queue,
            &mut encoder,
            &jobs,
            &screen,
        );
        {
            // Keep the original draw order and textures. Set the projection and
            // scissor to the client even if the swap chain has a different size.
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            state.renderer.read().render(&mut pass, &jobs, &screen);
        }
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    bytes_per_row: Some(stride),
                    ..Default::default()
                },
            },
            extent,
        );
        let submit = state
            .queue
            .submit(callbacks.into_iter().chain([encoder.finish()]));
        let (send, receive) = mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = send.send(result);
            });
        device.poll(wgpu::PollType::Wait {
            submission_index: Some(submit),
            timeout: Some(Duration::from_secs(8)),
        })?;
        receive.recv_timeout(Duration::from_secs(1))??;
        let bytes = buffer.slice(..).get_mapped_range();
        let mut pixels = Vec::with_capacity(width as usize * height as usize);
        for row in bytes.chunks_exact(stride as usize) {
            for rgba in row[..row_bytes as usize].as_chunks::<4>().0 {
                let (red, blue) = if bgra {
                    (rgba[2], rgba[0])
                } else {
                    (rgba[0], rgba[2])
                };
                pixels.push(egui::Color32::from_rgba_premultiplied(
                    red, rgba[1], blue, rgba[3],
                ));
            }
        }
        drop(bytes);
        buffer.unmap();
        Ok(Some(Arc::new(egui::ColorImage::new(
            [width as usize, height as usize],
            pixels,
        ))))
    }
}
