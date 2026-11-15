use eframe::egui::{self, epaint::TextShape, *};
use std::collections::HashMap;
use windows::{
    Win32::{
        Graphics::{
            Direct2D::{Common::*, *},
            DirectWrite::*,
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Imaging::*,
        },
        System::Com::*,
    },
    core::{Result, w},
};

struct Factories {
    write: IDWriteFactory,
    line_metrics: [Option<DWRITE_FONT_METRICS>; 2],
    coverage: IDWriteRenderingParams,
    display: IDWriteRenderingParams,
    contrast: [u8; 256],
    draw: ID2D1Factory,
    images: IWICImagingFactory,
}

impl Factories {
    fn line_box(&self, size: f32, semibold: bool) -> Option<(f32, f32)> {
        let m = self.line_metrics[usize::from(semibold)]?;
        let unit = size / f32::from(m.designUnitsPerEm);
        let height = f32::from(m.ascent) + f32::from(m.descent) + f32::from(m.lineGap);
        Some((height * unit, f32::from(m.ascent) * unit))
    }
    fn line_metrics(write: &IDWriteFactory) -> [Option<DWRITE_FONT_METRICS>; 2] {
        let family = unsafe {
            let mut collection = None;
            let mut index = 0;
            let mut exists = Default::default();
            write
                .GetSystemFontCollection(&mut collection, false)
                .ok()
                .and(collection)
                .and_then(|c| {
                    c.FindFamilyName(w!("Segoe UI"), &mut index, &mut exists)
                        .ok()?;
                    exists
                        .as_bool()
                        .then(|| c.GetFontFamily(index).ok())
                        .flatten()
                })
        };
        [DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD].map(|weight| unsafe {
            let face = family
                .as_ref()?
                .GetFirstMatchingFont(weight, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL)
                .ok()?;
            let mut metrics = DWRITE_FONT_METRICS::default();
            face.GetMetrics(&mut metrics);
            (metrics.designUnitsPerEm > 0).then_some(metrics)
        })
    }
    fn new() -> Result<Self> {
        // The windowing backend owns the UI thread's COM apartment.
        unsafe {
            let write: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let coverage = write.CreateCustomRenderingParams(
                1.,
                0.,
                1.,
                DWRITE_PIXEL_GEOMETRY_RGB,
                DWRITE_RENDERING_MODE_DEFAULT,
            )?;
            // WPF applies the display's enhanced contrast to the glyph mask,
            // before compositing the foreground against the current surface.
            let display = write.CreateRenderingParams()?;
            let contrast = display.GetEnhancedContrast();
            let contrast = std::array::from_fn(|i| {
                let alpha = i as f32 / 255.;
                (255. * alpha * (1. + contrast) / (1. + alpha * contrast)).round() as u8
            });
            Ok(Self {
                line_metrics: Self::line_metrics(&write),
                write,
                coverage,
                display,
                contrast,
                draw: D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
                images: CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?,
            })
        }
    }

    fn render(&self, key: &RunKey, max_size: usize) -> Option<(ColorImage, Vec2)> {
        let size = f32::from_bits(key.pixels);
        let weight = if key.semibold {
            DWRITE_FONT_WEIGHT_SEMI_BOLD
        } else {
            DWRITE_FONT_WEIGHT_NORMAL
        };
        // Rasterize in physical pixels. The UI applies the window's DIP scale when
        // it places the finished run. DirectWrite handles Korean and mixed-script fallback.
        unsafe {
            let format = self
                .write
                .CreateTextFormat(
                    w!("Segoe UI"),
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("ko-KR"),
                )
                .ok()?;
            // WPF's TextBlock uses the family baseline and line spacing across
            // fallback runs. DirectWrite's default combines the largest ascent and
            // descent from different faces, making mixed Korean/Latin rows taller.
            if let Some((height, baseline)) = self.line_box(size, key.semibold) {
                format
                    .SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, height, baseline)
                    .ok()?;
            }
            let wrap_width = key.wrap_width.map(f32::from_bits);
            format
                .SetWordWrapping(if wrap_width.is_some() {
                    DWRITE_WORD_WRAPPING_WRAP
                } else {
                    DWRITE_WORD_WRAPPING_NO_WRAP
                })
                .ok()?;
            let utf16: Vec<_> = key.text.encode_utf16().collect();
            let layout = self
                .write
                .CreateTextLayout(
                    &utf16,
                    &format,
                    wrap_width.unwrap_or(max_size as f32),
                    max_size as f32,
                )
                .ok()?;
            let mut metrics = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut metrics).ok()?;
            let width = metrics.widthIncludingTrailingWhitespace.ceil() as usize + 3;
            let height = metrics.height.ceil() as usize + 2;
            if width > max_size || height > max_size || width.checked_mul(height)? > 1_048_576 {
                return None;
            }
            let bitmap = self
                .images
                .CreateBitmap(
                    width as u32,
                    height as u32,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapCacheOnLoad,
                )
                .ok()?;
            let target = self
                .draw
                .CreateWicBitmapRenderTarget(
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
                )
                .ok()?;
            let brush_color = |color: Color32| D2D1_COLOR_F {
                r: f32::from(color.r()) / 255.,
                g: f32::from(color.g()) / 255.,
                b: f32::from(color.b()) / 255.,
                a: 1.,
            };
            let (ink, backdrop) = key.colors.unwrap_or((Color32::WHITE, Color32::BLACK));
            let brush = target.CreateSolidColorBrush(&brush_color(ink), None).ok()?;
            target.BeginDraw();
            target.Clear(Some(&brush_color(backdrop)));
            target.SetTextRenderingParams(if key.colors.is_some() {
                &self.display
            } else {
                &self.coverage
            });
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
            target.DrawTextLayout(
                windows_numerics::Vector2 {
                    X: f32::from(key.origin[0]) / 64.,
                    Y: f32::from(key.origin[1]) / 64.,
                },
                &layout,
                &brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
            target.EndDraw(None, None).ok()?;
            let mut bytes = vec![0; width * height * 4];
            bitmap
                .CopyPixels(std::ptr::null(), (width * 4) as u32, &mut bytes)
                .ok()?;
            let mask = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|px| {
                    if key.colors.is_some() {
                        let pixel = Color32::from_rgb(px[2], px[1], px[0]);
                        if pixel == backdrop {
                            Color32::TRANSPARENT
                        } else {
                            pixel
                        }
                    } else {
                        // For an alpha bitmap WPF samples the center of the coverage texture.
                        Color32::from_white_alpha(self.contrast[px[1] as usize])
                    }
                })
                .collect();
            Some((
                ColorImage::new([width, height], mask),
                vec2(metrics.widthIncludingTrailingWhitespace, metrics.height),
            ))
        }
    }
}

thread_local! {
    static FACTORIES: Option<Factories> = Factories::new().ok();
}
pub fn line_height(size: f32) -> Option<f32> {
    FACTORIES.with(|f| Some(f.as_ref()?.line_box(size, false)?.0))
}

#[derive(Clone, Eq, PartialEq, Hash)]
struct RunKey {
    text: String,
    pixels: u32,
    semibold: bool,
    wrap_width: Option<u32>,
    origin: [u8; 2],
    colors: Option<(Color32, Color32)>,
}

/// DirectWrite snaps each baseline in device space. Preserve the fraction of
/// the layout origin when rasterizing the run into its own pixel-aligned bitmap.
fn raster_origin(position: Pos2, scale: f32) -> (Pos2, [u8; 2]) {
    let units = [position.x, position.y].map(|p| (p * scale * 64.).round() as i32);
    let [x, y] = units.map(|v| v.div_euclid(64) as f32 / scale);
    (pos2(x, y), units.map(|v| v.rem_euclid(64) as u8))
}

struct CachedRun {
    texture: TextureHandle,
    layout_size: Vec2,
    last_pass: u64,
}

#[derive(Default)]
pub struct TextRenderer {
    runs: HashMap<RunKey, CachedRun>,
    pass: u64,
    pub bitmap_target: bool,
}

impl TextRenderer {
    fn run(&mut self, ctx: &Context, key: RunKey, max_size: usize) -> Option<&CachedRun> {
        let run = match self.runs.entry(key) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let (image, layout_size) =
                    FACTORIES.with(|f| f.as_ref()?.render(e.key(), max_size))?;
                e.insert(CachedRun {
                    texture: ctx.load_texture("DirectWrite text", image, TextureOptions::NEAREST),
                    layout_size,
                    last_pass: self.pass,
                })
            }
        };
        run.last_pass = self.pass;
        Some(run)
    }

    fn paint(
        &mut self,
        ctx: &Context,
        text: &TextShape,
        scale: f32,
        max_size: usize,
        backdrop: Option<Color32>,
    ) -> Option<Shape> {
        let [section] = text.galley.job.sections.as_slice() else {
            return None;
        };
        let format = &section.format;
        let semibold = match &format.font_id.family {
            FontFamily::Proportional => false,
            FontFamily::Name(name) if name.as_ref() == "system-semibold" => true,
            _ => return None,
        };
        // Preserve the UI renderer's selection backgrounds, IME underlines,
        // rotated and rich text, all of which include geometry beyond a glyph run.
        if text.angle != 0.
            || format.italics
            || !text.underline.is_empty()
            || format.extra_letter_spacing != 0.
            || text.galley.rows.iter().any(|row| {
                let visual = &row.visuals;
                visual.glyph_vertex_range != (0..visual.mesh.vertices.len())
            })
        {
            return None;
        }
        let mut color = text.override_text_color.unwrap_or(format.color);
        if color == Color32::PLACEHOLDER {
            color = text.fallback_color;
        }
        color = color.gamma_multiply(text.opacity_factor);
        let colors = backdrop.filter(|_| color.a() == 255).map(|bg| (color, bg));
        let mut shapes = Vec::with_capacity(text.galley.rows.len());
        for row in &text.galley.rows {
            if row.glyphs.is_empty() {
                continue;
            }
            let (start, origin) = raster_origin(text.pos + row.pos.to_vec2(), scale);
            let key = RunKey {
                text: row.glyphs.iter().map(|glyph| glyph.chr).collect(),
                pixels: (format.font_id.size * scale).to_bits(),
                semibold,
                wrap_width: None,
                origin,
                colors,
            };
            let entry = self.run(ctx, key, max_size)?;
            let rect = Rect::from_min_size(start, entry.texture.size_vec2() / scale);
            shapes.push(Shape::image(
                entry.texture.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                if colors.is_some() {
                    Color32::WHITE
                } else {
                    color
                },
            ));
        }
        Some(Shape::Vec(shapes))
    }
}

/// Use the same system font metrics for wrapping, drawing and sizing a Windows TextBlock.
pub fn text_block(ui: &mut Ui, text: &str, size: f32, semibold: bool, color: Color32) -> Response {
    draw_label(ui, text, size, semibold, color, true)
        .unwrap_or_else(|| ui.label(crate::style::rich(text, size, semibold, color)))
}

pub fn natural_size(ui: &Ui, text: &str, size: f32, semibold: bool) -> Option<Vec2> {
    let scale = ui.ctx().pixels_per_point();
    let key = RunKey {
        text: text.into(),
        pixels: (size * scale).to_bits(),
        semibold,
        wrap_width: None,
        origin: [0; 2],
        colors: None,
    };
    let renderer = ui.ctx().plugin_opt::<TextRenderer>()?;
    let mut renderer = renderer.lock();
    Some(
        renderer
            .run(ui.ctx(), key, ui.input(|i| i.max_texture_side))?
            .layout_size
            / scale,
    )
}

pub fn draw_label(
    ui: &mut Ui,
    text: &str,
    size: f32,
    semibold: bool,
    color: Color32,
    fill_width: bool,
) -> Option<Response> {
    let width = ui.available_width();
    let scale = ui.ctx().pixels_per_point();
    let max_size = ui.input(|i| i.max_texture_side);
    if !width.is_finite() || width <= 0. || ui.layout().main_wrap() {
        return None;
    }
    let wrapped = natural_size(ui, text, size, semibold).is_none_or(|v| v.x > width);
    let mut key = RunKey {
        text: text.into(),
        pixels: (size * scale).to_bits(),
        semibold,
        wrap_width: wrapped.then(|| (width * scale).to_bits()),
        origin: [0; 2],
        colors: None,
    };
    let renderer = ui.ctx().plugin_opt::<TextRenderer>()?;
    let mut renderer = renderer.lock();
    let measured = renderer.run(ui.ctx(), key.clone(), max_size)?.layout_size;
    // WPF measures TextBlocks before arranging their parent. Retain the
    // fractional natural size across stacked rows, independently of the bitmap.
    let mut desired = measured / scale;
    desired.x = if fill_width {
        width
    } else {
        desired.x.min(width)
    };
    let (rect, response) = ui.allocate_exact_size(desired, Sense::hover());
    let (start, origin) = raster_origin(rect.min, scale);
    key.origin = origin;
    let run = renderer.run(ui.ctx(), key, max_size)?;
    ui.painter().image(
        run.texture.id(),
        Rect::from_min_size(start, run.texture.size_vec2() / scale),
        Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
        color,
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), text));
    Some(response)
}

struct Surface {
    bounds: Rect,
    interior: Rect,
    radius: f32,
    color: Option<Color32>,
}

impl Surface {
    fn behind(surfaces: &[Self], area: Rect) -> Option<Color32> {
        let surface = surfaces.iter().rev().find(|s| s.bounds.intersects(area))?;
        if !surface.interior.contains_rect(area) {
            return None;
        }
        let middle_x = surface.interior.shrink2(vec2(surface.radius, 0.));
        let middle_y = surface.interior.shrink2(vec2(0., surface.radius));
        (middle_x.contains_rect(area) || middle_y.contains_rect(area))
            .then_some(surface.color)
            .flatten()
    }

    fn record(surfaces: &mut Vec<Self>, shape: &Shape, clip: Rect) {
        if let Shape::Vec(shapes) = shape {
            for shape in shapes {
                Self::record(surfaces, shape, clip);
            }
            return;
        }
        let bounds = shape.visual_bounding_rect().intersect(clip);
        if !bounds.is_positive() {
            return;
        }
        let mut surface = Self {
            bounds,
            interior: Rect::NOTHING,
            radius: 0.,
            color: None,
        };
        if let Shape::Rect(r) = shape
            && r.fill.a() == 255
            && r.brush.is_none()
            && r.blur_width == 0.
        {
            surface.interior = r.rect.shrink(r.stroke.width).intersect(clip);
            let corners = r.corner_radius;
            surface.radius = f32::from(corners.nw.max(corners.ne).max(corners.sw).max(corners.se));
            surface.color = Some(r.fill);
        }
        surfaces.push(surface);
    }
}

fn system_cleartype() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let setting = |key| {
        let mut value = 0u32;
        unsafe { SystemParametersInfoW(key, 0, (&raw mut value).cast(), 0) };
        value
    };
    setting(SPI_GETFONTSMOOTHING) != 0
        && setting(SPI_GETFONTSMOOTHINGTYPE) == FE_FONTSMOOTHINGCLEARTYPE
}

impl egui::Plugin for TextRenderer {
    fn debug_name(&self) -> &'static str {
        "DirectWrite"
    }

    fn on_end_pass(&mut self, ctx: &Context) {
        self.pass += 1;
        self.runs.retain(|_, run| self.pass - run.last_pass <= 3);
        let scale = ctx.pixels_per_point();
        let max_size = ctx.input(|i| i.max_texture_side);
        // ClearType requires a known, opaque screen background. The app's
        // bitmap export and glyphs over images retain their grayscale mask.
        let lcd = !self.bitmap_target && system_cleartype();
        let mut layers = ctx.memory(|m| m.layer_ids().collect::<Vec<_>>());
        if !layers.contains(&LayerId::background()) {
            layers.push(LayerId::background());
        }
        for layer in layers {
            enum Label {
                Text(TextShape, Option<Color32>),
                Mask(std::sync::Arc<Mesh>, RunKey),
            }
            let runs = ctx.graphics(|g| {
                let mut runs = vec![];
                let mut surfaces = vec![];
                let masks: HashMap<_, _> = if lcd {
                    self.runs
                        .iter()
                        .filter(|(key, _)| key.colors.is_none())
                        .map(|(key, run)| (run.texture.id(), key))
                        .collect()
                } else {
                    HashMap::new()
                };
                if let Some(list) = g.get(layer) {
                    for (index, entry) in list.all_entries().enumerate() {
                        let area = entry
                            .shape
                            .visual_bounding_rect()
                            .intersect(entry.clip_rect);
                        if !area.is_positive() {
                            continue;
                        }
                        if let Shape::Text(text) = &entry.shape {
                            let backdrop = Surface::behind(&surfaces, area);
                            runs.push((index, Label::Text(text.clone(), backdrop)));
                        } else if let Shape::Mesh(mesh) = &entry.shape
                            && let Some(&key) = masks.get(&mesh.texture_id)
                            && let Some(backdrop) = Surface::behind(&surfaces, area)
                            && let Some(first) = mesh.vertices.first()
                            && first.color.a() == 255
                            && mesh.vertices.iter().all(|v| v.color == first.color)
                        {
                            let mut colored = key.clone();
                            colored.colors = Some((first.color, backdrop));
                            runs.push((index, Label::Mask(mesh.clone(), colored)));
                        }
                        if lcd {
                            Surface::record(&mut surfaces, &entry.shape, entry.clip_rect);
                        }
                    }
                }
                runs
            });
            for (index, run) in runs {
                let shape = match run {
                    Label::Text(text, backdrop) => {
                        self.paint(ctx, &text, scale, max_size, backdrop)
                    }
                    Label::Mask(mut mesh, key) => self.run(ctx, key, max_size).map(|run| {
                        let colored = std::sync::Arc::make_mut(&mut mesh);
                        colored.texture_id = run.texture.id();
                        for vertex in &mut colored.vertices {
                            vertex.color = Color32::WHITE;
                        }
                        Shape::Mesh(mesh)
                    }),
                };
                if let Some(shape) = shape {
                    ctx.graphics_mut(|g| {
                        g.entry(layer)
                            .mutate_shape(layers::ShapeIdx(index), |entry| entry.shape = shape)
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_script_text_uses_the_system_rasterizer_at_both_scales() {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .unwrap();
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe { CoUninitialize() };
            }
        }
        let _apartment = Apartment;
        let factories = Factories::new().unwrap();
        let mut previous_width = 0;
        for pixels in [13.0_f32, 26.0] {
            let run = RunKey {
                text: "Codex · 계정 연결".into(),
                pixels: pixels.to_bits(),
                semibold: false,
                wrap_width: None,
                origin: [0; 2],
                colors: None,
            };
            let (mask, size) = factories.render(&run, 4096).unwrap();
            let latin = RunKey {
                text: "Codex".into(),
                ..run
            };
            let (_, latin_size) = factories.render(&latin, 4096).unwrap();
            assert_eq!(size.y, latin_size.y);
            assert!(mask.width() > previous_width);
            assert!(mask.pixels.iter().any(|p| p.a() == 0));
            assert!(mask.pixels.iter().any(|p| (1..255).contains(&p.a())));
            let narrow = RunKey {
                wrap_width: Some((3. * pixels).to_bits()),
                ..run
            };
            let (wrapped, wrapped_size) = factories.render(&narrow, 4096).unwrap();
            assert!(wrapped.width() < mask.width() && wrapped_size.y > size.y);
            previous_width = mask.width();
        }
    }
}
