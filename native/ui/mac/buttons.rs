use super::{bitmap_pixels, bitmap_rep};
use block2::StackBlock;
use eframe::egui::*;
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{
    NSAppearanceCustomization, NSApplication, NSButton, NSControl, NSSegmentDistribution,
    NSSegmentSwitchTracking, NSSegmentedControl,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSString};

#[derive(Clone)]
struct ControlImage {
    texture: TextureHandle,
    size: Vec2,
    offset: Vec2,
}

impl ControlImage {
    fn paint(&self, ui: &Ui, rect: Rect) {
        let mut painter = ui.painter().clone();
        // AppKit has already applied the disabled appearance.
        if !ui.is_enabled() {
            painter.set_opacity(1.);
        }
        painter.image(
            self.texture.id(),
            Rect::from_min_size(rect.min + self.offset, self.size),
            Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
            Color32::WHITE,
        );
    }
}

fn control_image(
    ui: &Ui,
    kind: &'static str,
    state: Id,
    size: Vec2,
    make: impl FnOnce(MainThreadMarker) -> Retained<NSControl>,
) -> Option<ControlImage> {
    let main = MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(main);
    let appearance = app.effectiveAppearance();
    let scale = ui.ctx().pixels_per_point();
    let enabled = ui.is_enabled();
    let id = Id::new((
        kind,
        state,
        appearance.name().to_string(),
        size.x.to_bits(),
        size.y.to_bits(),
        scale.to_bits(),
        enabled,
    ));
    if let Some(cached) = ui.data(|d| d.get_temp::<ControlImage>(id)) {
        return Some(cached);
    }

    let control = make(main);
    control.setAppearance(Some(&appearance));
    control.setEnabled(enabled);
    let alignment = NSRect::new(NSPoint::ZERO, NSSize::new(size.x.into(), size.y.into()));
    let frame = control.frameForAlignmentRect(alignment);
    control.setFrame(NSRect::new(NSPoint::ZERO, frame.size));
    control.layoutSubtreeIfNeeded();
    let width = (frame.size.width * f64::from(scale)).ceil() as isize;
    let height = (frame.size.height * f64::from(scale)).ceil() as isize;
    let rep = bitmap_rep(width, height)?;
    rep.setSize(frame.size);
    appearance.performAsCurrentDrawingAppearance(&StackBlock::new(|| {
        control.cacheDisplayInRect_toBitmapImageRep(control.bounds(), &rep);
    }));
    let rgba = bitmap_pixels(&rep)?;
    if !rgba.pixels().any(|p| p[3] != 0) {
        return None;
    }
    let texture = ui.ctx().load_texture(
        kind,
        ColorImage::from_rgba_unmultiplied([width as usize, height as usize], rgba.as_raw()),
        TextureOptions::NEAREST,
    );
    // Cocoa frames extend below the alignment rectangle for the native shadow.
    // egui's positive y direction runs from the top of the sheet downward.
    let offset = vec2(
        frame.origin.x as f32,
        size.y - (frame.origin.y + frame.size.height) as f32,
    );
    let image = ControlImage {
        texture,
        size: vec2(frame.size.width as f32, frame.size.height as f32),
        offset,
    };
    ui.data_mut(|d| d.insert_temp(id, image.clone()));
    Some(image)
}

fn button_image(ui: &Ui, title: &str, size: Vec2, pressed: bool) -> Option<ControlImage> {
    control_image(
        ui,
        "AppKit push button",
        Id::new((title, pressed)),
        size,
        |main| {
            // These views only draw. The dashboard handles actions and accessibility.
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(title),
                    None,
                    None,
                    main,
                )
            };
            button.setHighlighted(pressed);
            button.into_super()
        },
    )
}

pub fn draw_segments(
    ui: &Ui,
    bounds: Rect,
    items: &[(&str, &str)],
    current: &str,
    pressed: Option<usize>,
) -> bool {
    let image = control_image(
        ui,
        "AppKit segmented control",
        Id::new((items, current, pressed)),
        bounds.size(),
        |main| {
            let labels: Vec<_> = items
                .iter()
                .map(|(_, title)| NSString::from_str(title))
                .collect();
            let labels = NSArray::from_retained_slice(&labels);
            let control = unsafe {
                NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                    &labels,
                    NSSegmentSwitchTracking::SelectOne,
                    None,
                    None,
                    main,
                )
            };
            control.setSegmentDistribution(NSSegmentDistribution::FillEqually);
            let selected = pressed.or_else(|| items.iter().position(|(key, _)| *key == current));
            control.setSelectedSegment(selected.map_or(-1, |i| i as isize));
            control.setHighlighted(pressed.is_some());
            control.into_super()
        },
    );
    if let Some(image) = image {
        image.paint(ui, bounds);
        true
    } else {
        false
    }
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
    let normal = button_image(ui, title, size, false)?;
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let image = if response.is_pointer_button_down_on() {
        button_image(ui, title, size, true).unwrap_or(normal)
    } else {
        normal
    };
    let enabled = ui.is_enabled();
    image.paint(ui, rect);
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(1.),
            6.,
            Stroke::new(3.0_f32, focus),
            StrokeKind::Middle,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, title));
    Some(response)
}
