use std::{cell::Cell, ffi::c_void, mem::ManuallyDrop, slice};
use windows::Win32::{
    Foundation::{E_NOTIMPL, E_POINTER},
    Graphics::{Direct2D::*, DirectWrite::*},
};
use windows_core::{BOOL, ComObject, IUnknown, IUnknownImpl, Ref, Result, implement};
use windows_numerics::Vector2;

pub(super) fn mark_dash_runs(
    factory: &IDWriteFactory,
    layout: &IDWriteTextLayout,
    text: &[u16],
) -> Result<()> {
    // WPF starts a new glyph run around these dashes, and another for spaces
    // immediately after them. A DirectWrite drawing effect splits painting
    // while preserving the original shaping, kerning, wrapping and bidi order.
    let mut next = 0;
    while let Some(offset) = text[next..]
        .iter()
        .position(|ch| matches!(ch, 0x002d | 0x2011 | 0x2013 | 0x2014))
    {
        let start = next + offset;
        let end = start
            + text[start..]
                .iter()
                .take_while(|&&ch| ch == text[start])
                .count();
        next = end + text[end..].iter().take_while(|&&ch| ch == 0x20).count();
        for range in [start..end, end..next] {
            if !range.is_empty() {
                unsafe {
                    let marker = factory.CreateTypography()?;
                    layout.SetDrawingEffect(
                        &marker,
                        DWRITE_TEXT_RANGE {
                            startPosition: range.start as u32,
                            length: range.len() as u32,
                        },
                    )?;
                }
            }
        }
    }
    Ok(())
}

// WPF's ideal-mode formatter measures in 1/300 DIP and stores run advances
// and offsets in thousandths of an em for fonts up to 48 DIP. The layout
// engine keeps its own line widths, character clusters and wrapping.
fn glyph_metrics(em: f32, scale: f32) -> impl FnMut(f32) -> f32 {
    let (em, scale) = (f64::from(em), f64::from(scale));
    let mut compressed = em > 0. && em <= 48. * scale;
    move |value| {
        let dip = f64::from(value) / scale;
        let mut ideal = (dip * 300.).round_ties_even();
        if dip > 0. {
            ideal = ideal.max(1.);
        }
        let physical = ideal / 300. * scale;
        if compressed {
            let thousandths = physical / em * 1000.;
            if (f64::from(i16::MIN)..=f64::from(i16::MAX)).contains(&thousandths) {
                return (thousandths.round_ties_even() * em / 1000.) as f32;
            }
            // An overflow promotes this metric array to full precision from
            // this glyph onward. Already stored advances keep their rounding.
            compressed = false;
        }
        physical as f32
    }
}

#[implement(IDWriteTextRenderer)]
struct GlyphPainter {
    scale: f32,
    failed: Cell<bool>,
}

enum Destination<'a, 'b> {
    Bitmap(&'a ID2D1RenderTarget, &'a ID2D1SolidColorBrush),
    Mask(&'a mut Coverage<'b>),
}

impl IDWritePixelSnapping_Impl for GlyphPainter_Impl {
    fn IsPixelSnappingDisabled(&self, _: *const c_void) -> Result<BOOL> {
        Ok(false.into())
    }

    fn GetCurrentTransform(&self, _: *const c_void, matrix: *mut DWRITE_MATRIX) -> Result<()> {
        let matrix = unsafe { matrix.as_mut() }.ok_or(E_POINTER)?;
        *matrix = DWRITE_MATRIX {
            m11: 1.,
            m22: 1.,
            ..Default::default()
        };
        Ok(())
    }

    fn GetPixelsPerDip(&self, _: *const c_void) -> Result<f32> {
        // Our 96-DPI bitmap and its text layout both use physical pixels.
        Ok(1.)
    }
}

impl IDWriteTextRenderer_Impl for GlyphPainter_Impl {
    fn DrawGlyphRun(
        &self,
        context: *const c_void,
        x: f32,
        y: f32,
        mode: DWRITE_MEASURING_MODE,
        run: *const DWRITE_GLYPH_RUN,
        _: *const DWRITE_GLYPH_RUN_DESCRIPTION,
        _: Ref<'_, IUnknown>,
    ) -> Result<()> {
        let run = unsafe { run.as_ref() }.ok_or(E_POINTER)?;
        let count = run.glyphCount as usize;
        if count == 0 {
            return Ok(());
        }
        // DirectWrite owns the font and the shaping buffers until this callback
        // returns. Replace only the metrics in a borrowed copy of the run.
        let mut adjusted = ManuallyDrop::new(unsafe { std::ptr::read(run) });
        let advances: Vec<f32>;
        if !run.glyphAdvances.is_null() {
            advances = unsafe { slice::from_raw_parts(run.glyphAdvances, count) }
                .iter()
                .copied()
                .map(glyph_metrics(run.fontEmSize, self.scale))
                .collect();
            adjusted.glyphAdvances = advances.as_ptr();
        }
        let offsets: Vec<DWRITE_GLYPH_OFFSET>;
        if !run.glyphOffsets.is_null() {
            let mut advance = glyph_metrics(run.fontEmSize, self.scale);
            let mut ascender = glyph_metrics(run.fontEmSize, self.scale);
            offsets = unsafe { slice::from_raw_parts(run.glyphOffsets, count) }
                .iter()
                .map(|v| DWRITE_GLYPH_OFFSET {
                    advanceOffset: advance(v.advanceOffset),
                    ascenderOffset: ascender(v.ascenderOffset),
                })
                .collect();
            adjusted.glyphOffsets = offsets.as_ptr();
        }
        // DirectWrite calls us synchronously, including for inline objects.
        let output = unsafe { context.cast_mut().cast::<Destination<'_, '_>>().as_mut() }
            .ok_or(E_POINTER)?;
        match output {
            Destination::Bitmap(target, brush) => unsafe {
                target.DrawGlyphRun(Vector2 { X: x, Y: y }, &*adjusted, &**brush, mode);
            },
            Destination::Mask(mask) => {
                let origin = mask.run_origin(x, y, run, self.scale);
                mask.paint(&adjusted, origin, y, mode)
                    .inspect_err(|_| self.failed.set(true))?;
            }
        }
        Ok(())
    }

    fn DrawUnderline(
        &self,
        _: *const c_void,
        _: f32,
        _: f32,
        _: *const DWRITE_UNDERLINE,
        _: Ref<'_, IUnknown>,
    ) -> Result<()> {
        self.failed.set(true);
        Err(E_NOTIMPL.into())
    }
    fn DrawStrikethrough(
        &self,
        _: *const c_void,
        _: f32,
        _: f32,
        _: *const DWRITE_STRIKETHROUGH,
        _: Ref<'_, IUnknown>,
    ) -> Result<()> {
        self.failed.set(true);
        Err(E_NOTIMPL.into())
    }
    fn DrawInlineObject(
        &self,
        context: *const c_void,
        x: f32,
        y: f32,
        object: Ref<'_, IDWriteInlineObject>,
        sideways: BOOL,
        rtl: BOOL,
        effect: Ref<'_, IUnknown>,
    ) -> Result<()> {
        // DirectWrite supplies the trimming sign as a separate inline layout.
        // Keep its continuous layout with the preceding text when Direct2D
        // paints the sign, just as when that ellipsis is typed literally.
        if matches!(
            unsafe { context.cast::<Destination<'_, '_>>().as_ref() },
            Some(Destination::Mask(_))
        ) {
            self.failed.set(true);
            return Err(E_NOTIMPL.into());
        }
        unsafe {
            object.ok()?.Draw(
                (!context.is_null()).then_some(context),
                &self.to_interface::<IDWriteTextRenderer>(),
                x,
                y,
                sideways.as_bool(),
                rtl.as_bool(),
                effect.as_ref(),
            )
        }
        .inspect_err(|_| self.failed.set(true))
    }
}

fn draw_to(
    layout: &IDWriteTextLayout,
    origin: Vector2,
    scale: f32,
    mut destination: Destination<'_, '_>,
) -> Result<()> {
    let renderer = ComObject::new(GlyphPainter {
        scale,
        failed: Cell::new(false),
    });
    let context = (&raw mut destination).cast();
    unsafe {
        layout.Draw(
            Some(context),
            renderer.as_interface::<IDWriteTextRenderer>(),
            origin.X,
            origin.Y,
        )?;
    }
    // IDWriteTextLayout can return S_OK even if an inline drawing callback failed.
    if renderer.failed.get() {
        Err(E_NOTIMPL.into())
    } else {
        Ok(())
    }
}

pub(super) fn draw(
    target: &ID2D1RenderTarget,
    brush: &ID2D1SolidColorBrush,
    layout: &IDWriteTextLayout,
    origin: Vector2,
    scale: f32,
) -> Result<()> {
    draw_to(layout, origin, scale, Destination::Bitmap(target, brush))
}

struct Pen {
    y: f32,
    layout_end: f32,
    ideal_end: f64,
}

struct Coverage<'a> {
    factory: &'a super::Factories,
    params: &'a IDWriteRenderingParams,
    bgr: bool,
    rounding: u32,
    units: u16,
    size: [usize; 2],
    pixels: Vec<u16>,
    pen: Option<Pen>,
}

fn strip_weights(origin: f32, units: u16) -> (usize, u32) {
    let mut phase = (origin - origin.floor()) * 3.;
    if units > 1 {
        // The display samples to the left of each destination subpixel. Its
        // backward texture lookup rounds the forward shift up to a filter step.
        let steps = f32::from(units);
        phase = (phase * steps).ceil() / steps;
    }
    (phase as usize, (phase.fract() * 65536.) as u32)
}

impl Coverage<'_> {
    fn run_origin(&mut self, x: f32, y: f32, run: &DWRITE_GLYPH_RUN, scale: f32) -> f32 {
        // WPF sums ideal advances before storing them in a compressed GlyphRun.
        // A new line, tab or bidi reorder can give the next run a separate anchor.
        let start = self
            .pen
            .as_ref()
            .filter(|p| p.y == y && (p.layout_end - x).abs() < 0.001)
            .map_or(f64::from(x), |p| p.ideal_end);
        self.pen = None;
        if !run.glyphAdvances.is_null() {
            let advances =
                unsafe { slice::from_raw_parts(run.glyphAdvances, run.glyphCount as usize) };
            let units = 300. / f64::from(scale);
            let width: f64 = advances
                .iter()
                .map(|&v| {
                    let ideal = (f64::from(v) * units).round_ties_even();
                    if v > 0. { ideal.max(1.) } else { ideal }
                })
                .sum();
            let direction = if run.bidiLevel & 1 == 0 { 1. } else { -1. };
            self.pen = Some(Pen {
                y,
                layout_end: x + direction * advances.iter().sum::<f32>(),
                ideal_end: start + f64::from(direction) * width / units,
            });
        }
        start as f32
    }

    fn paint(
        &mut self,
        run: &DWRITE_GLYPH_RUN,
        x: f32,
        y: f32,
        mode: DWRITE_MEASURING_MODE,
    ) -> Result<()> {
        // WPF realizes each glyph run at (0, 0), applies enhanced contrast,
        // then positions its coverage strip at the fractional baseline origin.
        let analysis = unsafe {
            let rendering = run
                .fontFace
                .as_ref()
                .ok_or(E_POINTER)?
                .GetRecommendedRenderingMode(run.fontEmSize, 1., mode, self.params)?;
            self.factory
                .write
                .CreateGlyphRunAnalysis(run, 1., None, rendering, mode, 0., 0.)?
        };
        let aliased = unsafe { analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1)? };
        if aliased.right > aliased.left && aliased.bottom > aliased.top {
            // Embedded bitmap glyphs need Direct2D's aliased-text path.
            return Err(E_NOTIMPL.into());
        }
        let mut area = unsafe { analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_CLEARTYPE_3x1)? };
        let (left, baseline) = (x.floor() as i32, y.round() as i32);
        let [width, height] = self.size.map(|v| v as i32);
        // Include one source pixel to the left for subpixel interpolation.
        area.left = area.left.max(-left - 1);
        area.right = area.right.min(width - left);
        area.top = area.top.max(-baseline);
        area.bottom = area.bottom.min(height - baseline);
        if area.left >= area.right || area.top >= area.bottom {
            return Ok(());
        }
        let stride = 3 * (area.right - area.left) as usize;
        let mut strip = vec![0u8; stride * (area.bottom - area.top) as usize];
        unsafe { analysis.CreateAlphaTexture(DWRITE_TEXTURE_CLEARTYPE_3x1, &area, &mut strip)? };
        for value in &mut strip {
            *value = self.factory.contrast[usize::from(*value)];
        }
        let (shift, blend) = strip_weights(x, self.units);
        let full = 255 * u32::from(self.units);
        let dst_x = left + area.left;
        for (dy, row) in strip.chunks_exact(stride).enumerate() {
            let dst_row = (baseline + area.top + dy as i32) as usize * self.size[0];
            for sub in 0..stride + 3 {
                let px = dst_x + (sub / 3) as i32;
                if !(0..width).contains(&px) {
                    continue;
                }
                let sample = |offset| {
                    sub.checked_sub(offset)
                        .and_then(|i| row.get(i))
                        .copied()
                        .unwrap_or(0) as u32
                };
                let filtered = sample(shift) * (65536 - blend) + sample(shift + 1) * blend;
                let alpha = (filtered * u32::from(self.units) + self.rounding) >> 16;
                let channel = if self.bgr { sub % 3 } else { 2 - sub % 3 };
                let dst = &mut self.pixels[4 * (dst_row + px as usize) + channel];
                *dst = (alpha + ((full - alpha) * u32::from(*dst) + full / 2) / full) as u16;
            }
        }
        Ok(())
    }
}

pub(super) fn coverage(
    factory: &super::Factories,
    params: &IDWriteRenderingParams,
    layout: &IDWriteTextLayout,
    origin: Vector2,
    scale: f32,
    size: [usize; 2],
    key: &super::RunKey,
) -> Option<Vec<u16>> {
    let geometry = unsafe { params.GetPixelGeometry() };
    if unsafe { params.GetClearTypeLevel() } != 1.
        || !matches!(
            geometry,
            DWRITE_PIXEL_GEOMETRY_RGB | DWRITE_PIXEL_GEOMETRY_BGR
        )
    {
        return None;
    }
    let mut mask = Coverage {
        factory,
        params,
        size,
        bgr: geometry == DWRITE_PIXEL_GEOMETRY_BGR,
        rounding: key.subpixel_rounding(),
        units: key.coverage_units(),
        pixels: vec![0; size[0] * size[1] * 4],
        pen: None,
    };
    draw_to(layout, origin, scale, Destination::Mask(&mut mask)).ok()?;
    Some(mask.pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_filter_matches_the_wpf_license_text_origins() {
        // TextFormatter's origins after 27, 24, 3 and 6 leading spaces at
        // 13 DIPs, and the weights recovered from both themes of the live window.
        for (x, shift, numerator) in [(96.12, 0, 12), (85.44, 1, 11), (10.68, 2, 2), (21.36, 1, 3)]
        {
            assert_eq!(strip_weights(x, 32), (shift, numerator * 65536 / 32));
        }
        assert_eq!(strip_weights(-0.001, 32), (3, 0));
        assert_eq!(strip_weights(1., 32), (0, 0));
    }

    #[test]
    fn ideal_metrics_match_reference_glyphs_at_both_dpi_scales() {
        // WPF TextBlock("License", Segoe UI 13) exposes these actual advances.
        for scale in [1., 1.5, 2.] {
            for (design, advance) in [
                (6.1191406, 6.123),
                (3.1484375, 3.146),
                (5.5161133, 5.512),
                (0., 0.),
            ] {
                assert!(
                    (glyph_metrics(13. * scale, scale)(design * scale) - advance * scale).abs()
                        < 0.00001
                );
            }
        }
        let mut metric = glyph_metrics(13., 1.);
        assert_eq!(metric(512.), 512.);
        assert!((metric(6.1191406) - 6.12).abs() < 0.00001);
        assert!((glyph_metrics(72., 1.)(48.124) - 48.123333).abs() < 0.00001);
    }
}
