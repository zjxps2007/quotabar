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
mod glyphs;

struct Factories {
    write: IDWriteFactory,
    line_metrics: [Option<DWRITE_FONT_METRICS>; 2],
    coverage: IDWriteRenderingParams,
    lcd_coverage: IDWriteRenderingParams,
    gamma: [f32; 4],
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
            // Get channel coverage before Direct2D applies its own foreground
            // and background blending. WPF composes these samples itself.
            let lcd_coverage = write.CreateCustomRenderingParams(
                1.,
                0.,
                display.GetClearTypeLevel(),
                display.GetPixelGeometry(),
                display.GetRenderingMode(),
            )?;
            let gamma = glyph_gamma(display.GetGamma());
            let contrast = display.GetEnhancedContrast();
            let contrast = std::array::from_fn(|i| {
                let alpha = i as f32 / 255.;
                (255. * alpha * (1. + contrast) / (1. + alpha * contrast)).round() as u8
            });
            Ok(Self {
                line_metrics: Self::line_metrics(&write),
                write,
                coverage,
                lcd_coverage,
                gamma,
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
            let width = match key.line {
                LineWidth::Natural => max_size as f32,
                LineWidth::Wrap(bits) | LineWidth::Ellipsis(bits) => f32::from_bits(bits),
            };
            format
                .SetWordWrapping(if matches!(key.line, LineWidth::Wrap(_)) {
                    DWRITE_WORD_WRAPPING_WRAP
                } else {
                    DWRITE_WORD_WRAPPING_NO_WRAP
                })
                .ok()?;
            if matches!(key.line, LineWidth::Ellipsis(_)) {
                let sign = self.write.CreateEllipsisTrimmingSign(&format).ok()?;
                format
                    .SetTrimming(
                        &DWRITE_TRIMMING {
                            granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                            ..Default::default()
                        },
                        &sign,
                    )
                    .ok()?;
            }
            let utf16: Vec<_> = key.text.encode_utf16().collect();
            let mut layout = self
                .write
                .CreateTextLayout(&utf16, &format, width, max_size as f32)
                .ok()?;
            let mut metrics = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut metrics).ok()?;
            let mut draw_x = f32::from(key.origin[0]) / 64.;
            let mut fraction = 0.;
            let indent = utf16.iter().take_while(|&&c| c == u16::from(b' ')).count();
            if indent > 0
                && indent < utf16.len()
                && matches!(key.line, LineWidth::Natural)
                && metrics.lineCount == 1
            {
                // WPF's line formatter emits leading spaces as their own run.
                // It advances in 1/300-DIP units and translates the next run's
                // coverage texture, retaining the fraction of a display pixel.
                let (mut x, mut y) = (0., 0.);
                let mut hit = DWRITE_HIT_TEST_METRICS::default();
                layout
                    .HitTestTextPosition(0, true, &mut x, &mut y, &mut hit)
                    .ok()?;
                if hit.left == 0. && hit.bidiLevel == 0 {
                    let scale = f32::from_bits(key.scale);
                    let space = (x * 300. / scale).round() * scale / 300.;
                    draw_x += space * indent as f32;
                    fraction = draw_x.fract();
                    draw_x = draw_x.floor();
                    layout = self
                        .write
                        .CreateTextLayout(&utf16[indent..], &format, width, max_size as f32)
                        .ok()?;
                }
            }
            let width = metrics.widthIncludingTrailingWhitespace.ceil() as usize + 3;
            let height = metrics.height.ceil() as usize + 2;
            if width > max_size || height > max_size || width.checked_mul(height)? > 1_048_576 {
                return None;
            }
            let origin = windows_numerics::Vector2 {
                X: draw_x + fraction,
                Y: f32::from(key.origin[1]) / 64.,
            };
            let params = if key.colors.is_some() {
                &self.lcd_coverage
            } else {
                &self.coverage
            };
            let scale = f32::from_bits(key.scale);
            let size = [width, height];
            let bytes = glyphs::coverage(self, params, &layout, origin, scale, size)
                .or_else(|| self.bitmap_coverage(&layout, params, origin, fraction, scale, size))?;
            let colors = key.colors.map(|(ink, backdrop)| {
                let channels = [
                    (ink.r(), backdrop.r()),
                    (ink.g(), backdrop.g()),
                    (ink.b(), backdrop.b()),
                ];
                channels.map(|(fg, bg)| {
                    std::array::from_fn::<_, 256, _>(|i| compose_glyph(i as u8, fg, bg, self.gamma))
                })
            });
            let mask = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|px| {
                    if let Some([red, green, blue]) = &colors {
                        if px[..3] == [0, 0, 0] {
                            Color32::TRANSPARENT
                        } else {
                            Color32::from_rgb(
                                red[px[2] as usize],
                                green[px[1] as usize],
                                blue[px[0] as usize],
                            )
                        }
                    } else {
                        // For an alpha bitmap WPF samples the center of the coverage texture.
                        Color32::from_white_alpha(px[1])
                    }
                })
                .collect();
            Some((
                ColorImage::new([width, height], mask),
                vec2(metrics.widthIncludingTrailingWhitespace, metrics.height),
            ))
        }
    }

    fn bitmap_coverage(
        &self,
        layout: &IDWriteTextLayout,
        params: &IDWriteRenderingParams,
        mut origin: windows_numerics::Vector2,
        fraction: f32,
        scale: f32,
        [width, height]: [usize; 2],
    ) -> Option<Vec<u8>> {
        origin.X -= fraction;
        unsafe {
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
            let white = D2D1_COLOR_F {
                r: 1.,
                g: 1.,
                b: 1.,
                a: 1.,
            };
            let black = D2D1_COLOR_F {
                a: 1.,
                ..Default::default()
            };
            let brush = target.CreateSolidColorBrush(&white, None).ok()?;
            target.BeginDraw();
            target.Clear(Some(&black));
            target.SetTextRenderingParams(params);
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
            if glyphs::draw(&target, &brush, layout, origin, scale).is_err() {
                // An inline object or decoration can require the system renderer.
                // Clear any runs already painted before replaying the whole layout.
                target.Clear(Some(&black));
                target.DrawTextLayout(origin, layout, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
            target.EndDraw(None, None).ok()?;
            let mut bytes = vec![0; width * height * 4];
            bitmap
                .CopyPixels(std::ptr::null(), (width * 4) as u32, &mut bytes)
                .ok()?;
            for pixel in bytes.chunks_exact_mut(4) {
                for channel in &mut pixel[..3] {
                    *channel = self.contrast[usize::from(*channel)];
                }
            }
            if fraction > 0. {
                let bgr = params.GetPixelGeometry() == DWRITE_PIXEL_GEOMETRY_BGR;
                for row in bytes.chunks_exact_mut(width * 4) {
                    translate_coverage(row, fraction, bgr);
                }
            }
            Some(bytes)
        }
    }
}

// WPF positions the contrast-adjusted RGB coverage strip before applying
// foreground gamma. Traverse the BGRA buffer from right to left so that the
// two source subpixels are still available when writing the destination.
fn translate_coverage(row: &mut [u8], fraction: f32, bgr: bool) {
    let thirds = fraction * 3.;
    let whole = thirds.floor() as usize;
    let blend = ((thirds - whole as f32) * 65536.) as u32;
    let index = |i: usize| (i / 3) * 4 + if bgr { i % 3 } else { 2 - i % 3 };
    for i in (0..row.len() / 4 * 3).rev() {
        let sample = |distance| {
            i.checked_sub(distance)
                .map_or(0, |j| u32::from(row[index(j)]))
        };
        let value = sample(whole) * (65536 - blend) + sample(whole + 1) * blend;
        row[index(i)] = ((value + 32768) >> 16) as u8;
    }
}

// Glyph gamma coefficients from dotnet/wpf, WpfGfx/core/common/Gamma.cpp.
// Copyright (c) .NET Foundation and Contributors, MIT (windows/assets/WinForms-LICENSE).
fn glyph_gamma(display_gamma: f32) -> [f32; 4] {
    const COEFFICIENTS: [[f32; 4]; 13] = [
        [0., 0., 0., 0.],
        [0.0166, -0.0807, 0.2227, -0.0751],
        [0.0350, -0.1760, 0.4325, -0.1370],
        [0.0543, -0.2821, 0.6302, -0.1876],
        [0.0739, -0.3963, 0.8167, -0.2287],
        [0.0933, -0.5161, 0.9926, -0.2616],
        [0.1121, -0.6395, 1.1588, -0.2877],
        [0.1300, -0.7649, 1.3159, -0.3080],
        [0.1469, -0.8911, 1.4644, -0.3234],
        [0.1627, -1.0170, 1.6051, -0.3347],
        [0.1773, -1.1420, 1.7385, -0.3426],
        [0.1908, -1.2652, 1.8650, -0.3476],
        [0.2031, -1.3864, 1.9851, -0.3501],
    ];
    COEFFICIENTS[(((display_gamma - 1.) * 10.) as usize).min(COEFFICIENTS.len() - 1)]
}

fn compose_glyph(sample: u8, foreground: u8, background: u8, [g1, g2, g3, g4]: [f32; 4]) -> u8 {
    let alpha = f32::from(sample) / 255.;
    let ink = f32::from(foreground) / 255.;
    let corrected = alpha + alpha * (1. - alpha) * ((g1 * ink + g2) * alpha + (g3 * ink + g4));
    // The display target blends an 8-bit coverage value with the destination channel.
    let coverage = (255. * corrected.clamp(0., 1.)).round() as u32;
    ((u32::from(foreground) * coverage + u32::from(background) * (255 - coverage) + 127) / 255)
        as u8
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
    scale: u32,
    semibold: bool,
    line: LineWidth,
    origin: [u8; 2],
    colors: Option<(Color32, Color32)>,
}

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
enum LineWidth {
    Natural,
    Wrap(u32),
    Ellipsis(u32),
}

#[derive(Clone, Copy, PartialEq)]
pub enum LabelLayout {
    Inline,
    Block,
    SingleLine,
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
                scale: scale.to_bits(),
                semibold,
                line: LineWidth::Natural,
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
    draw_label(ui, text, size, semibold, color, LabelLayout::Block)
        .unwrap_or_else(|| ui.label(crate::style::rich(text, size, semibold, color)))
}

pub fn natural_size(ui: &Ui, text: &str, size: f32, semibold: bool) -> Option<Vec2> {
    let scale = ui.ctx().pixels_per_point();
    let key = RunKey {
        text: text.into(),
        pixels: (size * scale).to_bits(),
        scale: scale.to_bits(),
        semibold,
        line: LineWidth::Natural,
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
    layout: LabelLayout,
) -> Option<Response> {
    let width = ui.available_width();
    let scale = ui.ctx().pixels_per_point();
    let max_size = ui.input(|i| i.max_texture_side);
    if !width.is_finite() || width <= 0. || ui.layout().main_wrap() {
        return None;
    }
    let constrained = natural_size(ui, text, size, semibold).is_none_or(|v| v.x > width);
    let line = match (constrained, layout) {
        (false, _) => LineWidth::Natural,
        (true, LabelLayout::SingleLine) => LineWidth::Ellipsis((width * scale).to_bits()),
        (true, _) => LineWidth::Wrap((width * scale).to_bits()),
    };
    let mut key = RunKey {
        text: text.into(),
        pixels: (size * scale).to_bits(),
        scale: scale.to_bits(),
        semibold,
        line,
        origin: [0; 2],
        colors: None,
    };
    let renderer = ui.ctx().plugin_opt::<TextRenderer>()?;
    let mut renderer = renderer.lock();
    let measured = renderer.run(ui.ctx(), key.clone(), max_size)?.layout_size;
    // WPF measures TextBlocks before arranging their parent. Retain the
    // fractional natural size across stacked rows, independently of the bitmap.
    let mut desired = measured / scale;
    desired.x = if layout == LabelLayout::Inline {
        desired.x.min(width)
    } else {
        width
    };
    let (rect, response) = ui.allocate_exact_size(desired, Sense::hover());
    let (start, origin) = raster_origin(rect.min, scale);
    key.origin = origin;
    let run = renderer.run(ui.ctx(), key, max_size)?;
    let mut painter = ui.painter().clone();
    if layout == LabelLayout::SingleLine && constrained {
        let mut clip = painter.clip_rect();
        clip.max.x = clip.max.x.min(rect.right());
        painter.set_clip_rect(clip);
    }
    painter.image(
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
    fn fractional_run_translation_crosses_pixel_boundaries_in_display_order() {
        let mut rgb = [60, 30, 0, 255, 150, 120, 90, 255];
        translate_coverage(&mut rgb, 0.5, false);
        assert_eq!(rgb, [15, 0, 0, 255, 105, 75, 45, 255]);
        let mut bgr = [0, 30, 60, 255, 90, 120, 150, 255];
        translate_coverage(&mut bgr, 0.5, true);
        assert_eq!(bgr, [0, 0, 15, 255, 45, 75, 105, 255]);
    }

    #[test]
    fn cleartype_coverage_blends_light_and_dark_ink() {
        let gamma = glyph_gamma(1.75);
        let samples = [0, 67, 127, 189, 255];
        assert_eq!(
            samples.map(|s| compose_glyph(s, 23, 255, gamma)),
            [255, 211, 172, 117, 23]
        );
        assert_eq!(
            samples.map(|s| compose_glyph(s, 229, 24, gamma)),
            [24, 106, 154, 191, 229]
        );
        assert_eq!(compose_glyph(127, 255, 0, glyph_gamma(1.)), 127);
        assert_eq!(glyph_gamma(3.), glyph_gamma(2.25));
    }

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
        // The WPF run for "변경" starts at 212.0933 DIP in the mixed label.
        // A leading Segoe UI space (3.56 DIP) plus 34/64 DIP places that
        // same run at 4.09125 DIP on its own. Both masks must keep the phase.
        let mixed = RunKey {
            text: "LicenseDocument { Title = 저작권 및 변경 고지".into(),
            pixels: 13_f32.to_bits(),
            scale: 1_f32.to_bits(),
            semibold: false,
            line: LineWidth::Natural,
            origin: [0; 2],
            colors: None,
        };
        let (label, _) = factories.render(&mixed, 4096).unwrap();
        let (fragment, _) = factories
            .render(
                &RunKey {
                    text: " 변경".into(),
                    origin: [34, 0],
                    ..mixed
                },
                4096,
            )
            .unwrap();
        for y in 0..label.height() {
            for x in 0..26 {
                assert!(label[(212 + x, y)].a().abs_diff(fragment[(4 + x, y)].a()) <= 2);
            }
        }
        let mut previous_width = 0;
        for pixels in [13.0_f32, 26.0] {
            let run = RunKey {
                text: "Codex · 계정 연결".into(),
                pixels: pixels.to_bits(),
                scale: (pixels / 13.).to_bits(),
                semibold: false,
                line: LineWidth::Natural,
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
            // Leave room for three Latin letters and the sign. The remaining
            // Latin/Korean text must collapse onto the same single line.
            let sign = RunKey {
                text: "Cod…".into(),
                ..run.clone()
            };
            let (sign_mask, sign_size) = factories.render(&sign, 4096).unwrap();
            let ellipsis = RunKey {
                line: LineWidth::Ellipsis((sign_size.x + 0.1).to_bits()),
                ..run.clone()
            };
            let (trimmed, trimmed_size) = factories.render(&ellipsis, 4096).unwrap();
            assert_eq!(trimmed_size, sign_size);
            assert_eq!(trimmed.pixels, sign_mask.pixels);
            let narrow = RunKey {
                line: LineWidth::Wrap((3. * pixels).to_bits()),
                ..run
            };
            let (wrapped, wrapped_size) = factories.render(&narrow, 4096).unwrap();
            assert!(wrapped.width() < mask.width() && wrapped_size.y > size.y);
            previous_width = mask.width();
        }
    }
}
