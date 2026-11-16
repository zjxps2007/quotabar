use super::{bitmap_pixels, bitmap_rep};
use eframe::egui::{emath::GuiRounding, *};
use objc2_app_kit::{NSColor, NSGraphicsContext};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CALayer, kCACornerCurveContinuous};

fn mask(ui: &Ui, pixels: [usize; 2], radius: f32, stroke: f32) -> Option<TextureHandle> {
    // A grid uses at most two rounded device-pixel widths and heights. Replace
    // each mask when resizing changes its radius, and share it across dates.
    let key = Id::new(("calendar corner mask", pixels, stroke.to_bits()));
    if let Some((cached_radius, texture)) = ui.data(|d| d.get_temp::<(f32, TextureHandle)>(key))
        && cached_radius == radius
    {
        return Some(texture);
    }
    let [w, h] = pixels;
    let rep = bitmap_rep(w as isize, h as isize)?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    let shape = CALayer::layer();
    shape.setBounds(NSRect::new(NSPoint::ZERO, NSSize::new(w as f64, h as f64)));
    shape.setCornerCurve(unsafe { kCACornerCurveContinuous });
    shape.setCornerRadius(f64::from(radius));
    let white = NSColor::whiteColor().CGColor();
    if stroke > 0. {
        shape.setBorderColor(Some(&white));
        shape.setBorderWidth(f64::from(stroke));
    } else {
        shape.setBackgroundColor(Some(&white));
    }
    shape.renderInContext(&context.CGContext());
    let image = bitmap_pixels(&rep)?;
    if !image.pixels().any(|p| p[3] != 0) {
        return None;
    }
    let alpha = image
        .pixels()
        .map(|p| Color32::from_white_alpha(p[3]))
        .collect();
    let texture = ui.ctx().load_texture(
        "Calendar cell",
        ColorImage::new(pixels, alpha),
        TextureOptions::NEAREST,
    );
    ui.data_mut(|d| d.insert_temp(key, (radius, texture.clone())));
    Some(texture)
}

pub fn paint_cell(ui: &Ui, rect: Rect, radius: f32, fill: Color32, stroke: Stroke) {
    if !ui.is_rect_visible(rect.expand(stroke.width)) {
        return;
    }
    let scale = ui.ctx().pixels_per_point();
    let paint = |bounds: Rect, radius: f32, width: f32, color: Color32| {
        let bounds = bounds.round_to_pixels(scale);
        let size = (bounds.size() * scale).round();
        if let Some(texture) = mask(
            ui,
            [size.x as usize, size.y as usize],
            radius * scale,
            width * scale,
        ) {
            ui.painter().image(
                texture.id(),
                bounds,
                Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                color,
            );
        } else if width > 0. {
            ui.painter().rect_stroke(
                bounds,
                radius.round() as u8,
                Stroke::new(width, color),
                StrokeKind::Inside,
            );
        } else {
            ui.painter()
                .rect_filled(bounds, radius.round() as u8, color);
        }
    };
    paint(rect, radius, 0., fill);
    if stroke.width > 0. {
        let half = stroke.width / 2.;
        paint(rect.expand(half), radius + half, stroke.width, stroke.color);
    }
}
