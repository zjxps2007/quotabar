use eframe::egui::{self, *};
#[cfg(not(target_os = "macos"))]
use font_kit::{family_name::FamilyName, handle::Handle, properties::Properties};
use font_kit::{properties::Weight, source::SystemSource};
use std::sync::Arc;

#[cfg(windows)]
mod windows_bitmaps;

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
        ["system-monospaced", "Apple SD Gothic Neo"]
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
    #[cfg(target_os = "macos")]
    placeholder: Color32,
    #[cfg(target_os = "macos")]
    pub(crate) focus_ring: Color32,
    #[cfg(target_os = "macos")]
    pub system_link: Color32,
    #[cfg(target_os = "macos")]
    pub window_background: Color32,
    pub card: Color32,
    pub bg: Color32,
    pub rail: Color32,
    pub line: Color32,
    pub track: Color32,
    pub accent: Color32,
    pub on: Color32,
    pub warning: Color32,
}

/// Composite a semantic foreground over an opaque sRGB surface, retaining its
/// system-provided alpha and rounding only the resulting surface color.
pub fn surface_fill(surface: Color32, foreground: Color32, opacity: f32) -> Color32 {
    let [r, g, b, a] = foreground.to_srgba_unmultiplied();
    surface.lerp_to_gamma(Color32::from_rgb(r, g, b), opacity * f32::from(a) / 255.)
}

#[derive(Clone, Copy)]
pub struct CardSurface {
    frame: Frame,
    #[cfg(target_os = "macos")]
    border: Color32,
}

impl CardSurface {
    pub fn total_margin(&self) -> epaint::MarginF32 {
        self.frame.total_margin()
    }

    pub fn corner_radius(mut self, radius: u8) -> Self {
        self.frame = self.frame.corner_radius(radius);
        self
    }

    #[cfg(target_os = "macos")]
    pub fn show_centered_in_grid<R>(
        self,
        ui: &mut Ui,
        id: Id,
        width: f32,
        content: impl FnOnce(&mut Ui) -> R,
    ) -> InnerResponse<R> {
        let height = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or_default();
        ui.allocate_ui_with_layout(vec2(width, height), Layout::top_down(Align::Min), |ui| {
            let result = self.show(ui, content);
            let measured = result.response.rect.height();
            if measured != height {
                ui.data_mut(|d| d.insert_temp(id, measured));
                ui.ctx()
                    .request_discard("Center cards using their intrinsic heights");
            }
            result
        })
        .inner
    }

    pub fn show<R>(self, ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
        let result = self.frame.show(ui, content);
        #[cfg(target_os = "macos")]
        {
            use eframe::emath::GuiRounding;
            // SwiftUI snaps the card frame to backing pixels. The centered
            // stroke still covers half a pixel on either side at 1x.
            let bounds = result.response.rect.round_to_pixels(ui.pixels_per_point());
            ui.painter().add(
                epaint::RectShape::stroke(
                    bounds,
                    self.frame.corner_radius,
                    Stroke::new(1_f32, self.border),
                    StrokeKind::Middle,
                )
                .with_round_to_pixels(false),
            );
        }
        result
    }
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
        let (ink, muted, tertiary, placeholder, focus_ring, warning, system_link) = {
            use block2::StackBlock;
            use objc2::MainThreadMarker;
            use objc2_app_kit::{NSApplication, NSColor};
            use std::cell::Cell;

            let app = NSApplication::sharedApplication(
                MainThreadMarker::new().expect("dashboard UI thread"),
            );
            let appearance = app.effectiveAppearance();
            let key = Id::new(("AppKit control colors", appearance.name().to_string()));
            ctx.data(|data| data.get_temp(key)).unwrap_or_else(|| {
                let primary = gray(220, 39);
                let secondary = gray(151, 128);
                let tertiary = gray(84, 189);
                let orange = if dark {
                    Color32::from_rgb(255, 159, 10)
                } else {
                    Color32::from_rgb(255, 149, 0)
                };
                let link = if dark {
                    Color32::from_rgb(65, 156, 255)
                } else {
                    Color32::from_rgb(0, 104, 218)
                };
                let colors = Cell::new((
                    primary, secondary, tertiary, tertiary, primary, orange, link,
                ));
                appearance.performAsCurrentDrawingAppearance(&StackBlock::new(|| {
                    colors.set((
                        mac_color(&NSColor::labelColor(), primary),
                        mac_color(&NSColor::secondaryLabelColor(), secondary),
                        mac_color(&NSColor::tertiaryLabelColor(), tertiary),
                        mac_color(&NSColor::placeholderTextColor(), tertiary),
                        mac_color(&NSColor::keyboardFocusIndicatorColor(), primary),
                        mac_color(&NSColor::systemOrangeColor(), orange),
                        mac_color(&NSColor::linkColor(), link),
                    ));
                }));
                let colors = colors.get();
                ctx.data_mut(|data| data.insert_temp(key, colors));
                colors
            })
        };
        #[cfg(windows)]
        let (ink, muted, tertiary) = (gray(242, 23), gray(163, 117), gray(163, 117));
        let card = gray(28, 255);
        Self {
            dark,
            ink,
            muted,
            tertiary,
            #[cfg(target_os = "macos")]
            placeholder,
            #[cfg(target_os = "macos")]
            focus_ring,
            #[cfg(target_os = "macos")]
            system_link,
            #[cfg(target_os = "macos")]
            // The exposed SwiftUI sheet surface in Dark Aqua and Aqua.
            window_background: gray(30, 236),
            card,
            bg: gray(14, 245),
            rail: gray(19, 255),
            line: gray(45, 235),
            track: if cfg!(target_os = "macos") {
                surface_fill(card, ink, 0.06)
            } else {
                gray(42, 240)
            },
            accent: gray(255, 0),
            on: gray(0, 255),
            #[cfg(target_os = "macos")]
            warning,
            #[cfg(windows)]
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
                // WPF ToolTipService.InitialShowDelay and BetweenShowDelay, in seconds.
                style.interaction.tooltip_delay = 1.0;
                style.interaction.tooltip_grace_time = 0.1;
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
    pub fn frame(self, pad: impl Into<Margin>) -> CardSurface {
        let frame = Frame::new()
            .fill(self.card)
            .corner_radius(16)
            .inner_margin(pad);
        #[cfg(not(target_os = "macos"))]
        let frame = frame.stroke(Stroke::new(1_f32, self.line));
        CardSurface {
            frame,
            // SwiftUI overlays a centered stroke without reserving layout space.
            #[cfg(target_os = "macos")]
            border: self.ink.gamma_multiply(0.07),
        }
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
    let background = ui.painter().add(Shape::Noop);
    // Rounded AppKit fields inset the editable line and its placeholder by
    // 11 points. The field editor's text baseline sits above the placeholder.
    let field_margin = Margin {
        left: 11,
        right: 11,
        top: 0,
        bottom: if text.is_empty() { 0 } else { 2 },
    };
    let field = ui.add(
        TextEdit::singleline(text)
            .id(Id::new(id))
            .hint_text(rich(hint, 13., false, p.placeholder))
            .font(FontId::proportional(13.))
            .frame(false)
            .margin(field_margin)
            .vertical_align(Align::Center)
            .min_size(vec2(0., 22.))
            .desired_width((width - field_margin.sum().x).max(0.)),
    );
    // A rounded Dark Aqua editor has a raised bezel within its layout bounds.
    let (bezel, fill, edge) = if p.dark {
        (
            field.rect.shrink(1.),
            Color32::from_gray(40),
            Color32::from_gray(59),
        )
    } else {
        (field.rect, p.card, p.line)
    };
    ui.painter().set(
        background,
        epaint::RectShape::new(
            bezel,
            6.,
            fill,
            Stroke::new(1.0_f32, edge),
            StrokeKind::Inside,
        ),
    );
    if field.has_focus() {
        ui.painter().rect_stroke(
            field.rect,
            6.,
            Stroke::new(4.0_f32, p.focus_ring),
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
    if let Some(response) = crate::windows_text::draw_label(
        ui,
        &s,
        size,
        heavy,
        c,
        crate::windows_text::LabelLayout::Inline,
    ) {
        return response;
    }
    ui.label(rich(s, size, heavy, c))
}

pub fn ellipsis_label(ui: &mut Ui, text: &str, size: f32, color: Color32) -> Response {
    #[cfg(windows)]
    if let Some(response) = crate::windows_text::draw_label(
        ui,
        text,
        size,
        false,
        color,
        crate::windows_text::LabelLayout::SingleLine,
    ) {
        return tooltip_text(response, text);
    }
    let response = ui.add(
        Label::new(rich(text, size, false, color))
            .truncate()
            .show_tooltip_when_elided(false),
    );
    tooltip_text(response, text)
}

pub fn tooltip_text(response: Response, text: &str) -> Response {
    #[cfg(windows)]
    {
        response
            .ctx
            .accesskit_node_builder(response.id, |node| node.set_tooltip(text));
        response.on_hover_text_at_pointer(text)
    }
    #[cfg(target_os = "macos")]
    {
        response
            .ctx
            .accesskit_node_builder(response.id, |node| node.set_description(text));
        response.on_hover_text(text)
    }
}
pub fn caption(ui: &mut Ui, s: impl Into<String>, p: Palette) -> Response {
    label(ui, s, 11., false, p.muted)
}

pub fn system_label(ui: &mut Ui, text: &str, size: f32, heavy: bool, color: Color32) -> Response {
    #[cfg(target_os = "macos")]
    if let Some(response) = crate::mac_text::system_label(ui, text, size, heavy, color) {
        return response;
    }
    label(ui, text, size, heavy, color)
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct ExpandedDisclosure;

#[cfg(windows)]
pub fn reset_disclosures(ctx: &Context) {
    // WPF rebuilds its Expander controls when the page or its data is replaced.
    ctx.data_mut(|data| data.remove_by_type::<ExpandedDisclosure>());
}

#[cfg(windows)]
pub fn disclosure(
    ui: &mut Ui,
    key: impl std::hash::Hash,
    title: &str,
    size: f32,
    p: Palette,
    body: impl FnOnce(&mut Ui),
) {
    use crate::windows_text::{MeasuredLabel, TextStack};

    let id = ui.make_persistent_id(key);
    let was_open = ui.data(|data| data.get_temp::<ExpandedDisclosure>(id).is_some());
    // The WPF Expander stretches its transparent toggle across the entire card.
    // Its 10-DIP arrow and natural TextBlock share 2 DIPs of vertical padding.
    let height = TextStack::line_height(size).unwrap_or(size).max(10.) + 4.;
    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), height));
    let mut response = ui.interact(rect, id, Sense::click());
    if response.clicked_by(PointerButton::Primary)
        || (response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down()))
    {
        response.request_focus();
    }
    button_pressed(ui, &mut response);
    let mut open = was_open ^ response.clicked();
    if response.enabled() {
        ui.input_mut(|input| {
            input.consume_accesskit_action_requests(id, |request| {
                match request.action {
                    accesskit::Action::Expand => open = true,
                    accesskit::Action::Collapse => open = false,
                    _ => return false,
                }
                true
            })
        });
    }
    if open != was_open {
        ui.data_mut(|data| {
            if open {
                data.insert_temp(id, ExpandedDisclosure);
            } else {
                data.remove::<ExpandedDisclosure>(id);
            }
        });
    }
    response
        .widget_info(|| WidgetInfo::labeled(WidgetType::CollapsingHeader, ui.is_enabled(), title));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_expanded(open);
        if response.enabled() {
            node.add_action(if open {
                accesskit::Action::Collapse
            } else {
                accesskit::Action::Expand
            });
        }
    });
    if ui.is_rect_visible(rect) {
        let center = pos2(rect.left() + 5., rect.center().y);
        let arrow = [vec2(-4., -4.), vec2(0., 0.), vec2(-4., 4.)];
        let arrow = arrow.map(|v| center + if open { vec2(-v.y, v.x) } else { v });
        ui.painter()
            .add(Shape::line(arrow.to_vec(), Stroke::new(1.2_f32, p.muted)));
        let origin = pos2(rect.left() + 15., rect.top() + 2.);
        if let Some(text) = MeasuredLabel::natural(ui, title, size, false) {
            text.paint(ui.painter(), origin, p.muted);
        } else {
            ui.painter().text(
                origin,
                Align2::LEFT_TOP,
                title,
                FontId::proportional(size),
                p.muted,
            );
        }
    }
    if open {
        ui.add_space(6.);
        ui.push_id(id, body);
    }
}

#[cfg(target_os = "macos")]
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
        // SwiftUI aligns the disclosed stack with the card's leading edge.
        ui.spacing_mut().button_padding = Vec2::ZERO;
        ui.spacing_mut().indent = 12.;
        ui.spacing_mut().interact_size.y = 22.;
        header
            .icon(move |ui, openness, response| {
                let center = response.rect.center() - vec2(2., 0.);
                let bounds = Rect::from_center_size(center, vec2(9., 9.));
                crate::mac::symbol(
                    ui.painter(),
                    bounds,
                    if openness >= 0.5 {
                        "chevron.down"
                    } else {
                        "chevron.right"
                    },
                    8.,
                    true,
                    p.tertiary,
                );
            })
            .show_unindented(ui, body);
    });
}
#[cfg(windows)]
pub use crate::windows_choice::choose;
#[cfg(target_os = "macos")]
pub fn mac_scroll(
    ui: &mut Ui,
    scroll: ScrollArea,
    p: Palette,
    rail: Option<&crate::mac::NativeScroller>,
    capturing: bool,
    body: impl FnOnce(&mut Ui),
) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSScroller, NSScrollerStyle};
    let content_style = ui.style().clone();
    ui.scope(|ui| {
        let style = ui.style_mut();
        // NSClipView clips content exactly at the viewport, including when
        // the last settings card is scrolled past the fixed demo banner.
        style.visuals.clip_rect_margin = 0.;
        let native_width = rail.and_then(|r| r.legacy_width());
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
        if let Some(width) = native_width {
            style.spacing.scroll.bar_width = width;
            style.spacing.scroll.bar_inner_margin = 0.;
            style.spacing.scroll.bar_outer_margin = 0.;
            style.visuals.extreme_bg_color = Color32::TRANSPARENT;
            style.visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
            style.visuals.widgets.hovered.bg_fill = Color32::TRANSPARENT;
            style.visuals.widgets.active.bg_fill = Color32::TRANSPARENT;
        }
        let area = scroll.show(ui, |ui| {
            ui.set_style(content_style);
            body(ui);
        });
        if let Some(rail) = rail {
            if let Some(width) = native_width {
                rail.place(
                    area.id,
                    area.inner_rect,
                    width,
                    area.content_size.y,
                    area.state.offset.y,
                );
                if capturing {
                    let rect = Rect::from_min_size(
                        area.inner_rect.right_top(),
                        vec2(width, area.inner_rect.height()),
                    );
                    rail.paint_capture(ui, rect);
                }
            } else {
                rail.hide();
            }
        }
    });
}

#[cfg(target_os = "macos")]
pub use crate::mac::popup_choice as choose;
/// Windows ButtonBase activates on Space release and cancels a held press on
/// focus loss. Enter, pointer clicks and accessibility Invoke keep their own timing.
pub fn button_pressed(ui: &Ui, response: &mut Response) -> bool {
    control_pressed(ui, response, true)
}

fn control_pressed(ui: &Ui, response: &mut Response, accepts_return: bool) -> bool {
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
            let enter = i.key_pressed(Key::Enter);
            if down || activate || (enter && !accepts_return) {
                response.flags.set(
                    egui::response::Flags::FAKE_PRIMARY_CLICKED,
                    activate
                        || (enter && accepts_return)
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
    if !primary
        && symbol.is_some()
        && let Some(response) = crate::mac::standard_button(
            ui,
            text,
            symbol,
            objc2_app_kit::NSControlSize::Regular,
            None,
            p.focus_ring,
        )
    {
        return response;
    }
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
pub fn sheet_button(ui: &mut Ui, title: &str, p: Palette) -> Response {
    #[cfg(target_os = "macos")]
    if let Some(response) = crate::mac::standard_button(
        ui,
        title,
        None,
        objc2_app_kit::NSControlSize::Regular,
        None,
        p.focus_ring,
    ) {
        return response;
    }
    action(ui, title, false, p)
}

#[cfg(target_os = "macos")]
pub fn small_button(ui: &mut Ui, title: &str, p: Palette) -> Response {
    crate::mac::standard_button(
        ui,
        title,
        None,
        objc2_app_kit::NSControlSize::Small,
        None,
        p.focus_ring,
    )
    .unwrap_or_else(|| action(ui, title, false, p))
}

pub fn action(ui: &mut Ui, s: &str, primary: bool, p: Palette) -> Response {
    action_on(ui, s, primary, p, p.card)
}

#[cfg(all(test, windows))]
mod button_tests {
    use super::*;

    #[test]
    fn narrow_switch_wraps_its_caption_and_focuses_the_track() {
        use windows::Win32::System::Com::{
            COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
        };
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .unwrap();
        let ctx = Context::default();
        ctx.add_plugin(crate::windows_text::TextRenderer::default());
        let p = Palette::new(&ctx, "light");
        let frame = || {
            let mut row = None;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(100., 120.))),
                ..Default::default()
            };
            let output = ctx.run(input, |ctx| {
                CentralPanel::default().frame(Frame::NONE).show(ctx, |ui| {
                    ui.set_width(100.);
                    let title = "작업 표시줄에 앱 아이콘 표시";
                    row = Some(named_toggle(ui, title, title, true, p.accent, p));
                });
            });
            (row.unwrap(), output.shapes)
        };
        frame().0.request_focus();
        let (response, shapes) = frame();
        let row = response.rect;
        assert!((row.height() - 43.89).abs() < 1. / 32., "{row:?}");
        let ring = shapes
            .iter()
            .find_map(|s| match &s.shape {
                Shape::Rect(r) if r.fill == Color32::TRANSPARENT && r.stroke.color == p.ink => {
                    Some(r.rect)
                }
                _ => None,
            })
            .expect("focused switch border");
        assert_eq!(
            ring,
            Rect::from_min_max(
                pos2(row.right() - 31., row.top() - 3.),
                row.max + vec2(3., 3.)
            )
        );
        unsafe { CoUninitialize() };
    }

    #[test]
    fn stacked_link_centers_its_label_in_the_full_hit_area() {
        let ctx = Context::default();
        let p = Palette::new(&ctx, "light");
        p.apply(&ctx);
        let title = "계정 관리에서 확인 →";
        let mut hit = Rect::NOTHING;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(252., 100.))),
            ..Default::default()
        };
        let frame = ctx.run(input, |ctx| {
            CentralPanel::default().frame(Frame::NONE).show(ctx, |ui| {
                hit = link(ui, title, p, p.card).rect;
            });
        });
        let label = frame
            .shapes
            .iter()
            .find_map(|item| match &item.shape {
                Shape::Text(text) if text.galley.text() == title => {
                    Some(Rect::from_min_size(text.pos, text.galley.size()))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(hit.width(), 252.);
        assert!(label.width() < hit.width() && label.height() < hit.height());
        assert!(label.center().distance(hit.center()) < 1.);
    }

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
        #[cfg(windows)]
        {
            let text = crate::windows_text::natural_size(ui, s, 12., false).unwrap_or_else(|| {
                ui.painter()
                    .layout_no_wrap(s.to_owned(), FontId::proportional(12.), p.ink)
                    .size()
            });
            // A WPF link stretches across a vertical StackPanel. In a horizontal
            // section header it docks to the right at its natural width.
            let width = if ui.layout().is_vertical() {
                ui.available_width()
            } else {
                text.x
            };
            let (rect, response) = ui.allocate_exact_size(vec2(width, text.y + 8.), Sense::click());
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                s,
                FontId::proportional(12.),
                p.ink,
            );
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), s));
            response
        }
        #[cfg(target_os = "macos")]
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
    use crate::mac_text::PreferredFont;
    let caption_height = detail.map_or(0., |_| PreferredFont::Caption.line_height(ui));
    let height = if detail.is_some() {
        (PreferredFont::Body.line_height(ui) + 4. + caption_height).max(22.)
    } else {
        22.
    };
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
        PreferredFont::Caption.paint_at(
            ui,
            row.left_bottom() - vec2(0., caption_height),
            detail,
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
    if !crate::mac::draw_switch(
        ui,
        track,
        value,
        response.is_pointer_button_down_on(),
        objc2_app_kit::NSControlSize::Regular,
        None,
    ) {
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
    }
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
    checkbox_in_row(ui, checked, title, size, p, None)
}
pub fn checkbox_in_row(
    ui: &mut Ui,
    checked: &mut bool,
    title: &str,
    size: f32,
    p: Palette,
    top: Option<f32>,
) -> Response {
    let text = ui
        .painter()
        .layout_no_wrap(title.to_owned(), FontId::proportional(size), p.ink);
    #[cfg(windows)]
    let content = crate::windows_text::natural_size(ui, title, size, false).unwrap_or(text.size());
    #[cfg(target_os = "macos")]
    let content = crate::mac_text::natural_size(ui, title, size, false).unwrap_or(text.size());
    let row_size = vec2(
        20. + content.x,
        ui.spacing().interact_size.y.max(content.y).max(13.),
    );
    let (row, mut response) = ui.allocate_exact_size(row_size, Sense::click());
    let row = top.map_or(row, |y| Rect::from_min_size(pos2(row.left(), y), row_size));
    control_pressed(ui, &mut response, false);
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *checked, title)
    });
    let box_rect = Rect::from_center_size(pos2(row.left() + 6.5, row.center().y), Vec2::splat(13.));
    // SwiftUI aligns the small AppKit switch cell just below the text row's center.
    #[cfg(target_os = "macos")]
    let box_rect = box_rect.translate(vec2(0., 1.));
    #[cfg(target_os = "macos")]
    if crate::mac::draw_checkbox(ui, box_rect, *checked, response.is_pointer_button_down_on()) {
        if response.has_focus() {
            ui.painter().rect_stroke(
                box_rect.expand(2.),
                4,
                Stroke::new(2.0_f32, p.focus_ring),
                StrokeKind::Outside,
            );
        }
        ui.painter().galley(
            pos2(row.left() + 19., row.center().y - content.y / 2. - 1.),
            text,
            p.ink,
        );
        return response;
    }
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
#[cfg(target_os = "macos")]
pub fn mini_toggle_width(ui: &Ui, title: &str) -> f32 {
    let caption = crate::mac_text::natural_size(ui, title, 10., false).map_or_else(
        || {
            ui.painter()
                .layout_no_wrap(title.into(), FontId::proportional(10.), Color32::WHITE)
                .size()
                .x
        },
        |size| size.x,
    );
    caption.ceil() + 8. + 26.
}

#[cfg(windows)]
const SWITCH_TRACK: Vec2 = vec2(28., 16.);
#[cfg(windows)]
const SWITCH_CAPTION_GAP: f32 = 12.;

#[cfg(windows)]
pub fn switch_natural_size(ui: &Ui, title: &str) -> Vec2 {
    let caption = crate::windows_text::natural_size(ui, title, 11., false).unwrap_or_else(|| {
        ui.painter()
            .layout_no_wrap(title.into(), FontId::proportional(11.), Color32::WHITE)
            .size()
    });
    vec2(
        caption.x + SWITCH_CAPTION_GAP + SWITCH_TRACK.x,
        caption.y.max(SWITCH_TRACK.y),
    )
}

#[cfg(windows)]
pub fn named_toggle(
    ui: &mut Ui,
    title: &str,
    name: &str,
    value: bool,
    tint: Color32,
    p: Palette,
) -> Response {
    use crate::{
        windows_border::rounded_rectangle,
        windows_text::{LabelLayout, MeasuredLabel},
    };
    let width = ui.available_width().max(SWITCH_TRACK.x);
    let caption_width = (width - SWITCH_TRACK.x - SWITCH_CAPTION_GAP).max(0.);
    let caption = if caption_width > 0. {
        MeasuredLabel::fit(ui, title, 11., false, caption_width, LabelLayout::Block)
    } else {
        None
    };
    let fallback = || {
        ui.painter().layout(
            title.into(),
            FontId::proportional(11.),
            p.ink,
            caption_width,
        )
    };
    let fallback = (caption.is_none() && caption_width > 0.).then(fallback);
    let height = caption
        .as_ref()
        .map(|c| c.size.y)
        .or_else(|| fallback.as_ref().map(|g| g.size().y))
        .unwrap_or(0.);
    let (row, mut response) =
        ui.allocate_exact_size(vec2(width, height.max(SWITCH_TRACK.y)), Sense::click());
    control_pressed(ui, &mut response, false);
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), value, name));

    // WPF composites the disabled CheckBox as one layer over its card.
    let opacity = ui.opacity();
    if !ui.is_enabled() {
        ui.set_opacity(1.);
    }
    let origin = pos2(row.left(), row.center().y - height * 0.5);
    if let Some(caption) = caption {
        caption.paint(ui.painter(), origin, p.ink);
    } else if let Some(galley) = fallback {
        ui.painter().galley(origin, galley, p.ink);
    }
    let track = Rect::from_center_size(
        row.right_center() - vec2(SWITCH_TRACK.x * 0.5, 0.),
        SWITCH_TRACK,
    );
    rounded_rectangle(
        ui,
        track,
        8.,
        if value { tint } else { p.track },
        Stroke::NONE,
    );
    let thumb_x = if value {
        track.right() - 8.
    } else {
        track.left() + 8.
    };
    let thumb = Rect::from_center_size(pos2(thumb_x, track.center().y), Vec2::splat(12.));
    rounded_rectangle(
        ui,
        thumb,
        6.,
        if value { p.card } else { p.muted },
        Stroke::NONE,
    );
    if response.has_focus() {
        let focus = Rect::from_x_y_ranges(track.x_range(), row.y_range()).expand(3.);
        rounded_rectangle(
            ui,
            focus,
            10.,
            Color32::TRANSPARENT,
            Stroke::new(1.0_f32, p.ink),
        );
    }
    if !ui.is_enabled() {
        ui.painter()
            .rect_filled(row, 0., p.card.gamma_multiply(1. - opacity));
        ui.set_opacity(opacity);
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

#[cfg(target_os = "macos")]
pub fn named_toggle(
    ui: &mut Ui,
    s: &str,
    name: &str,
    value: bool,
    tint: Color32,
    p: Palette,
) -> Response {
    let height = 15.;
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
        FontId::proportional(10.),
        p.ink,
    );
    let size = vec2(26., 15.);
    let t = Rect::from_center_size(r.right_center() - vec2(size.x * 0.5, 0.), size);
    // SwiftUI aligns a mini switch to the caption baseline, one point below
    // the geometric center of the row.
    let t = {
        let track = t.translate(vec2(0., 1.));
        // AppKit exposes the switch itself to accessibility, including the
        // top of its alignment row. The caption remains part of the click area.
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_bounds(egui::accesskit::Rect {
                x0: track.left().into(),
                y0: r.top().into(),
                x1: track.right().into(),
                y1: track.bottom().into(),
            });
        });
        track
    };
    let native = crate::mac::draw_switch(
        ui,
        t,
        value,
        response.is_pointer_button_down_on(),
        objc2_app_kit::NSControlSize::Mini,
        Some(tint),
    );
    if !native {
        painter.rect_filled(t, 8, if value { tint } else { p.track });
        painter.circle_filled(
            pos2(
                if value { t.right() - 8. } else { t.left() + 8. },
                t.center().y,
            ),
            6.,
            if value || p.dark {
                Color32::WHITE
            } else {
                p.muted
            },
        );
    }
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

const PROVIDER_MARKS: [(&str, &[u8], f32); 3] = [
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

pub struct Assets {
    textures: [TextureHandle; 3],
    #[cfg(windows)]
    bitmaps: windows_bitmaps::Cache,
}

impl Assets {
    pub fn new(ctx: &Context) -> Self {
        Self {
            textures: PROVIDER_MARKS.map(|(label, bytes, _)| {
                let image = image::load_from_memory(bytes)
                    .expect("provider mark")
                    .to_rgba8();
                ctx.load_texture(
                    label,
                    ColorImage::from_rgba_unmultiplied(
                        [image.width() as usize, image.height() as usize],
                        &image,
                    ),
                    TextureOptions::LINEAR.with_mipmap_mode(Some(TextureFilter::Linear)),
                )
            }),
            #[cfg(windows)]
            bitmaps: windows_bitmaps::Cache::default(),
        }
    }
    pub fn logo(&self, ui: &mut Ui, p: &str, size: f32) {
        let index = match p {
            "Claude" => 1,
            "Gemini" => 2,
            _ => 0,
        };
        let texture = &self.textures[index];
        let (r, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Image, ui.is_enabled(), p));
        let artwork =
            Rect::from_center_size(r.center(), Vec2::splat(size * PROVIDER_MARKS[index].2));
        #[cfg(windows)]
        let texture = self.bitmaps.texture(ui, index, texture, artwork.width());
        #[cfg(target_os = "macos")]
        let native = crate::mac::provider_logo(ui.ctx(), p, artwork.width());
        #[cfg(target_os = "macos")]
        let (texture, artwork) = native.as_ref().map_or((texture, artwork), |native| {
            // AppKit has already resampled the artwork at backing resolution.
            // Align its bottom-left origin in Cocoa coordinates so each texel
            // covers one physical pixel, including odd-sized centered icons.
            let backing = ui.pixels_per_point();
            let pixels = native.size_vec2();
            let left = (artwork.center().x * backing - pixels.x / 2. + 0.5).floor();
            let bottom = (-artwork.center().y * backing - pixels.y / 2. + 0.5).floor();
            let top_left = pos2(left, -bottom - pixels.y) / backing;
            (native, Rect::from_min_size(top_left, pixels / backing))
        });
        ui.painter().image(
            texture.id(),
            artwork,
            Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
            Color32::WHITE,
        );
    }
}
pub fn mark(ui: &mut Ui, size: f32, p: Palette) {
    let (r, _response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    #[cfg(target_os = "macos")]
    {
        _response.widget_info(|| {
            WidgetInfo::labeled(WidgetType::Image, ui.is_enabled(), "QuotaBar 로고")
        });
        crate::mac::rounded_rect(
            ui,
            r,
            size * 108. / 456.,
            p.accent,
            Stroke::NONE,
            crate::mac::Curve::Circular,
        );
        if crate::mac::app_mark(ui, r, p.on) {
            return;
        }
    }
    #[cfg(windows)]
    crate::windows_border::rounded_rectangle(ui, r, size * 108. / 456., p.accent, Stroke::NONE);
    let s = size / 456.;
    let org = r.center() - vec2(133., 141.) * s;
    for (x, y, h) in [(0., 164., 118.), (100., 80., 202.), (200., 0., 282.)] {
        let bar = Rect::from_min_size(org + vec2(x, y) * s, vec2(66., h) * s);
        #[cfg(windows)]
        crate::windows_border::rounded_rectangle(ui, bar, 22. * s, p.on, Stroke::NONE);
        #[cfg(target_os = "macos")]
        ui.painter().rect_filled(bar, 22. * s, p.on);
    }
}
#[cfg(target_os = "macos")]
pub fn system_symbol_name(name: &str) -> &'static str {
    match name {
        "dashboard" => "square.grid.2x2",
        "accounts" => "person.2",
        "settings" => "slider.horizontal.3",
        "attention" => "exclamationmark.circle",
        "bolt" => "bolt",
        _ => "chart.bar",
    }
}
pub fn glyph(painter: &Painter, r: Rect, name: &str, c: Color32, _selected: bool) {
    #[cfg(target_os = "macos")]
    {
        if crate::mac::symbol(painter, r, system_symbol_name(name), 13., _selected, c) {
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
