use super::{bitmap_pixels, bitmap_rep};
use eframe::egui::{emath::GuiRounding, *};
use objc2_app_kit::{NSColor, NSGraphicsContext};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CALayer, kCACornerCurveContinuous};

#[derive(Clone, Copy, Hash, Eq, PartialEq)]
pub enum Curve {
    Continuous,
    Circular,
}

fn mask(
    ui: &Ui,
    pixels: [usize; 2],
    radius: f32,
    stroke: f32,
    curve: Curve,
) -> Option<TextureHandle> {
    // Equal-sized views share a backing mask. Replace it when resizing
    // changes the effective radius at this size.
    let key = Id::new(("AppKit rounded mask", pixels, stroke.to_bits(), curve));
    if let Some((cached_radius, texture)) = ui.data(|d| d.get_temp::<(f32, TextureHandle)>(key))
        && cached_radius == radius
    {
        return Some(texture);
    }
    let [w, h] = pixels;
    // The 1.2-point "today" outline straddles the cell boundary. Keep both
    // halves when rasterizing it, including on a 1x display.
    let samples = if stroke > 0. { 10 } else { 1 };
    let factor = samples as f64;
    let canvas = NSRect::new(
        NSPoint::ZERO,
        NSSize::new((w * samples) as f64, (h * samples) as f64),
    );
    let rep = bitmap_rep((w * samples) as isize, (h * samples) as isize)?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    let shape = CALayer::layer();
    shape.setBounds(canvas);
    if curve == Curve::Continuous {
        shape.setCornerCurve(unsafe { kCACornerCurveContinuous });
    }
    shape.setCornerRadius(f64::from(radius) * factor);
    let white = NSColor::whiteColor().CGColor();
    if stroke > 0. {
        let half = f64::from(stroke) / 2.;
        let inset = (half.ceil() - half) * factor;
        shape.setFrame(NSRect::new(
            NSPoint::new(inset, inset),
            NSSize::new(
                canvas.size.width - 2. * inset,
                canvas.size.height - 2. * inset,
            ),
        ));
        shape.setBorderColor(Some(&white));
        shape.setBorderWidth(f64::from(stroke) * factor);
        let container = CALayer::layer();
        container.setBounds(canvas);
        container.addSublayer(&shape);
        container.renderInContext(&context.CGContext());
    } else {
        shape.setBackgroundColor(Some(&white));
        shape.renderInContext(&context.CGContext());
    }
    let image = bitmap_pixels(&rep)?;
    let image = if samples > 1 {
        image::imageops::thumbnail(&image, w as u32, h as u32)
    } else {
        image
    };
    if !image.pixels().any(|p| p[3] != 0) {
        return None;
    }
    let alpha = image
        .pixels()
        .map(|p| Color32::from_white_alpha(p[3]))
        .collect();
    let texture = ui.ctx().load_texture(
        "Rounded shape",
        ColorImage::new(pixels, alpha),
        TextureOptions::NEAREST,
    );
    ui.data_mut(|d| d.insert_temp(key, (radius, texture.clone())));
    Some(texture)
}

pub fn rounded_rect(ui: &Ui, rect: Rect, radius: f32, fill: Color32, stroke: Stroke, curve: Curve) {
    if !ui.is_rect_visible(rect.expand(stroke.width)) {
        return;
    }
    let scale = ui.ctx().pixels_per_point();
    let rect = rect.round_to_pixels(scale);
    let paint = |width: f32, color: Color32| {
        let half = width / 2.;
        let bounds = rect.expand((half * scale).ceil() / scale);
        let size = (bounds.size() * scale).round();
        if let Some(texture) = mask(
            ui,
            [size.x as usize, size.y as usize],
            (radius + half) * scale,
            width * scale,
            curve,
        ) {
            ui.painter().image(
                texture.id(),
                bounds,
                Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                color,
            );
        } else if width > 0. {
            ui.painter().rect_stroke(
                rect,
                radius.round() as u8,
                Stroke::new(width, color),
                StrokeKind::Middle,
            );
        } else {
            ui.painter()
                .rect_filled(bounds, radius.round() as u8, color);
        }
    };
    paint(0., fill);
    if stroke.width > 0. {
        paint(stroke.width, stroke.color);
    }
}
