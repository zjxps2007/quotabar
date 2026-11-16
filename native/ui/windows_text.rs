use eframe::egui::{self, epaint::TextShape, *};
use std::{collections::HashMap, ops::Range};
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
    software_gamma: [[u8; 2]; 256],
    contrast: [u8; 256],
    draw: ID2D1Factory,
    images: IWICImagingFactory,
}

impl Factories {
    fn line_box(&self, size: f32, semibold: bool, scale: f32, stacked: bool) -> Option<(f32, f32)> {
        let m = self.line_metrics[usize::from(semibold)]?;
        let height = i32::from(m.ascent) + i32::from(m.descent) + i32::from(m.lineGap);
        if !stacked {
            let unit = size / f32::from(m.designUnitsPerEm);
            return Some((height as f32 * unit, f32::from(m.ascent) * unit));
        }
        // TextFormatter measures line spacing and ascent separately in 1/300 DIP.
        let units = f64::from(size / scale) * 300. / f64::from(m.designUnitsPerEm);
        let pixels =
            |design: i32| ((f64::from(design) * units).round_ties_even() / 300.) as f32 * scale;
        Some((pixels(height), pixels(i32::from(m.ascent))))
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
                software_gamma: software_gamma_table(gamma),
                contrast,
                draw: D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
                images: CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?,
            })
        }
    }

    fn render(&self, key: &RunKey, max_size: usize) -> Option<(ColorImage, Vec2, Range<usize>)> {
        let size = f32::from_bits(key.pixels);
        let scale = f32::from_bits(key.scale);
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
            if let Some((height, baseline)) = self.line_box(size, key.semibold, scale, key.stacked)
            {
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
            let mut run_text = utf16.as_slice();
            let mut layout = self
                .write
                .CreateTextLayout(run_text, &format, width, max_size as f32)
                .ok()?;
            let mut metrics = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut metrics).ok()?;
            let mut draw_x = f32::from(key.origin.x64) / 64.;
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
                    let space = (x * 300. / scale).round() * scale / 300.;
                    draw_x += space * indent as f32;
                    fraction = draw_x.fract();
                    draw_x = draw_x.floor();
                    run_text = &utf16[indent..];
                    layout = self
                        .write
                        .CreateTextLayout(run_text, &format, width, max_size as f32)
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
                Y: f32::from_bits(key.origin.y_bits),
            };
            let params = if key.colors.is_some() {
                &self.lcd_coverage
            } else {
                &self.coverage
            };
            let size = [width, height];
            glyphs::mark_dash_runs(&self.write, &layout, run_text).ok()?;
            let rounding = key.subpixel_rounding();
            let bytes = glyphs::coverage(self, params, &layout, origin, scale, size, rounding)
                .or_else(|| self.bitmap_coverage(&layout, params, origin, fraction, key, size))?;
            // Include subpixel ink from all three channels, even for a run
            // whose first texture uses the grayscale coverage channel.
            let has_ink = |row: &[u8]| row.chunks_exact(4).any(|pixel| pixel[..3] != [0; 3]);
            let mut rows = bytes.chunks_exact(width * 4);
            let ink_rows = rows
                .clone()
                .position(has_ink)
                .map_or(0..0, |top| top..rows.rposition(has_ink).unwrap() + 1);
            let colors = key.colors.map(|colors| {
                let ink = colors.ink;
                let backdrop = colors.backdrop;
                let channels = [
                    (ink.r(), backdrop.r()),
                    (ink.g(), backdrop.g()),
                    (ink.b(), backdrop.b()),
                ];
                let lookup = channels.map(|(fg, bg)| {
                    std::array::from_fn::<_, 256, _>(|i| {
                        if colors.software {
                            software_channel(i as u8, fg, bg, &self.software_gamma)
                        } else {
                            compose_glyph(i as u8, fg, bg, self.gamma)
                        }
                    })
                });
                (ink, lookup)
            });
            let mask = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|px| {
                    if let Some((ink, [red, green, blue])) = &colors {
                        if px[..3] == [0, 0, 0] {
                            Color32::TRANSPARENT
                        } else if px[..3] == [255, 255, 255] {
                            *ink
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
                ink_rows,
            ))
        }
    }

    fn bitmap_coverage(
        &self,
        layout: &IDWriteTextLayout,
        params: &IDWriteRenderingParams,
        mut origin: windows_numerics::Vector2,
        fraction: f32,
        key: &RunKey,
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
            if glyphs::draw(&target, &brush, layout, origin, f32::from_bits(key.scale)).is_err() {
                // An inline object or decoration can require the system renderer.
                // Restore its default brush before replaying the entire layout.
                layout
                    .SetDrawingEffect(
                        None,
                        DWRITE_TEXT_RANGE {
                            startPosition: 0,
                            length: u32::MAX,
                        },
                    )
                    .ok()?;
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
                    translate_coverage(row, fraction, bgr, key.subpixel_rounding());
                }
            }
            Some(bytes)
        }
    }
}

// WPF positions the contrast-adjusted RGB coverage strip before applying
// foreground gamma. Traverse the BGRA buffer from right to left so that the
// two source subpixels are still available when writing the destination.
fn translate_coverage(row: &mut [u8], fraction: f32, bgr: bool, rounding: u32) {
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
        row[index(i)] = ((value + rounding) >> 16) as u8;
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
    // The display shader keeps fractional coverage through the blend. Only the
    // final destination channel is stored in an eight-bit render target.
    let bg = f32::from(background);
    (bg + (f32::from(foreground) - bg) * corrected.clamp(0., 1.)).round() as u8
}

// WPF's software ClearType scan operation uses two byte coefficients for
// gamma correction and truncates the foreground and destination separately.
// See CGammaHandler::CalculateGammaTable and CSWGlyphRunPainter::ApplyClearTypeOver.
fn software_gamma_table([g1, g2, g3, g4]: [f32; 4]) -> [[u8; 2]; 256] {
    let norm13 = 65536. / (255. * 255.);
    let norm24 = 256. / 255.;
    std::array::from_fn(|i| {
        let a = i as f32 / 255.;
        let curve = a * (1. - a);
        let offset = a + curve * ((g2 * norm24) * a + g4 * norm24);
        let slope = curve * ((g1 * norm13) * a + g3 * norm13);
        [
            (255. * offset).round_ties_even() as u8,
            (255. * slope).round_ties_even() as u8,
        ]
    })
}

fn software_channel(sample: u8, ink: u8, backdrop: u8, gamma: &[[u8; 2]; 256]) -> u8 {
    // The source brush is an opaque premultiplied color. WPF separately
    // short-circuits whole pixels with all three coverages zero or full.
    let combined = (usize::from(sample) * 255) >> 8;
    let [offset, slope] = gamma[combined].map(u32::from);
    let coverage = offset + ((slope * u32::from(ink)) >> 8);
    (((u32::from(ink) * coverage) >> 8) + ((u32::from(backdrop) * (255 - coverage)) >> 8)) as u8
}

thread_local! {
    static FACTORIES: Option<Factories> = Factories::new().ok();
}
pub fn line_height(size: f32) -> Option<f32> {
    FACTORIES.with(|f| Some(f.as_ref()?.line_box(size, false, 1., false)?.0))
}

#[derive(Clone, Eq, PartialEq, Hash)]
struct RunKey {
    text: String,
    pixels: u32,
    scale: u32,
    semibold: bool,
    stacked: bool,
    line: LineWidth,
    origin: RasterOrigin,
    colors: Option<LcdColors>,
}

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
struct LcdColors {
    ink: Color32,
    backdrop: Color32,
    software: bool,
}

impl RunKey {
    fn subpixel_rounding(&self) -> u32 {
        if self.colors.is_some_and(|c| c.software) {
            0
        } else {
            32768
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
enum LineWidth {
    Natural,
    Wrap(u32),
    Ellipsis(u32),
}

/// WPF stacks natural text heights with whole-DIP margins and padding. Keep
/// that sum independently of egui's 1/32-DIP widget allocation grid.
pub struct TextStack(std::cell::Cell<f64>);

impl TextStack {
    pub fn begin(ui: &Ui) -> Self {
        if let Some(renderer) = ui.ctx().plugin_opt::<TextRenderer>() {
            renderer.lock().stacked_viewport = Some(ui.ctx().viewport_id());
        }
        Self(std::cell::Cell::new(f64::from(ui.next_widget_position().y)))
    }

    pub fn line_height(size: f32) -> Option<f32> {
        FACTORIES.with(|f| Some(f.as_ref()?.line_box(size, false, 1., true)?.0))
    }

    pub fn top(&self, allocated: f32) -> f32 {
        let anchor = self.0.get();
        (anchor + (f64::from(allocated) - anchor).round()) as f32
    }

    pub fn advance(&self, height: f32) {
        let height = (f64::from(height) * 300.).round_ties_even() / 300.;
        self.0.set(self.0.get() + height);
    }

    pub fn label(
        &self,
        ui: &mut Ui,
        text: &str,
        size: f32,
        semibold: bool,
        color: Color32,
    ) -> Response {
        draw_label(ui, text, size, semibold, color, LabelLayout::Stacked(self)).unwrap_or_else(
            || {
                let response = ui.label(crate::style::rich(text, size, semibold, color));
                self.advance(response.rect.height());
                response
            },
        )
    }
}

#[derive(Clone, Copy)]
pub enum LabelLayout<'a> {
    Inline,
    Block,
    Stacked(&'a TextStack),
    SingleLine,
}

/// DirectWrite snaps each baseline in device space. Preserve the fraction of
/// the layout origin when rasterizing the run into its own pixel-aligned bitmap.
#[derive(Clone, Copy, Default, Eq, PartialEq, Hash)]
struct RasterOrigin {
    x64: u8,
    y_bits: u32,
}

fn raster_origin(position: Pos2, scale: f32, stacked: bool) -> (Pos2, RasterOrigin) {
    let x = (position.x * scale * 64.).round() as i32;
    let y = position.y * scale;
    let y = if stacked { y } else { (y * 64.).round() / 64. };
    let top = y.floor();
    (
        pos2(x.div_euclid(64) as f32 / scale, top / scale),
        RasterOrigin {
            x64: x.rem_euclid(64) as u8,
            y_bits: (y - top).to_bits(),
        },
    )
}

struct CachedRun {
    texture: TextureHandle,
    layout_size: Vec2,
    ink_rows: Range<usize>,
    last_pass: u64,
}

impl CachedRun {
    fn ink_bounds(&self, texture_rect: Rect) -> Rect {
        let row_height = texture_rect.height() / self.texture.size_vec2().y;
        Rect::from_x_y_ranges(
            texture_rect.x_range(),
            (texture_rect.top() + self.ink_rows.start as f32 * row_height)
                ..=(texture_rect.top() + self.ink_rows.end as f32 * row_height),
        )
    }
}

#[derive(Default)]
pub struct TextRenderer {
    runs: HashMap<RunKey, CachedRun>,
    pass: u64,
    pub bitmap_target: bool,
    // Only a viewport that laid out a natural stack in this pass can use its
    // unrounded paint origins. Other widgets retain their own allocation grid.
    stacked_viewport: Option<ViewportId>,
}

impl TextRenderer {
    fn run(&mut self, ctx: &Context, key: RunKey, max_size: usize) -> Option<&CachedRun> {
        let run = match self.runs.entry(key) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let (image, layout_size, ink_rows) =
                    FACTORIES.with(|f| f.as_ref()?.render(e.key(), max_size))?;
                e.insert(CachedRun {
                    texture: ctx.load_texture("DirectWrite text", image, TextureOptions::NEAREST),
                    layout_size,
                    ink_rows,
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
        software: bool,
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
        let colors = backdrop
            .filter(|_| color.a() == 255)
            .map(|backdrop| LcdColors {
                ink: color,
                backdrop,
                software,
            });
        let stacked = self.stacked_viewport == Some(ctx.viewport_id());
        let mut shapes = Vec::with_capacity(text.galley.rows.len());
        for row in &text.galley.rows {
            if row.glyphs.is_empty() {
                continue;
            }
            let (start, origin) = raster_origin(text.pos + row.pos.to_vec2(), scale, stacked);
            let key = RunKey {
                text: row.glyphs.iter().map(|glyph| glyph.chr).collect(),
                pixels: (format.font_id.size * scale).to_bits(),
                scale: scale.to_bits(),
                semibold,
                stacked,
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
    Some(MeasuredLabel::new(ui, text, size, semibold, LineWidth::Natural)?.size)
}

pub struct MeasuredLabel {
    key: RunKey,
    pub size: Vec2,
}

impl MeasuredLabel {
    fn new(ui: &Ui, text: &str, size: f32, semibold: bool, line: LineWidth) -> Option<Self> {
        let scale = ui.ctx().pixels_per_point();
        let renderer = ui.ctx().plugin_opt::<TextRenderer>()?;
        let mut renderer = renderer.lock();
        let key = RunKey {
            text: text.into(),
            pixels: (size * scale).to_bits(),
            scale: scale.to_bits(),
            semibold,
            stacked: renderer.stacked_viewport == Some(ui.ctx().viewport_id()),
            line,
            origin: RasterOrigin::default(),
            colors: None,
        };
        let size = renderer
            .run(ui.ctx(), key.clone(), ui.input(|i| i.max_texture_side))?
            .layout_size
            / scale;
        Some(Self { key, size })
    }

    pub fn fit(
        ui: &Ui,
        text: &str,
        size: f32,
        semibold: bool,
        width: f32,
        layout: LabelLayout<'_>,
    ) -> Option<Self> {
        let measure = |line| Self::new(ui, text, size, semibold, line);
        if let Some(label) = measure(LineWidth::Natural)
            && label.size.x <= width
        {
            return Some(label);
        }
        let pixels = (width * ui.ctx().pixels_per_point()).to_bits();
        measure(if matches!(layout, LabelLayout::SingleLine) {
            LineWidth::Ellipsis(pixels)
        } else {
            LineWidth::Wrap(pixels)
        })
    }

    pub fn paint(mut self, painter: &Painter, pos: Pos2, color: Color32) -> Option<()> {
        let ctx = painter.ctx();
        let scale = ctx.pixels_per_point();
        let (start, origin) = raster_origin(pos, scale, self.key.stacked);
        self.key.origin = origin;
        let renderer = ctx.plugin_opt::<TextRenderer>()?;
        let mut renderer = renderer.lock();
        let run = renderer.run(ctx, self.key, ctx.input(|i| i.max_texture_side))?;
        painter.image(
            run.texture.id(),
            Rect::from_min_size(start, run.texture.size_vec2() / scale),
            Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
            color,
        );
        Some(())
    }
}

pub fn draw_label(
    ui: &mut Ui,
    text: &str,
    size: f32,
    semibold: bool,
    color: Color32,
    layout: LabelLayout<'_>,
) -> Option<Response> {
    let width = ui.available_width();
    if !width.is_finite() || width <= 0. || ui.layout().main_wrap() {
        return None;
    }
    let measured = MeasuredLabel::fit(ui, text, size, semibold, width, layout)?;
    // WPF measures TextBlocks before arranging their parent. Retain the
    // fractional natural size across stacked rows, independently of the bitmap.
    let mut desired = measured.size;
    desired.x = if matches!(layout, LabelLayout::Inline) {
        desired.x.min(width)
    } else {
        width
    };
    let (rect, response) = ui.allocate_exact_size(desired, Sense::hover());
    let mut pos = rect.min;
    if let LabelLayout::Stacked(stack) = layout {
        pos.y = stack.top(pos.y);
    }
    let mut painter = ui.painter().clone();
    if matches!(measured.key.line, LineWidth::Ellipsis(_)) {
        let mut clip = painter.clip_rect();
        clip.max.x = clip.max.x.min(rect.right());
        painter.set_clip_rect(clip);
    }
    measured.paint(&painter, pos, color)?;
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), text));
    if let LabelLayout::Stacked(stack) = layout {
        stack.advance(desired.y);
    }
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
        let surface = surfaces
            .iter()
            .rev()
            .find(|s| s.bounds.intersect(area).is_positive())?;
        if !surface.interior.contains_rect(area) {
            return None;
        }
        let middle_x = surface.interior.shrink2(vec2(surface.radius, 0.));
        let middle_y = surface.interior.shrink2(vec2(0., surface.radius));
        (middle_x.contains_rect(area) || middle_y.contains_rect(area))
            .then_some(surface.color)
            .flatten()
    }

    fn unknown(bounds: Rect) -> Self {
        Self {
            bounds,
            interior: Rect::NOTHING,
            radius: 0.,
            color: None,
        }
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
        let mut surface = Self::unknown(bounds);
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

fn software_text_composition() -> bool {
    use std::sync::OnceLock;
    use windows_sys::Win32::{System::Registry::*, UI::WindowsAndMessaging::*};
    use windows_sys::core::w;
    // The original uses WPF's default RDP policy. Check the session again
    // after reconnecting, since a running dashboard can move to the console.
    if unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0 {
        return true;
    }
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| unsafe {
        let mut value = 0u32;
        let mut size = size_of::<u32>() as u32;
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Avalon.Graphics"),
            w!("DisableHWAcceleration"),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut value).cast(),
            &mut size,
        ) == 0
            && value == 1
    })
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
        let software = lcd && software_text_composition();
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
                        .map(|(key, run)| (run.texture.id(), (key, run)))
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
                            && let Some(&(key, run)) = masks.get(&mesh.texture_id)
                        {
                            // Adjacent TextBlocks can have overlapping bitmap padding.
                            // Only the occupied rows can cover a background or other text.
                            let ink = run
                                .ink_bounds(mesh.calc_bounds())
                                .intersect(entry.clip_rect);
                            if ink.is_positive() {
                                if let Some(backdrop) = Surface::behind(&surfaces, ink)
                                    && let Some(first) = mesh.vertices.first()
                                    && first.color.a() == 255
                                    && mesh.vertices.iter().all(|v| v.color == first.color)
                                {
                                    let mut colored = key.clone();
                                    colored.colors = Some(LcdColors {
                                        ink: first.color,
                                        backdrop,
                                        software,
                                    });
                                    runs.push((index, Label::Mask(mesh.clone(), colored)));
                                }
                                surfaces.push(Surface::unknown(ink));
                            }
                            continue;
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
                        self.paint(ctx, &text, scale, max_size, backdrop, software)
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
        if self.stacked_viewport == Some(ctx.viewport_id()) {
            self.stacked_viewport = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_run_translation_crosses_pixel_boundaries_in_display_order() {
        let mut rgb = [60, 30, 0, 255, 150, 120, 90, 255];
        translate_coverage(&mut rgb, 0.5, false, 32768);
        assert_eq!(rgb, [15, 0, 0, 255, 105, 75, 45, 255]);
        let mut bgr = [0, 30, 60, 255, 90, 120, 150, 255];
        translate_coverage(&mut bgr, 0.5, true, 32768);
        assert_eq!(bgr, [0, 0, 15, 255, 45, 75, 105, 255]);
    }

    #[test]
    fn partial_lcd_pixels_match_the_software_rendered_wpf_window() {
        let gamma = software_gamma_table(glyph_gamma(1.75));
        // Channel pairs sampled on the same glyph edges in the official
        // license window's light and dark themes over Remote Desktop.
        let samples = [0, 7, 59, 109, 153, 191, 225, 255];
        assert_eq!(
            samples.map(|s| software_channel(s, 23, 255, &gamma)),
            [254, 250, 216, 184, 152, 114, 71, 23]
        );
        assert_eq!(
            samples.map(|s| software_channel(s, 242, 28, &gamma)),
            [27, 36, 104, 152, 182, 204, 222, 239]
        );
    }

    #[test]
    fn cleartype_coverage_blends_light_and_dark_ink() {
        let gamma = glyph_gamma(1.75);
        // Paired glyph edges from the original license window on the console.
        let samples = [0, 59, 109, 153, 191, 225, 255];
        assert_eq!(
            samples.map(|s| compose_glyph(s, 23, 255, gamma)),
            [255, 216, 185, 152, 114, 71, 23]
        );
        assert_eq!(
            samples.map(|s| compose_glyph(s, 242, 28, gamma)),
            [28, 108, 154, 185, 207, 225, 242]
        );
        assert_eq!(compose_glyph(127, 255, 0, glyph_gamma(1.)), 127);
        assert_eq!(glyph_gamma(3.), glyph_gamma(2.25));
    }

    struct Apartment;
    impl Apartment {
        fn open() -> Self {
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
                .ok()
                .unwrap();
            Self
        }
    }
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    #[test]
    fn wrapped_help_and_consent_keep_the_original_device_baselines() {
        let _apartment = Apartment::open();
        let factories = Factories::new().unwrap();
        let stack = TextStack(std::cell::Cell::new(28.));
        // Title, subtitle, provider, path caption, editor and browse button.
        for (size, bold) in [
            (24., true),
            (12., false),
            (13., false),
            (11., false),
            (13., false),
            (13., false),
        ] {
            stack.advance(factories.line_box(size, bold, 1., true).unwrap().0);
        }
        let help = stack.top(281.375);
        let (height, ascent) = factories.line_box(12., false, 1., true).unwrap();
        stack.advance(2. * height);
        let consent = stack.top(328.28125);
        let (_, checkbox_ascent) = factories.line_box(13., false, 1., true).unwrap();
        // Absolute WPF GlyphRun baselines from the official connection dialog,
        // in 1/300 DIP. At 150% they fall at 441.5, 465.44 and 513.495 pixels.
        for scale in [1., 1.25, 1.5, 1.75, 2.] {
            for (top, offset, expected) in [
                (help, ascent, 88300),
                (help, height + ascent, 93088),
                (consent, checkbox_ascent, 102699),
            ] {
                let (pos, origin) = raster_origin(pos2(0., top), scale, true);
                let baseline = (pos.y * scale).round()
                    + (f32::from_bits(origin.y_bits) + offset * scale).round();
                assert_eq!(
                    baseline,
                    (f64::from(expected) * f64::from(scale) / 300.).round() as f32
                );
            }
        }
    }

    #[test]
    fn reopening_another_dialog_restores_its_native_line_spacing() {
        let _apartment = Apartment::open();
        let ctx = Context::default();
        ctx.add_plugin(TextRenderer::default());
        // Connection and license dialogs reuse the same viewport. The second
        // dialog must recover its ordinary glyph layout, including cached runs.
        let measure = |stacked| {
            let mut height = 0.;
            let _ = ctx.run(Default::default(), |ctx| {
                CentralPanel::default().show(ctx, |ui| {
                    if stacked {
                        TextStack::begin(ui);
                    }
                    height = natural_size(ui, "LicenseDocument", 13., false).unwrap().y;
                });
            });
            height
        };
        let ordinary = measure(false);
        assert_eq!(ordinary, line_height(13.).unwrap());
        assert_eq!(measure(true), TextStack::line_height(13.).unwrap());
        assert_ne!(ordinary, TextStack::line_height(13.).unwrap());
        assert_eq!(measure(false), ordinary);
    }

    #[test]
    fn adjacent_text_blocks_keep_cleartype_over_their_unpainted_padding() {
        let _apartment = Apartment::open();
        let ctx = Context::default();
        let mut renderer = TextRenderer::default();
        let background = Rect::from_min_size(Pos2::ZERO, vec2(600., 150.));
        for scale in [1.0_f32, 1.5] {
            let mut measure = |text: &str, size: f32, bold: bool, top: f32| {
                let pos = pos2(28., top);
                let (start, origin) = raster_origin(pos, scale, false);
                let key = RunKey {
                    text: text.into(),
                    pixels: (size * scale).to_bits(),
                    scale: scale.to_bits(),
                    semibold: bold,
                    stacked: false,
                    line: LineWidth::Natural,
                    origin,
                    colors: None,
                };
                let run = renderer.run(&ctx, key, 4096).unwrap();
                let texture = Rect::from_min_size(start, run.texture.size_vec2() / scale);
                (texture, run.ink_bounds(texture), run.layout_size.y / scale)
            };
            let (title, title_ink, height) = measure("계정 연결", 24., true, 28.);
            let (subtitle, subtitle_ink, _) =
                measure("서비스를 선택하세요.", 12., false, 28. + height);
            assert!(title.intersect(subtitle).is_positive());
            let surfaces = [
                Surface {
                    bounds: background,
                    interior: background,
                    radius: 0.,
                    color: Some(Color32::WHITE),
                },
                Surface::unknown(title_ink),
            ];
            assert_eq!(
                Surface::behind(&surfaces, subtitle_ink),
                Some(Color32::WHITE)
            );
            assert_eq!(Surface::behind(&surfaces, title_ink), None);
        }
    }

    #[test]
    fn hyphenated_labels_preserve_the_wpf_suffix_origin() {
        let _apartment = Apartment::open();
        let factories = Factories::new().unwrap();
        // Baseline origins from WPF TextBlock at 13 DIP, in a Segoe UI run.
        for (text, column, fraction) in [
            ("C-beta", 13, 16),
            ("ii-beta", 11, 32),
            ("sss-beta", 21, 48),
        ] {
            let whole = RunKey {
                text: text.into(),
                pixels: 13_f32.to_bits(),
                scale: 1_f32.to_bits(),
                semibold: false,
                stacked: false,
                line: LineWidth::Natural,
                origin: RasterOrigin::default(),
                colors: None,
            };
            let (label, _, _) = factories.render(&whole, 4096).unwrap();
            let suffix = RunKey {
                text: "beta".into(),
                origin: RasterOrigin {
                    x64: fraction,
                    ..Default::default()
                },
                ..whole
            };
            let (fragment, _, _) = factories.render(&suffix, 4096).unwrap();
            for y in 0..label.height() {
                for x in 1..fragment.width().min(label.width() - column) {
                    assert_eq!(
                        label[(column + x, y)],
                        fragment[(x, y)],
                        "{text} ({x}, {y})"
                    );
                }
            }
        }
    }

    #[test]
    fn mixed_script_text_uses_the_system_rasterizer_at_both_scales() {
        let _apartment = Apartment::open();
        let factories = Factories::new().unwrap();
        // The WPF run for "변경" starts at 212.0933 DIP in the mixed label.
        // A leading Segoe UI space (3.56 DIP) plus 34/64 DIP places that
        // same run at 4.09125 DIP on its own. Both masks must keep the phase.
        let mixed = RunKey {
            text: "LicenseDocument { Title = 저작권 및 변경 고지".into(),
            pixels: 13_f32.to_bits(),
            scale: 1_f32.to_bits(),
            semibold: false,
            stacked: false,
            line: LineWidth::Natural,
            origin: RasterOrigin::default(),
            colors: None,
        };
        let (label, _, _) = factories.render(&mixed, 4096).unwrap();
        let (fragment, _, _) = factories
            .render(
                &RunKey {
                    text: " 변경".into(),
                    origin: RasterOrigin {
                        x64: 34,
                        ..Default::default()
                    },
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
                stacked: false,
                line: LineWidth::Natural,
                origin: RasterOrigin::default(),
                colors: None,
            };
            let (mask, size, _) = factories.render(&run, 4096).unwrap();
            let latin = RunKey {
                text: "Codex".into(),
                ..run
            };
            let (_, latin_size, _) = factories.render(&latin, 4096).unwrap();
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
            let (sign_mask, sign_size, _) = factories.render(&sign, 4096).unwrap();
            let ellipsis = RunKey {
                line: LineWidth::Ellipsis((sign_size.x + 0.1).to_bits()),
                ..run.clone()
            };
            let (trimmed, trimmed_size, _) = factories.render(&ellipsis, 4096).unwrap();
            assert_eq!(trimmed_size, sign_size);
            assert_eq!(trimmed.pixels, sign_mask.pixels);
            let narrow = RunKey {
                line: LineWidth::Wrap((3. * pixels).to_bits()),
                ..run
            };
            let (wrapped, wrapped_size, _) = factories.render(&narrow, 4096).unwrap();
            assert!(wrapped.width() < mask.width() && wrapped_size.y > size.y);
            previous_width = mask.width();
        }
    }
}
