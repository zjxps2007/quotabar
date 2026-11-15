use super::{bitmap_pixels, bitmap_rep};
use block2::StackBlock;
use eframe::egui::*;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSAppearanceCustomization, NSApplication, NSButton};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

#[derive(Clone)]
struct ButtonImage {
    texture: TextureHandle,
    size: Vec2,
    offset: Vec2,
}

fn image(ui: &Ui, title: &str, size: Vec2, pressed: bool) -> Option<ButtonImage> {
    let main = MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(main);
    let appearance = app.effectiveAppearance();
    let scale = ui.ctx().pixels_per_point();
    let enabled = ui.is_enabled();
    let id = Id::new((
        "AppKit push button",
        title,
        appearance.name().to_string(),
        size.x.to_bits(),
        size.y.to_bits(),
        scale.to_bits(),
        enabled,
        pressed,
    ));
    if let Some(cached) = ui.data(|d| d.get_temp::<ButtonImage>(id)) {
        return Some(cached);
    }

    // The view is used only for drawing. The dashboard owns hit testing, focus,
    // accessibility and shortcuts, just as it does for the other sheet controls.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(&NSString::from_str(title), None, None, main)
    };
    button.setAppearance(Some(&appearance));
    button.setEnabled(enabled);
    button.setHighlighted(pressed);
    let alignment = NSRect::new(NSPoint::ZERO, NSSize::new(size.x.into(), size.y.into()));
    let frame = button.frameForAlignmentRect(alignment);
    button.setFrame(NSRect::new(NSPoint::ZERO, frame.size));
    button.layoutSubtreeIfNeeded();
    let width = (frame.size.width * f64::from(scale)).ceil() as isize;
    let height = (frame.size.height * f64::from(scale)).ceil() as isize;
    let rep = bitmap_rep(width, height)?;
    rep.setSize(frame.size);
    appearance.performAsCurrentDrawingAppearance(&StackBlock::new(|| {
        button.cacheDisplayInRect_toBitmapImageRep(button.bounds(), &rep);
    }));
    let rgba = bitmap_pixels(&rep)?;
    if !rgba.pixels().any(|p| p[3] != 0) {
        return None;
    }
    let texture = ui.ctx().load_texture(
        "AppKit push button",
        ColorImage::from_rgba_unmultiplied([width as usize, height as usize], rgba.as_raw()),
        TextureOptions::NEAREST,
    );
    // Cocoa frames extend below the alignment rectangle for the native shadow.
    // egui's positive y direction runs from the top of the sheet downward.
    let offset = vec2(
        frame.origin.x as f32,
        size.y - (frame.origin.y + frame.size.height) as f32,
    );
    let image = ButtonImage {
        texture,
        size: vec2(frame.size.width as f32, frame.size.height as f32),
        offset,
    };
    ui.data_mut(|d| d.insert_temp(id, image.clone()));
    Some(image)
}

pub fn standard_button(ui: &mut Ui, title: &str, focus: Color32) -> Option<Response> {
    let label = crate::mac_text::natural_size(ui, title, 13., false)?;
    let size = vec2(
        label.x.ceil() + 16.,
        label
            .y
            .max(crate::mac_text::system_line_height(ui, 13.))
            .ceil()
            + 6.,
    );
    let normal = image(ui, title, size, false)?;
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let image = if response.is_pointer_button_down_on() {
        image(ui, title, size, true).unwrap_or(normal)
    } else {
        normal
    };
    let enabled = ui.is_enabled();
    let mut painter = ui.painter().clone();
    if !enabled {
        painter.set_opacity(1.);
    }
    painter.image(
        image.texture.id(),
        Rect::from_min_size(rect.min + image.offset, image.size),
        Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
        Color32::WHITE,
    );
    if response.has_focus() {
        painter.rect_stroke(
            rect.expand(1.),
            6.,
            Stroke::new(3.0_f32, focus),
            StrokeKind::Middle,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, title));
    Some(response)
}
