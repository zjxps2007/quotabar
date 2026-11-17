use super::PROVIDER_MARKS;
use eframe::egui::{ColorImage, Id, Rect, TextureHandle, TextureOptions, Ui};
use std::{cell::RefCell, collections::HashMap};
use windows::{
    Win32::{
        Graphics::{
            Direct2D::{Common::*, *},
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Imaging::*,
        },
        System::Com::*,
    },
    core::{Interface, Result},
};

#[derive(Default)]
pub(super) struct Cache(RefCell<HashMap<(usize, u32), TextureHandle>>);

impl Cache {
    pub(super) fn texture(
        &self,
        ui: &Ui,
        index: usize,
        original: &TextureHandle,
        width: f32,
    ) -> TextureHandle {
        let (name, png, _) = PROVIDER_MARKS[index];
        let source = original.size()[0] as u32;
        let size = prefilter_size(source, width * ui.pixels_per_point());
        if size == source {
            return original.clone();
        }
        self.0
            .borrow_mut()
            .entry((index, size))
            .or_insert_with(|| match area_sample(png, size) {
                Ok(image) => {
                    ui.ctx()
                        .load_texture(format!("{name}-{size}"), image, TextureOptions::LINEAR)
                }
                Err(_) => original.clone(),
            })
            .clone()
    }
}

pub(super) fn claude_vector(ui: &Ui, id: Id, artwork: Rect) -> Option<(TextureHandle, Rect)> {
    cached_vector(
        ui,
        id.with("Claude SVG"),
        artwork,
        24.,
        include_bytes!("../../../shared/assets/ClaudeLogo.svg"),
    )
}

pub(super) fn glyph_vector(ui: &Ui, name: &str, bounds: Rect) -> Option<(TextureHandle, Rect)> {
    // WPF places the stroked path in a 20-by-20 Viewbox. Its line caps are
    // flat, the joins are round, and all six symbols share the same stroke.
    macro_rules! glyph {
        ($path:literal) => {
            concat!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20">"#,
                r##"<path fill="none" stroke="#fff" stroke-width="1.3" stroke-linejoin="round" d=""##,
                $path,
                r#""/></svg>"#
            ).as_bytes()
        }
    }
    let data = match name {
        "dashboard" => glyph!("M2 2H8V8H2Z M12 2H18V8H12Z M2 12H8V18H2Z M12 12H18V18H12Z"),
        "accounts" => glyph!(
            "M10 7A3 3 0 1 0 4 7A3 3 0 1 0 10 7 M1 18V16C1 10 13 10 13 16V18 M14 4A3 3 0 0 1 14 10 M15 13C19 13 19 15 19 18"
        ),
        "settings" => glyph!(
            "M2 5H7 M11 5H18 M2 10H12 M16 10H18 M2 15H4 M8 15H18 M7 3H11V7H7Z M12 8H16V12H12Z M4 13H8V17H4Z"
        ),
        "attention" => glyph!("M10 2A8 8 0 1 1 9.99 2 M10 6V11 M10 14V15"),
        "bolt" => glyph!("M11 1L4 11H9L8 19L16 8H11Z"),
        _ => glyph!("M3 18V11H6V18 M9 18V6H12V18 M15 18V2H18V18"),
    };
    cached_vector(ui, ui.id().with(("Windows glyph", name)), bounds, 20., data)
}

fn cached_vector(
    ui: &Ui,
    id: Id,
    artwork: Rect,
    side: f32,
    data: &'static [u8],
) -> Option<(TextureHandle, Rect)> {
    let scale = ui.pixels_per_point();
    let device = artwork * scale;
    let bounds = Rect::from_min_max(device.min.floor(), device.max.ceil());
    let local = device.translate(-bounds.min.to_vec2());
    // Keep the fractional placement in the drawing. Snapping only the finished
    // texture stops the GPU from filtering the same vector edges a second time.
    let cached = ui.data(|data| data.get_temp::<(Rect, Option<TextureHandle>)>(id));
    let texture = match cached {
        Some((previous, texture)) if previous == local => texture,
        _ => {
            let image = render_vector(local, side, data).ok();
            let texture = image.map(|image| {
                ui.ctx()
                    .load_texture("Windows vector artwork", image, TextureOptions::NEAREST)
            });
            ui.data_mut(|data| data.insert_temp(id, (local, texture.clone())));
            texture
        }
    }?;
    Some((texture, bounds / scale))
}

fn render_vector(artwork: Rect, side: f32, data: &'static [u8]) -> Result<ColorImage> {
    let (width, height) = (
        artwork.right().ceil() as u32,
        artwork.bottom().ceil() as u32,
    );
    unsafe {
        let images: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let draw: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let bitmap = images.CreateBitmap(
            width,
            height,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapCacheOnLoad,
        )?;
        let target = draw.CreateWicBitmapRenderTarget(
            &bitmap,
            &D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.,
                dpiY: 96.,
                ..Default::default()
            },
        )?;
        let context: ID2D1DeviceContext5 = target.cast()?;
        let stream = images.CreateStream()?;
        stream.InitializeFromMemory(data)?;
        let viewport = D2D_SIZE_F {
            width: side,
            height: side,
        };
        let svg = context.CreateSvgDocument(&stream, viewport)?;
        context.SetTransform(&windows_numerics::Matrix3x2 {
            M11: artwork.width() / viewport.width,
            M22: artwork.height() / viewport.height,
            M31: artwork.left(),
            M32: artwork.top(),
            ..Default::default()
        });
        context.BeginDraw();
        context.Clear(None);
        context.DrawSvgDocument(&svg);
        context.EndDraw(None, None)?;
        color_image(&bitmap, width, height)
    }
}

// WPF's high-quality bitmap brush first area-samples into half-octave size
// intervals, then reconstructs at the actual device-space position with bilinear filtering.
fn prefilter_size(source: u32, target: f32) -> u32 {
    let threshold = std::f32::consts::FRAC_1_SQRT_2;
    if target <= 1. {
        return 1;
    }
    let scale = target / source as f32;
    if scale > threshold {
        return source;
    }
    let exponent = (scale.ln() / threshold.ln()).floor() as i32;
    (source as f32 * threshold.powi(exponent)).ceil() as u32
}

fn area_sample(png: &'static [u8], size: u32) -> windows::core::Result<ColorImage> {
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = factory.CreateStream()?;
        stream.InitializeFromMemory(png)?;
        let decoder = factory.CreateDecoderFromStream(
            &stream,
            std::ptr::null(),
            WICDecodeMetadataCacheOnLoad,
        )?;
        let frame = decoder.GetFrame(0)?;
        let pixels = WICConvertBitmapSource(&GUID_WICPixelFormat32bppPBGRA, &frame)?;
        let scaler = factory.CreateBitmapScaler()?;
        scaler.Initialize(&pixels, size, size, WICBitmapInterpolationModeFant)?;
        color_image(&scaler, size, size)
    }
}

fn color_image(source: &IWICBitmapSource, width: u32, height: u32) -> Result<ColorImage> {
    let mut rgba = vec![0; width as usize * height as usize * 4];
    unsafe { source.CopyPixels(std::ptr::null(), width * 4, &mut rgba)? };
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok(ColorImage::from_rgba_premultiplied(
        [width as usize, height as usize],
        &rgba,
    ))
}

#[cfg(test)]
mod tests {
    use super::prefilter_size;

    #[test]
    fn svg_uses_the_device_position_before_rasterization() {
        use eframe::egui::{Color32, Rect, pos2, vec2};
        use windows::Win32::System::Com::{
            COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
        };

        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .unwrap();
        let first = Rect::from_min_size(pos2(0.25, 0.25), vec2(27., 27.));
        let images = [first, first.translate(vec2(0., 0.5))].map(|r| {
            super::render_vector(
                r,
                24.,
                include_bytes!("../../../shared/assets/ClaudeLogo.svg"),
            )
        });
        unsafe { CoUninitialize() };
        let [a, b] = images.map(Result::unwrap);
        assert_eq!(a.size, [28, 28]);
        assert_eq!(b.size, a.size);
        assert!(a.pixels.contains(&Color32::from_rgb(217, 119, 87)));
        assert!(a.pixels.contains(&Color32::TRANSPARENT));
        assert_ne!(a.pixels, b.pixels);
    }

    #[test]
    fn bitmap_prefilter_uses_physical_size_at_each_display_scale() {
        for (points, dpi_scale, expected) in [
            (26., 1., 32),
            (36., 1., 46),
            (44., 1., 46),
            (36., 1.25, 46),
            (36., 1.5, 64),
            (36., 1.75, 64),
            (36., 2., 91),
            (256., 2., 256),
            (0.5, 1., 1),
        ] {
            assert_eq!(prefilter_size(256, points * dpi_scale), expected);
        }
    }
}
