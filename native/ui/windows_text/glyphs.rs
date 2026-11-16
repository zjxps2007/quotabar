use std::{ffi::c_void, mem::ManuallyDrop, slice};
use windows::Win32::{
    Foundation::{E_NOTIMPL, E_POINTER},
    Graphics::{Direct2D::*, DirectWrite::*},
};
use windows_core::{BOOL, IUnknown, IUnknownImpl, Ref, Result, implement};
use windows_numerics::Vector2;

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
    target: ID2D1RenderTarget,
    brush: ID2D1SolidColorBrush,
    scale: f32,
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
        _: *const c_void,
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
        unsafe {
            self.target
                .DrawGlyphRun(Vector2 { X: x, Y: y }, &*adjusted, &self.brush, mode);
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
        // Send its glyphs through this same renderer as the text it follows.
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
    }
}

pub(super) fn draw(
    target: &ID2D1RenderTarget,
    brush: &ID2D1SolidColorBrush,
    layout: &IDWriteTextLayout,
    origin: Vector2,
    scale: f32,
) -> Result<()> {
    let renderer: IDWriteTextRenderer = GlyphPainter {
        target: target.clone(),
        brush: brush.clone(),
        scale,
    }
    .into();
    unsafe { layout.Draw(None, &renderer, origin.X, origin.Y) }
}

#[cfg(test)]
mod tests {
    use super::*;

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
