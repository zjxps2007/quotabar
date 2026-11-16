use super::PROVIDER_MARKS;
use eframe::egui::{ColorImage, TextureHandle, TextureOptions, Ui};
use std::{cell::RefCell, collections::HashMap};
use windows::Win32::{Graphics::Imaging::*, System::Com::*};

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
        // The WPF Claude mark is a vector drawing. Codex and Gemini are bitmaps.
        if name == "Claude" {
            return original.clone();
        }
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
        let mut rgba = vec![0; (size * size * 4) as usize];
        scaler.CopyPixels(std::ptr::null(), size * 4, &mut rgba)?;
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        Ok(ColorImage::from_rgba_premultiplied(
            [size as usize; 2],
            &rgba,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::prefilter_size;

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
