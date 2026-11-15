use eframe::egui::{self, *};
#[cfg(not(target_os = "macos"))]
use font_kit::{family_name::FamilyName, handle::Handle, properties::Properties};
use font_kit::{properties::Weight, source::SystemSource};
use std::sync::Arc;

fn bold() -> FontFamily {
    FontFamily::Name("system-semibold".into())
}

#[cfg(target_os = "macos")]
fn system_face(_: &SystemSource, title: &str, weight: Weight) -> Option<FontData> {
    crate::mac_fonts::load(title, weight == Weight::SEMIBOLD)
}

#[cfg(not(target_os = "macos"))]
fn system_face(source: &SystemSource, title: &str, weight: Weight) -> Option<FontData> {
    let props = Properties {
        weight,
        ..Default::default()
    };
    let handle = source
        .select_best_match(&[FamilyName::Title(title.to_owned())], &props)
        .ok()?;
    let (data, index) = match handle {
        Handle::Path { path, font_index } => (std::fs::read(path).ok()?, font_index),
        Handle::Memory { bytes, font_index } => (bytes.as_ref().clone(), font_index),
    };
    let mut face = FontData::from_owned(data);
    face.index = index;
    Some(face)
}

pub fn fonts(ctx: &Context) {
    let source = SystemSource::new();
    let mut defs = FontDefinitions::default();
    let fallback = defs.families[&FontFamily::Proportional].clone();
    let fixed_fallback = defs.families[&FontFamily::Monospace].clone();
    let sans = if cfg!(windows) {
        ["Segoe UI", "Malgun Gothic"]
    } else {
        [".AppleSystemUIFont", "Apple SD Gothic Neo"]
    };
    let fixed = if cfg!(windows) {
        ["Consolas", "Malgun Gothic"]
    } else {
        ["Menlo", "Apple SD Gothic Neo"]
    };
    for (family, weight, candidates, fallback) in [
        (FontFamily::Proportional, Weight::NORMAL, sans, &fallback),
        (bold(), Weight::SEMIBOLD, sans, &fallback),
        (
            FontFamily::Monospace,
            Weight::NORMAL,
            fixed,
            &fixed_fallback,
        ),
    ] {
        let mut names = Vec::new();
        for title in candidates {
            let name = format!("{title}-{}", weight.0);
            if defs.font_data.contains_key(&name) {
                names.push(name);
                continue;
            }
            let Some(face) = system_face(&source, title, weight) else {
                continue;
            };
            defs.font_data.insert(name.clone(), Arc::new(face));
            names.push(name);
        }
        names.extend_from_slice(fallback);
        defs.families.insert(family, names);
    }
    ctx.set_fonts(defs);
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub ink: Color32,
    pub muted: Color32,
    pub tertiary: Color32,
    pub card: Color32,
    pub bg: Color32,
    pub rail: Color32,
    pub line: Color32,
    pub track: Color32,
    pub accent: Color32,
    pub on: Color32,
    pub warning: Color32,
}
impl Palette {
    pub fn new(ctx: &Context, appearance: &str) -> Self {
        let dark = match appearance {
            "dark" => true,
            "light" => false,
            _ => ctx.system_theme() == Some(Theme::Dark),
        };
        let gray = |d, l| Color32::from_gray(if dark { d } else { l });
        #[cfg(target_os = "macos")]
        let (ink, muted, tertiary) = {
            use block2::StackBlock;
            use objc2::MainThreadMarker;
            use objc2_app_kit::{NSApplication, NSColor};
            use std::cell::Cell;

            let app = NSApplication::sharedApplication(
                MainThreadMarker::new().expect("dashboard UI thread"),
            );
            let appearance = app.effectiveAppearance();
            let key = Id::new(("AppKit label colors", appearance.name().to_string()));
            ctx.data(|data| data.get_temp(key)).unwrap_or_else(|| {
                let colors = Cell::new((gray(220, 39), gray(151, 128), gray(84, 189)));
                appearance.performAsCurrentDrawingAppearance(&StackBlock::new(|| {
                    let (primary, secondary, tertiary) = colors.get();
                    colors.set((
                        mac_color(&NSColor::labelColor(), primary),
                        mac_color(&NSColor::secondaryLabelColor(), secondary),
                        mac_color(&NSColor::tertiaryLabelColor(), tertiary),
                    ));
                }));
                let colors = colors.get();
                ctx.data_mut(|data| data.insert_temp(key, colors));
                colors
            })
        };
        #[cfg(windows)]
        let (ink, muted, tertiary) = (gray(242, 23), gray(163, 117), gray(163, 117));
        Self {
            dark,
            ink,
            muted,
            tertiary,
            card: gray(28, 255),
            bg: gray(14, 245),
            rail: gray(19, 255),
            line: gray(45, 235),
            track: gray(42, 240),
            accent: gray(255, 0),
            on: gray(0, 255),
            warning: if dark {
                Color32::from_rgb(245, 163, 79)
            } else {
                Color32::from_rgb(184, 103, 8)
            },
        }
    }
    pub fn apply(self, ctx: &Context) {
        let mut v = if self.dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        v.override_text_color = Some(self.ink);
        if cfg!(windows) {
            v.disabled_alpha = 0.45;
        }
        v.panel_fill = self.bg;
        v.window_fill = self.bg;
        v.window_stroke = Stroke::new(1.0_f32, self.line);
        v.extreme_bg_color = self.card;
        v.faint_bg_color = self.track;
        v.selection.bg_fill = self.muted.gamma_multiply(0.4);
        v.selection.stroke = Stroke::new(1.0_f32, self.ink);
        v.hyperlink_color = self.ink;
        for (widget, bg) in [
            (&mut v.widgets.inactive, self.card),
            (&mut v.widgets.noninteractive, self.card),
            (&mut v.widgets.hovered, self.track),
            (&mut v.widgets.active, self.track),
            (&mut v.widgets.open, self.track),
        ] {
            widget.bg_fill = bg;
            widget.weak_bg_fill = bg;
            widget.bg_stroke = Stroke::new(1.0_f32, self.line);
            widget.corner_radius = CornerRadius::same(7);
            widget.fg_stroke = Stroke::new(1.0_f32, self.ink);
            widget.expansion = 0.;
        }
        ctx.set_visuals(v);
        ctx.style_mut(|style| {
            style
                .text_styles
                .insert(TextStyle::Body, FontId::proportional(12.));
            style
                .text_styles
                .insert(TextStyle::Button, FontId::proportional(12.));
            style.spacing.item_spacing = vec2(8., 0.);
            style.spacing.button_padding = vec2(12., 7.);
            style.spacing.interact_size = if cfg!(target_os = "macos") {
                vec2(0., 15.)
            } else {
                vec2(32., 28.)
            };
            style.spacing.scroll = egui::style::ScrollStyle::solid();
            if cfg!(windows) {
                style.spacing.scroll.bar_width = 13.;
                style.spacing.scroll.bar_inner_margin = 2.;
                style.spacing.scroll.bar_outer_margin = 2.;
                style.visuals.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 1. };
            }
        });
    }
    pub fn tint(self, p: &str) -> Color32 {
        let rgb = match (p, self.dark) {
            ("Claude", true) => [230, 130, 74],
            ("Claude", false) => [168, 79, 38],
            ("Gemini", true) => [138, 180, 248],
            ("Gemini", false) => [26, 115, 232],
            (_, true) => [137, 149, 255],
            (_, false) => [79, 92, 200],
        };
        Color32::from_rgb(rgb[0], rgb[1], rgb[2])
    }
    pub fn frame(self, pad: i8) -> Frame {
        Frame::new()
            .fill(self.card)
            .stroke(Stroke::new(1.0_f32, self.line))
            .corner_radius(16)
            .inner_margin(pad)
    }
}
#[cfg(target_os = "macos")]
fn mac_color(value: &objc2_app_kit::NSColor, fallback: Color32) -> Color32 {
    let Some(color) = value.colorUsingColorSpace(&objc2_app_kit::NSColorSpace::sRGBColorSpace())
    else {
        return fallback;
    };
    let byte = |n: f64| (n.clamp(0., 1.) * 255.).round() as u8;
    Color32::from_rgba_unmultiplied(
        byte(color.redComponent()),
        byte(color.greenComponent()),
        byte(color.blueComponent()),
        byte(color.alphaComponent()),
    )
}

#[cfg(target_os = "macos")]
pub fn mac_field(
    ui: &mut Ui,
    text: &mut String,
    hint: &str,
    id: &str,
    width: f32,
    p: Palette,
) -> Response {
    use objc2_app_kit::NSColor;
    let placeholder = mac_color(&NSColor::placeholderTextColor(), p.muted);
    let background = ui.painter().add(Shape::Noop);
    let field = ui.add(
        TextEdit::singleline(text)
            .id(Id::new(id))
            .hint_text(rich(hint, 13., false, placeholder))
            .font(FontId::proportional(13.))
            .frame(false)
            .margin(Margin::symmetric(10, 0))
            .vertical_align(Align::Center)
            .min_size(vec2(0., 22.))
            .desired_width((width - 20.).max(0.)),
    );
    ui.painter().set(
        background,
        epaint::RectShape::new(
            field.rect,
            6.,
            p.card,
            Stroke::new(1.0_f32, p.line),
            StrokeKind::Inside,
        ),
    );
    if field.has_focus() {
        let ring = mac_color(&NSColor::keyboardFocusIndicatorColor(), p.ink);
        ui.painter().rect_stroke(
            field.rect.expand(1.),
            6.,
            Stroke::new(3.0_f32, ring),
            StrokeKind::Middle,
        );
    }
    field
}

pub fn rich(s: impl Into<String>, size: f32, heavy: bool, c: Color32) -> RichText {
    RichText::new(s)
        .font(FontId::new(
            size,
            if heavy {
                bold()
            } else {
                FontFamily::Proportional
            },
        ))
        .color(c)
}
pub fn label(ui: &mut Ui, s: impl Into<String>, size: f32, heavy: bool, c: Color32) -> Response {
    let s = s.into();
    #[cfg(target_os = "macos")]
    if let Some(response) = crate::mac_text::label(ui, &s, size, heavy, c) {
        return response;
    }
    #[cfg(windows)]
    if let Some(response) = crate::windows_text::draw_label(ui, &s, size, heavy, c, false) {
        return response;
    }
    ui.label(rich(s, size, heavy, c))
}
pub fn caption(ui: &mut Ui, s: impl Into<String>, p: Palette) -> Response {
    label(ui, s, 11., false, p.muted)
}

pub fn disclosure(
    ui: &mut Ui,
    key: impl std::hash::Hash,
    title: &str,
    size: f32,
    p: Palette,
    body: impl FnOnce(&mut Ui),
) {
    ui.scope(|ui| {
        let header = CollapsingHeader::new(rich(title, size, false, p.muted)).id_salt(key);
        if cfg!(windows) {
            ui.spacing_mut().button_padding = vec2(0., 2.);
            ui.spacing_mut().interact_size.y = 0.;
            ui.spacing_mut().indent = 15.;
            header
                .icon(move |ui, openness, response| {
                    let center = response.rect.center() - vec2(4., 0.);
                    let turn = emath::Rot2::from_angle(openness * std::f32::consts::FRAC_PI_2);
                    let vertices = [vec2(-2., -4.), vec2(2., 0.), vec2(-2., 4.)];
                    ui.painter().add(Shape::line(
                        vertices.map(|v| center + turn * v).to_vec(),
                        Stroke::new(1.2_f32, p.muted),
                    ));
                })
                .show_unindented(ui, |ui| {
                    ui.add_space(6.);
                    body(ui);
                });
        } else {
            header.show(ui, body);
        }
    });
}
#[cfg(windows)]
pub use crate::windows_choice::choose;
#[cfg(target_os = "macos")]
pub fn mac_scroll(ui: &mut Ui, scroll: ScrollArea, p: Palette, body: impl FnOnce(&mut Ui)) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSScroller, NSScrollerStyle};
    let content_style = ui.style().clone();
    ui.scope(|ui| {
        let style = ui.style_mut();
        if MainThreadMarker::new()
            .is_some_and(|m| NSScroller::preferredScrollerStyle(m) == NSScrollerStyle::Overlay)
        {
            style.spacing.scroll = egui::style::ScrollStyle::floating();
            style.spacing.scroll.dormant_handle_opacity = 0.;
        } else {
            style.spacing.scroll = egui::style::ScrollStyle::solid();
            style.spacing.scroll.bar_width = 8.;
            style.spacing.scroll.bar_inner_margin = 4.;
            style.spacing.scroll.bar_outer_margin = 3.;
        }
        let knob = if p.dark {
            Color32::from_gray(102)
        } else {
            Color32::from_gray(195)
        };
        style.visuals.widgets.inactive.bg_fill = knob;
        style.visuals.widgets.hovered.bg_fill = knob.lerp_to_gamma(p.ink, 0.15);
        style.visuals.widgets.active.bg_fill = knob.lerp_to_gamma(p.ink, 0.3);
        style.visuals.extreme_bg_color = if p.dark {
            p.rail
        } else {
            Color32::from_gray(250)
        };
        scroll.show(ui, |ui| {
            ui.set_style(content_style);
            body(ui);
        });
    });
}

#[cfg(target_os = "macos")]
pub fn choose(
    ui: &mut Ui,
    name: &str,
    width: f32,
    labels: &[impl AsRef<str>],
    selected: usize,
    size: f32,
) -> Option<usize> {
    let mut index = selected;
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = vec2(6., 2.);
        ui.spacing_mut().icon_width = 16.;
        ui.spacing_mut().interact_size.y = 22.;
        let enabled = ui.is_enabled();
        let visual = ui.visuals();
        let ink = visual.text_color();
        let on = if visual.dark_mode {
            Color32::BLACK
        } else {
            Color32::WHITE
        };
        ComboBox::from_id_salt(name)
            .width(width)
            .icon(move |ui, rect, _, _| {
                let badge = Rect::from_center_size(rect.center(), Vec2::splat(16.));
                if enabled {
                    ui.painter().rect_filled(badge, 4., ink);
                }
                crate::mac::symbol(
                    ui.painter(),
                    badge.shrink(2.),
                    "chevron.up.chevron.down",
                    11.,
                    true,
                    if enabled { on } else { ink },
                );
            })
            .selected_text(rich(
                labels[index].as_ref(),
                size,
                false,
                ui.visuals().text_color(),
            ))
            .show_ui(ui, |ui| {
                for (i, label) in labels.iter().enumerate() {
                    ui.selectable_value(&mut index, i, label.as_ref());
                }
            })
            .response
            .widget_info(|| WidgetInfo::labeled(WidgetType::ComboBox, enabled, name));
    });
    (index != selected).then_some(index)
}
/// Windows ButtonBase activates on Space release and cancels a held press on
/// focus loss. Enter, pointer clicks and accessibility Invoke keep their own timing.
pub fn button_pressed(ui: &Ui, response: &mut Response) -> bool {
    let pointer = response.is_pointer_button_down_on() && response.contains_pointer();
    if !cfg!(windows) {
        return pointer;
    }
    let key = response.id.with("held space");
    let was_held = ui.data(|d| d.get_temp::<bool>(key).unwrap_or(false));
    let focused = response.has_focus() && ui.is_enabled() && ui.input(|i| i.focused);
    let mut held = was_held && focused;
    if focused {
        ui.input(|i| {
            let (mut down, mut activate) = (false, false);
            for event in &i.events {
                if let Event::Key {
                    key: Key::Space,
                    pressed,
                    repeat,
                    ..
                } = event
                {
                    if *pressed {
                        down = true;
                        held |= !repeat;
                    } else {
                        activate |= held;
                        held = false;
                    }
                }
            }
            if down || activate {
                response.flags.set(
                    egui::response::Flags::FAKE_PRIMARY_CLICKED,
                    activate
                        || i.key_pressed(Key::Enter)
                        || i.has_accesskit_action_request(
                            response.id,
                            egui::accesskit::Action::Click,
                        ),
                );
            }
        });
    }
    if held != was_held {
        ui.data_mut(|d| {
            if held {
                d.insert_temp(key, true);
            } else {
                d.remove::<bool>(key);
            }
        });
    }
    pointer || held
}

/// WPF applies the interaction opacity to the entire painted Button template,
/// including its label and focus border. The background under these controls is solid.
pub fn button_feedback(
    ui: &mut Ui,
    backdrop: Color32,
    ink: Color32,
    focus_inset: Vec2,
    button: impl FnOnce(&mut Ui) -> Response,
) -> Response {
    if !cfg!(windows) {
        return button(ui);
    }
    let saved = ui.opacity();
    if !ui.is_enabled() {
        ui.set_opacity(1.);
    }
    let mut response = button(ui);
    let pressed = button_pressed(ui, &mut response);
    let mut bounds = response.rect;
    if response.has_focus() {
        let focus = response.rect.shrink2(focus_inset);
        bounds = bounds.union(focus);
        ui.painter()
            .rect_stroke(focus, 5, Stroke::new(1.0_f32, ink), StrokeKind::Inside);
    }
    let opacity = if !ui.is_enabled() {
        0.45
    } else if pressed {
        0.60
    } else if response.hovered() {
        0.78
    } else {
        1.
    };
    if opacity < 1. {
        let overlay = Color32::from_rgba_unmultiplied(
            backdrop.r(),
            backdrop.g(),
            backdrop.b(),
            ((1.0_f32 - opacity) * 255.).round() as u8,
        );
        ui.painter().rect_filled(bounds, 0, overlay);
    }
    ui.set_opacity(saved);
    response.on_hover_cursor(CursorIcon::PointingHand)
}
#[cfg(target_os = "macos")]
pub fn action_on(ui: &mut Ui, s: &str, primary: bool, p: Palette, backdrop: Color32) -> Response {
    let (text, symbol) = if let Some(title) = s.strip_prefix("+  ") {
        (title, Some("plus"))
    } else if let Some(title) = s.strip_prefix("↻  ") {
        (title, Some("arrow.clockwise"))
    } else {
        (s, None)
    };
    let mut size = crate::mac_text::natural_size(ui, text, 13., false).unwrap_or_else(|| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), FontId::proportional(13.), p.ink)
            .size()
    });
    size.y = size.y.max(crate::mac_text::system_line_height(ui, 13.));
    let icon_width = if symbol.is_some() { 20. } else { 0. };
    let padding = if primary { vec2(12., 6.) } else { vec2(8., 3.) };
    let (rect, response) = ui.allocate_exact_size(
        size.ceil() + 2. * padding + vec2(icon_width, 0.),
        Sense::click(),
    );
    let enabled = ui.is_enabled();
    let fill = match (primary, enabled) {
        (true, true) => p.accent,
        (true, false) => p.track,
        _ => p.card,
    };
    let ink = if !enabled {
        p.muted
    } else if primary {
        p.on
    } else {
        p.ink
    };
    let stroke = if primary {
        Stroke::NONE
    } else {
        Stroke::new(1.0_f32, p.line)
    };
    let corner = if primary { 8 } else { 6 };
    let opacity = ui.opacity();
    if !enabled {
        ui.set_opacity(1.);
    }
    let painter = ui.painter();
    painter.rect(rect, corner, fill, stroke, StrokeKind::Inside);
    if let Some(name) = symbol {
        let icon = Rect::from_center_size(
            pos2(rect.left() + padding.x + 6.5, rect.center().y),
            Vec2::splat(14.),
        );
        crate::mac::symbol(painter, icon, name, 13., false, ink);
    }
    painter.text(
        pos2(rect.left() + padding.x + icon_width, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(13.),
        ink,
    );
    if response.is_pointer_button_down_on() {
        painter.rect_filled(rect, corner, backdrop.gamma_multiply(0.30));
    }
    if response.has_focus() {
        painter.rect_stroke(
            rect.expand(2.),
            corner,
            Stroke::new(2.0_f32, p.muted),
            StrokeKind::Outside,
        );
    }
    ui.set_opacity(opacity);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, text));
    response
}

#[cfg(not(target_os = "macos"))]
pub fn action_on(ui: &mut Ui, s: &str, primary: bool, p: Palette, backdrop: Color32) -> Response {
    let ink = if primary { p.on } else { p.ink };
    button_feedback(ui, backdrop, ink, vec2(10., 5.), |ui| {
        #[cfg(windows)]
        let minimum = crate::windows_text::natural_size(ui, s, 12., false)
            .map_or(vec2(0., 31.), |size| size + vec2(26., 16.));
        #[cfg(not(windows))]
        let minimum = vec2(0., 31.);
        ui.add(
            Button::new(rich(s, 12., false, ink))
                .fill(if primary { p.accent } else { p.card })
                .stroke(Stroke::new(
                    1.0_f32,
                    if primary { p.accent } else { p.line },
                ))
                .corner_radius(8)
                .min_size(minimum),
        )
    })
    .on_hover_cursor(CursorIcon::PointingHand)
}
pub fn action(ui: &mut Ui, s: &str, primary: bool, p: Palette) -> Response {
    action_on(ui, s, primary, p, p.card)
}

#[cfg(all(test, windows))]
mod button_tests {
    use super::*;

    #[test]
    fn space_release_focus_loss_and_composed_button_states() {
        let ctx = Context::default();
        let p = Palette::new(&ctx, "light");
        p.apply(&ctx);
        let mut time = 0.;
        let mut frame = |events, enabled| {
            time += 0.05;
            let mut response = None;
            let input = RawInput {
                time: Some(time),
                events,
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(240., 120.))),
                ..Default::default()
            };
            let rendered = ctx.run(input, |ctx| {
                CentralPanel::default().frame(Frame::NONE).show(ctx, |ui| {
                    response = Some(
                        ui.add_enabled_ui(enabled, |ui| action(ui, "Connect", true, p))
                            .inner,
                    );
                });
            });
            let overlay = rendered
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    Shape::Rect(r) if r.fill.a() > 0 && r.fill.a() < 255 => Some(r.fill.a()),
                    _ => None,
                })
                .next_back();
            (response.unwrap(), overlay)
        };
        let key = |pressed, repeat| Event::Key {
            key: Key::Space,
            physical_key: Some(Key::Space),
            pressed,
            repeat,
            modifiers: Modifiers::NONE,
        };
        let (r, _) = frame(vec![], true);
        let center = r.rect.center();
        r.request_focus();
        assert_eq!(frame(vec![Event::PointerMoved(center)], true).1, Some(56));
        let (held, alpha) = frame(vec![key(true, false)], true);
        assert!(!held.clicked());
        assert_eq!(alpha, Some(102));
        assert!(!frame(vec![key(true, true)], true).0.clicked());
        assert!(frame(vec![key(false, false)], true).0.clicked());
        let (held, _) = frame(vec![key(true, false)], true);
        held.surrender_focus();
        assert!(!frame(vec![key(false, false)], true).0.clicked());
        let (disabled, alpha) = frame(vec![key(true, false), key(false, false)], false);
        assert!(!disabled.clicked());
        assert_eq!(alpha, Some(140));
    }
}
pub fn link(ui: &mut Ui, s: &str, p: Palette, backdrop: Color32) -> Response {
    button_feedback(ui, backdrop, p.ink, vec2(-3., 1.), |ui| {
        ui.add(Button::new(rich(s, 12., false, p.ink)).frame(false))
    })
    .on_hover_cursor(CursorIcon::PointingHand)
}
pub fn divider(ui: &mut Ui, p: Palette) {
    let (above, below) = if cfg!(windows) {
        (16., 16.)
    } else {
        (15., 14.)
    };
    spaced_divider(ui, p.line, above, below);
}

pub fn spaced_divider(ui: &mut Ui, color: Color32, above: f32, below: f32) {
    ui.add_space(above);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.), Sense::hover());
    ui.painter().rect_filled(r, 0, color);
    ui.add_space(below);
}
pub fn toggle(ui: &mut Ui, s: &str, value: bool, tint: Color32, p: Palette) -> Response {
    #[cfg(target_os = "macos")]
    {
        mac_setting_toggle(ui, s, None, value, tint, p)
    }
    #[cfg(windows)]
    {
        named_toggle(ui, s, s, value, tint, p)
    }
}

#[cfg(target_os = "macos")]
pub fn mac_setting_toggle(
    ui: &mut Ui,
    title: &str,
    detail: Option<&str>,
    value: bool,
    tint: Color32,
    p: Palette,
) -> Response {
    let height = if detail.is_some() { 33. } else { 22. };
    let (row, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), value, title));
    let painter = ui.painter();
    if let Some(detail) = detail {
        painter.text(
            row.left_top(),
            Align2::LEFT_TOP,
            title,
            FontId::proportional(13.),
            p.ink,
        );
        painter.text(
            row.left_bottom(),
            Align2::LEFT_BOTTOM,
            detail,
            FontId::proportional(10.),
            p.muted,
        );
    } else {
        painter.text(
            row.left_center(),
            Align2::LEFT_CENTER,
            title,
            FontId::proportional(13.),
            p.ink,
        );
    }
    let track = Rect::from_center_size(row.right_center() - vec2(19., 0.), vec2(38., 22.));
    let track_color = if value {
        // The system switch blends a monochrome accent into its control
        // surface. A pure white accent would hide the thumb in Dark Aqua.
        p.card.lerp_to_gamma(tint, if p.dark { 0.29 } else { 0.92 })
    } else {
        p.card.lerp_to_gamma(p.ink, 0.12)
    };
    let thumb = if p.dark {
        Color32::from_gray(198)
    } else {
        Color32::WHITE
    };
    painter.rect(
        track,
        11.,
        track_color,
        Stroke::new(1.0_f32, p.line),
        StrokeKind::Inside,
    );
    let knob = pos2(
        if value {
            track.right() - 11.
        } else {
            track.left() + 11.
        },
        track.center().y,
    );
    painter.circle(
        knob,
        10.,
        thumb,
        Stroke::new(1.0_f32, track_color.lerp_to_gamma(p.ink, 0.12)),
    );
    if response.has_focus() {
        painter.rect_stroke(
            track.expand(2.),
            12.,
            Stroke::new(2.0_f32, p.muted),
            StrokeKind::Outside,
        );
    }
    response
}
pub fn checkbox(ui: &mut Ui, checked: &mut bool, title: &str, size: f32, p: Palette) -> Response {
    let text = ui
        .painter()
        .layout_no_wrap(title.to_owned(), FontId::proportional(size), p.ink);
    #[cfg(windows)]
    let content = crate::windows_text::natural_size(ui, title, size, false).unwrap_or(text.size());
    #[cfg(target_os = "macos")]
    let content = crate::mac_text::natural_size(ui, title, size, false).unwrap_or(text.size());
    let (row, mut response) = ui.allocate_exact_size(
        vec2(
            20. + content.x,
            ui.spacing().interact_size.y.max(content.y).max(13.),
        ),
        Sense::click(),
    );
    button_pressed(ui, &mut response);
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *checked, title)
    });
    let box_rect = Rect::from_center_size(pos2(row.left() + 6.5, row.center().y), Vec2::splat(13.));
    let edge = if response.has_focus() {
        Stroke::new(2.0_f32, p.ink)
    } else {
        let border = if cfg!(target_os = "macos") {
            p.card.lerp_to_gamma(p.ink, 0.23)
        } else {
            p.muted
        };
        Stroke::new(1.0_f32, border)
    };
    ui.painter().rect(
        box_rect,
        3,
        if *checked { p.accent } else { p.card },
        edge,
        StrokeKind::Inside,
    );
    if *checked {
        let path = [vec2(2., 6.), vec2(5., 9.), vec2(11., 2.)].map(|v| box_rect.min + v);
        ui.painter()
            .add(Shape::line(path.to_vec(), Stroke::new(1.5_f32, p.on)));
    }
    ui.painter().galley(
        pos2(row.left() + 20., row.center().y - content.y / 2.),
        text,
        p.ink,
    );
    response
}
pub fn named_toggle(
    ui: &mut Ui,
    s: &str,
    name: &str,
    value: bool,
    tint: Color32,
    p: Palette,
) -> Response {
    #[cfg(windows)]
    let height =
        crate::windows_text::natural_size(ui, s, 11., false).map_or(16., |size| size.y.max(16.));
    #[cfg(not(windows))]
    let height = 19.;
    let (r, mut response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    button_pressed(ui, &mut response);
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), value, name));
    let painter = ui.painter();
    painter.text(
        r.left_center(),
        Align2::LEFT_CENTER,
        s,
        FontId::proportional(11.),
        p.ink,
    );
    let t = Rect::from_center_size(r.right_center() - vec2(14., 0.), vec2(28., 16.));
    painter.rect_filled(t, 8, if value { tint } else { p.track });
    painter.circle_filled(
        pos2(
            if value { t.right() - 8. } else { t.left() + 8. },
            t.center().y,
        ),
        6.,
        if cfg!(windows) {
            if value { p.card } else { p.muted }
        } else if value || p.dark {
            Color32::WHITE
        } else {
            p.muted
        },
    );
    if response.has_focus() {
        painter.rect_stroke(
            r.expand(2.),
            3,
            Stroke::new(1.0_f32, p.ink),
            StrokeKind::Inside,
        );
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

pub struct Assets([(TextureHandle, f32); 3]);
impl Assets {
    pub fn new(ctx: &Context) -> Self {
        let entries: [(&str, &[u8], f32); 3] = [
            (
                "Codex",
                include_bytes!("../../shared/assets/CodexLogo.png"),
                1.,
            ),
            (
                "Claude",
                include_bytes!("../../windows/assets/ClaudeLogo.png"),
                0.75,
            ),
            (
                "Gemini",
                include_bytes!("../../shared/assets/GeminiLogo.png"),
                0.82,
            ),
        ];
        Self(entries.map(|(label, bytes, scale)| {
            let image = image::load_from_memory(bytes)
                .expect("provider mark")
                .to_rgba8();
            (
                ctx.load_texture(
                    label,
                    ColorImage::from_rgba_unmultiplied(
                        [image.width() as usize, image.height() as usize],
                        &image,
                    ),
                    TextureOptions::LINEAR.with_mipmap_mode(Some(TextureFilter::Linear)),
                ),
                scale,
            )
        }))
    }
    pub fn logo(&self, ui: &mut Ui, p: &str, size: f32) {
        let (texture, scale) = &self.0[match p {
            "Claude" => 1,
            "Gemini" => 2,
            _ => 0,
        }];
        let (r, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Image, ui.is_enabled(), p));
        #[cfg(target_os = "macos")]
        let native = crate::mac::provider_logo(ui.ctx(), p, size * scale);
        #[cfg(target_os = "macos")]
        let texture = native.as_ref().unwrap_or(texture);
        ui.painter().image(
            texture.id(),
            Rect::from_center_size(r.center(), Vec2::splat(size * scale)),
            Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
            Color32::WHITE,
        );
    }
}
pub fn mark(ui: &mut Ui, size: f32, p: Palette) {
    let (r, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(r, (size * 108. / 456.).round() as u8, p.accent);
    let s = size / 456.;
    let org = r.center() - vec2(133., 141.) * s;
    for (x, y, h) in [(0., 164., 118.), (100., 80., 202.), (200., 0., 282.)] {
        painter.rect_filled(
            Rect::from_min_size(org + vec2(x, y) * s, vec2(66., h) * s),
            (22. * s).round() as u8,
            p.on,
        );
    }
}
pub fn glyph(painter: &Painter, r: Rect, name: &str, c: Color32, _selected: bool) {
    #[cfg(target_os = "macos")]
    {
        let symbol = match name {
            "dashboard" => "square.grid.2x2",
            "accounts" => "person.2",
            "settings" => "slider.horizontal.3",
            "attention" => "exclamationmark.circle",
            "bolt" => "bolt",
            _ => "chart.bar",
        };
        if crate::mac::symbol(painter, r, symbol, 13., _selected, c) {
            return;
        }
    }
    let pt = |x, y| {
        pos2(
            r.left() + x * r.width() / 20.,
            r.top() + y * r.height() / 20.,
        )
    };
    let stroke = Stroke::new(r.width() * 0.065, c);
    let path = |xy: &[(f32, f32)]| {
        painter.add(Shape::line(
            xy.iter().map(|&(x, y)| pt(x, y)).collect(),
            stroke,
        ))
    };
    match name {
        "dashboard" => {
            for (x, y) in [(2., 2.), (12., 2.), (2., 12.), (12., 12.)] {
                painter.rect_stroke(
                    Rect::from_min_max(pt(x, y), pt(x + 6., y + 6.)),
                    0,
                    stroke,
                    StrokeKind::Middle,
                );
            }
        }
        "accounts" => {
            painter.circle_stroke(pt(7., 7.), r.width() * 0.15, stroke);
            path(&[(1., 18.), (1., 16.)]);
            painter.add(epaint::CubicBezierShape::from_points_stroke(
                [pt(1., 16.), pt(1., 10.), pt(13., 10.), pt(13., 16.)],
                false,
                Color32::TRANSPARENT,
                stroke,
            ));
            path(&[(13., 16.), (13., 18.)]);
            painter.add(epaint::CubicBezierShape::from_points_stroke(
                [pt(14., 4.), pt(18., 4.), pt(18., 10.), pt(14., 10.)],
                false,
                Color32::TRANSPARENT,
                stroke,
            ));
            painter.add(epaint::CubicBezierShape::from_points_stroke(
                [pt(15., 13.), pt(19., 13.), pt(19., 15.), pt(19., 18.)],
                false,
                Color32::TRANSPARENT,
                stroke,
            ));
        }
        "settings" => {
            for (y, x) in [(5., 9.), (10., 14.), (15., 6.)] {
                path(&[(2., y), (x - 2., y)]);
                path(&[(x + 2., y), (18., y)]);
                painter.rect_stroke(
                    Rect::from_min_max(pt(x - 2., y - 2.), pt(x + 2., y + 2.)),
                    0,
                    stroke,
                    StrokeKind::Middle,
                );
            }
        }
        "attention" => {
            painter.circle_stroke(r.center(), r.width() * 0.4, stroke);
            path(&[(10., 6.), (10., 11.)]);
            path(&[(10., 14.), (10., 15.)]);
        }
        "bolt" => {
            path(&[
                (11., 1.),
                (4., 11.),
                (9., 11.),
                (8., 19.),
                (16., 8.),
                (11., 8.),
                (11., 1.),
            ]);
        }
        _ => {
            for (x, h) in [(3., 11.), (9., 6.), (15., 2.)] {
                path(&[(x, 18.), (x, h), (x + 3., h), (x + 3., 18.)]);
            }
        }
    }
}
