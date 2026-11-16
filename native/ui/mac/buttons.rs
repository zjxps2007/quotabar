use super::{bitmap_pixels, bitmap_rep};
use block2::StackBlock;
use eframe::egui::*;
use objc2::{MainThreadMarker, MainThreadOnly, msg_send, rc::Retained, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSApplication, NSButton, NSCellImagePosition, NSColor,
    NSControl, NSControlSize, NSFont, NSImage, NSPopUpButton, NSSegmentDistribution,
    NSSegmentSwitchTracking, NSSegmentedControl, NSSwitch, NSUserInterfaceLayoutDirection,
};
use objc2_foundation::{NSArray, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

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
    tint: Option<Color32>,
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
        tint,
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
    let switch = control.downcast_ref::<NSSwitch>().is_some();
    let accent_control = switch || control.downcast_ref::<NSPopUpButton>().is_some();
    let tinted: Option<Retained<NSAppearance>> =
        if accent_control && appearance.respondsToSelector(sel!(appearanceByApplyingTintColor:)) {
            let color = tint.unwrap_or(if ui.visuals().dark_mode {
                Color32::WHITE
            } else {
                Color32::BLACK
            });
            let [r, g, b] = [color.r(), color.g(), color.b()].map(|v| f64::from(v) / 255.);
            let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.);
            // AppKit's per-appearance tint is also used by WebKit for native form
            // controls. It is SPI: check availability and scope it to this control.
            unsafe { msg_send![&*appearance, appearanceByApplyingTintColor: &*color] }
        } else {
            None
        };
    let native_tint = tinted.is_some();
    let appearance = tinted.as_deref().unwrap_or(&appearance);
    control.setAppearance(Some(appearance));
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
    let mut rgba = bitmap_pixels(&rep)?;
    if enabled && accent_control && !native_tint {
        // Older systems can still draw the thumb, shadow and shading with the
        // system accent. Recolor that mask when a tinted appearance is unavailable.
        let rtl =
            control.userInterfaceLayoutDirection() == NSUserInterfaceLayoutDirection::RightToLeft;
        let (width, height) = rgba.dimensions();
        // The provider tint is sRGB. Weight the native accent mask with its
        // linear components, preserving the neutral thumb, highlights and shadow.
        let [red, green, blue] = tint.map_or_else(
            || [if ui.visuals().dark_mode { 92 } else { 0 }; 3],
            |color| {
                [color.r(), color.g(), color.b()].map(|channel| {
                    (eframe::egui::ecolor::linear_f32_from_gamma_u8(channel) * 255.).round() as u8
                })
            },
        );
        for (x, _, pixel) in rgba.enumerate_pixels_mut() {
            let trailing = if rtl { x } else { width - 1 - x };
            if switch || trailing < height {
                let [r, g, b, a] = pixel.0;
                let low = r.min(g).min(b);
                let range = u16::from(r.max(g).max(b) - low);
                let shade = |channel: u8| low + ((range * u16::from(channel) + 127) / 255) as u8;
                pixel.0 = [shade(red), shade(green), shade(blue), a];
            }
        }
    }
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

fn push_button(
    main: MainThreadMarker,
    title: &str,
    symbol: Option<&str>,
    control_size: NSControlSize,
    font_size: Option<f32>,
) -> Retained<NSButton> {
    // The native button draws its symbol, bezel and title together; egui owns
    // the dashboard action and its accessible hit target.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(&NSString::from_str(title), None, None, main)
    };
    if control_size != NSControlSize::Regular {
        button.setControlSize(control_size);
    }
    if let Some(points) = font_size.map(f64::from).or_else(|| {
        (control_size != NSControlSize::Regular)
            .then(|| NSFont::systemFontSizeForControlSize(control_size))
    }) {
        button.setFont(Some(&NSFont::systemFontOfSize(points)));
    }
    if let Some(icon) = symbol.and_then(|name| {
        NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)
    }) {
        button.setImage(Some(&icon));
        button.setImagePosition(NSCellImagePosition::ImageLeading);
    }
    button
}

fn button_image(
    ui: &Ui,
    title: &str,
    symbol: Option<&str>,
    control_size: NSControlSize,
    font_size: Option<f32>,
    size: Vec2,
    pressed: bool,
) -> Option<ControlImage> {
    control_image(
        ui,
        "AppKit push button",
        Id::new((
            title,
            symbol,
            control_size,
            font_size.map(f32::to_bits),
            pressed,
        )),
        size,
        None,
        |main| {
            let button = push_button(main, title, symbol, control_size, font_size);
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
        None,
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

pub fn draw_switch(
    ui: &Ui,
    bounds: Rect,
    checked: bool,
    pressed: bool,
    size: NSControlSize,
    tint: Option<Color32>,
) -> bool {
    let image = control_image(
        ui,
        "AppKit switch",
        Id::new((size, checked, pressed)),
        bounds.size(),
        tint,
        |main| {
            let control = NSSwitch::new(main);
            control.setControlSize(size);
            control.setState(isize::from(checked));
            control.setHighlighted(pressed);
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

pub fn draw_checkbox(ui: &Ui, bounds: Rect, checked: bool, pressed: bool) -> bool {
    let image = control_image(
        ui,
        "AppKit checkbox",
        Id::new((checked, pressed)),
        bounds.size(),
        None,
        |main| {
            let button = unsafe {
                NSButton::checkboxWithTitle_target_action(&NSString::new(), None, None, main)
            };
            button.setState(isize::from(checked));
            button.setHighlighted(pressed);
            button.into_super()
        },
    );
    if let Some(image) = image {
        image.paint(ui, bounds);
        true
    } else {
        false
    }
}

fn popup_button(
    main: MainThreadMarker,
    labels: &[impl AsRef<str>],
    selected: usize,
) -> Retained<NSPopUpButton> {
    let button =
        NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(main), NSRect::ZERO, false);
    let titles: Vec<_> = labels
        .iter()
        .map(|value| NSString::from_str(value.as_ref()))
        .collect();
    button.addItemsWithTitles(&NSArray::from_retained_slice(&titles));
    button.selectItemAtIndex(selected as isize);
    button
}

pub fn popup_choice(
    ui: &mut Ui,
    name: &str,
    width: f32,
    labels: &[impl AsRef<str>],
    selected: usize,
    size: f32,
) -> Option<usize> {
    let title = labels.get(selected)?.as_ref();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 22.), Sense::click());
    let enabled = ui.is_enabled();
    response.widget_info(|| {
        let mut info = WidgetInfo::labeled(WidgetType::ComboBox, enabled, name);
        info.current_text_value = Some(title.to_owned());
        info
    });
    if let Some(image) = control_image(
        ui,
        "AppKit pop-up button",
        Id::new(title),
        rect.size(),
        None,
        |main| {
            popup_button(main, labels, selected)
                .into_super()
                .into_super()
        },
    ) {
        image.paint(ui, rect);
    } else {
        let visuals = ui.style().interact(&response);
        ui.painter().rect(
            rect,
            5.,
            visuals.bg_fill,
            visuals.bg_stroke,
            StrokeKind::Inside,
        );
        ui.painter().text(
            rect.left_center() + vec2(6., 0.),
            Align2::LEFT_CENTER,
            title,
            FontId::proportional(size),
            visuals.text_color(),
        );
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(1.),
            6.,
            Stroke::new(3.0_f32, ui.visuals().selection.stroke.color),
            StrokeKind::Middle,
        );
    }
    let keyboard = enabled
        && response.has_focus()
        && ui.input_mut(|i| {
            i.consume_key(Modifiers::NONE, Key::ArrowDown)
                || i.consume_key(Modifiers::ALT, Key::ArrowDown)
        });
    if !(enabled && (response.clicked() || keyboard)) {
        return None;
    }
    let main = MainThreadMarker::new()?;
    let view = NSApplication::sharedApplication(main)
        .keyWindow()?
        .contentView()?;
    let zoom = f64::from(ui.ctx().zoom_factor());
    let bounds = view.bounds();
    let x = bounds.origin.x + f64::from(rect.left()) * zoom;
    let y = bounds.origin.y
        + if view.isFlipped() {
            f64::from(rect.top()) * zoom
        } else {
            bounds.size.height - f64::from(rect.bottom()) * zoom
        };
    let alignment = NSRect::new(
        NSPoint::new(x, y),
        NSSize::new(
            f64::from(rect.width()) * zoom,
            f64::from(rect.height()) * zoom,
        ),
    );
    let button = popup_button(main, labels, selected);
    button.setFrame(button.frameForAlignmentRect(alignment));
    view.addSubview(&button);
    // AppKit tracks the menu synchronously, including Escape, arrow keys and type selection.
    unsafe {
        button.performClick(None);
    }
    button.removeFromSuperview();
    let index = usize::try_from(button.indexOfSelectedItem()).ok()?;
    (index < labels.len() && index != selected).then_some(index)
}

pub fn standard_button(
    ui: &mut Ui,
    title: &str,
    symbol: Option<&str>,
    control_size: NSControlSize,
    font_size: Option<f32>,
    focus: Color32,
) -> Option<Response> {
    let size = if symbol.is_some() || control_size != NSControlSize::Regular {
        let main = MainThreadMarker::new()?;
        let key = Id::new((
            "AppKit button alignment size",
            title,
            symbol,
            control_size,
            font_size.map(f32::to_bits),
        ));
        ui.data_mut(|d| {
            *d.get_temp_mut_or_insert_with(key, || {
                let button = push_button(main, title, symbol, control_size, font_size);
                button.sizeToFit();
                let size = button.alignmentRectForFrame(button.frame()).size;
                vec2(size.width as f32, size.height as f32)
            })
        })
    } else {
        let points = font_size.unwrap_or(13.);
        let label = crate::mac_text::natural_size(ui, title, points, false)?;
        vec2(
            label.x.ceil() + 16.,
            label
                .y
                .max(crate::mac_text::system_line_height(ui, f64::from(points)))
                .ceil()
                + 6.,
        )
    };
    let normal = button_image(ui, title, symbol, control_size, font_size, size, false)?;
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let image = if response.is_pointer_button_down_on() {
        button_image(ui, title, symbol, control_size, font_size, size, true).unwrap_or(normal)
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
