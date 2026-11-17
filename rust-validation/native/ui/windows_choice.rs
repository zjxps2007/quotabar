use eframe::egui::*;

struct Selection {
    open: bool,
    highlighted: usize,
    chosen: usize,
}

#[derive(Clone, Default)]
struct TypeSearch {
    prefix: String,
    last_input: f64,
}

const LAST_INPUT: &str = "most recent Windows input device was keyboard";

pub fn track_input(ctx: &Context) {
    // WPF has one InputManager for the dispatcher, including its owned windows.
    ctx.on_end_pass(
        "Windows keyboard cues",
        std::sync::Arc::new(|ctx| {
            let keyboard = ctx.input(|input| {
                input.raw.events.iter().rev().find_map(|e| match e {
                    Event::Key { .. } => Some(true),
                    Event::PointerButton { .. }
                    | Event::MouseWheel { .. }
                    | Event::Touch { .. } => Some(false),
                    _ => None,
                })
            });
            if let Some(keyboard) = keyboard {
                ctx.data_mut(|d| d.insert_temp(Id::new(LAST_INPUT), keyboard));
            }
        }),
    );
}

fn keyboard_cues(ui: &Ui, response: &Response) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETKEYBOARDCUES, SystemParametersInfoW};

    let id = response.id.with("keyboard focus visual");
    // WPF creates the focus visual on a real focus change. UI Automation may
    // request focus again on the selected field while delivering a handled key.
    let previous = if response.gained_focus() {
        Id::new(LAST_INPUT)
    } else {
        id
    };
    let mut visible = ui
        .data(|d| d.get_temp::<bool>(previous))
        .unwrap_or_default();
    ui.input(|input| {
        for event in &input.raw.events {
            match event {
                Event::PointerButton { pressed: true, .. } => {
                    visible = false;
                }
                Event::Key {
                    key, pressed: true, ..
                } => {
                    // WPF processes navigation after the ComboBox handles its own keys.
                    // Changing the selection in place does not create a focus adorner.
                    if *key == Key::Tab
                        || (matches!(
                            key,
                            Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown
                        ) && input.events.contains(event))
                    {
                        visible = true;
                    }
                }
                _ => {}
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(id, visible));
    response.has_focus()
        && (visible || {
            let mut always = 0_i32;
            unsafe { SystemParametersInfoW(SPI_GETKEYBOARDCUES, 0, (&raw mut always).cast(), 0) };
            always != 0
        })
}

fn paint_focus_dash(mesh: &mut Mesh, start: Pos2, end: Pos2, scale: f32, ink: Color32) {
    let mut bounds = Rect::from_two_pos(start * scale, end * scale);
    // Snap across the pen as WPF does, but keep the fractional position along
    // its length. A one-DIP flat cap can cover half of each adjacent pixel.
    if start.y == end.y {
        bounds.min.y = (bounds.min.y - scale * 0.5).round();
        bounds.max.y = bounds.min.y + scale;
    } else {
        bounds.min.x = (bounds.min.x - scale * 0.5).round();
        bounds.max.x = bounds.min.x + scale;
    }
    for y in bounds.top().floor() as i32..bounds.bottom().ceil() as i32 {
        for x in bounds.left().floor() as i32..bounds.right().ceil() as i32 {
            let pixel = Rect::from_min_size(pos2(x as f32, y as f32), Vec2::splat(1.));
            let coverage = bounds.intersect(pixel).area();
            mesh.add_colored_rect(pixel / scale, ink.gamma_multiply(coverage));
        }
    }
}

fn draw_keyboard_cues(ui: &Ui, rect: Rect, ink: Color32) {
    // WPF's focus adorner has a two-DIP margin and a one-on, two-off pen.
    let r = rect.shrink(2.5);
    let corners = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    let clip = ui.clip_rect().expand(1.);
    let scale = ui.ctx().pixels_per_point();
    let mut mesh = Mesh::default();
    let mut perimeter = 0.;
    for pair in corners.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let delta = to - from;
        let length = delta.length();
        if length > 0. && clip.intersects(Rect::from_two_pos(from, to).expand(0.5)) {
            // A selected license document can be much taller than the viewport.
            let direction = delta / length;
            let begin = (clip.clamp(from) - from).dot(direction);
            let end = (clip.clamp(to) - from).dot(direction);
            let mut dash = ((perimeter + begin) / 3.).floor() * 3. - perimeter;
            while dash < end {
                let a = dash.max(0.);
                let b = (dash + 1.).min(length);
                if a < b {
                    paint_focus_dash(
                        &mut mesh,
                        from + direction * a,
                        from + direction * b,
                        scale,
                        ink,
                    );
                }
                dash += 3.;
            }
        }
        perimeter += length;
    }
    ui.painter().add(mesh);
}

impl TypeSearch {
    fn find(
        &mut self,
        labels: &[impl AsRef<str>],
        current: usize,
        text: &str,
        now: f64,
    ) -> Option<usize> {
        if text.is_empty() || text.chars().any(char::is_control) {
            return None;
        }
        let timeout = f64::from(unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime()
        }) / 500.;
        if now - self.last_input > timeout {
            self.prefix.clear();
        }
        let text = text.to_lowercase();
        let next = format!("{}{text}", self.prefix);
        let find = |prefix: &str, start: usize| {
            (start..start + labels.len())
                .map(|i| i % labels.len())
                .find(|&i| labels[i].as_ref().to_lowercase().starts_with(prefix))
        };
        let matched =
            if let Some(index) = find(&next, current + usize::from(self.prefix.is_empty())) {
                self.prefix = next;
                Some(index)
            } else if self.prefix == text {
                find(&text, current + 1)
            } else {
                None
            };
        self.last_input = now;
        matched
    }
}

impl Selection {
    fn navigate(&mut self, index: usize) {
        self.highlighted = index;
        if !self.open {
            self.chosen = index;
        }
    }

    fn key(&mut self, key: Key, modifiers: Modifiers, last: usize) -> bool {
        match (key, modifiers.alt) {
            (Key::F4, false) | (Key::ArrowDown | Key::ArrowUp, true) => {
                if self.open {
                    self.chosen = self.highlighted;
                }
                self.open = !self.open;
            }
            (Key::Escape, _) if self.open => self.open = false,
            (Key::Enter, _) if self.open => {
                self.chosen = self.highlighted;
                self.open = false;
            }
            (Key::ArrowDown, false) => self.navigate((self.highlighted + 1).min(last)),
            (Key::ArrowUp, false) => self.navigate(self.highlighted.saturating_sub(1)),
            (Key::ArrowRight, false) if !self.open => {
                self.navigate((self.highlighted + 1).min(last))
            }
            (Key::ArrowLeft, false) if !self.open => {
                self.navigate(self.highlighted.saturating_sub(1))
            }
            (Key::Home, false) => self.navigate(0),
            (Key::End, false) => self.navigate(last),
            (Key::PageUp, _) if self.open => self.navigate(0),
            (Key::PageDown, _) if self.open => self.navigate(last),
            _ => return false,
        }
        true
    }
}

enum Presentation<'a> {
    Label,
    Stacked(&'a super::windows_text::TextStack),
    Document(&'a [(String, String)]),
}

pub fn choose(
    ui: &mut Ui,
    name: &str,
    width: f32,
    labels: &[impl AsRef<str>],
    selected: usize,
    size: f32,
) -> Option<usize> {
    choose_presented(ui, name, width, labels, selected, size, Presentation::Label)
}

pub fn choose_in_stack(
    ui: &mut Ui,
    stack: &super::windows_text::TextStack,
    name: &str,
    labels: &[impl AsRef<str>],
    selected: usize,
    size: f32,
) -> Option<usize> {
    choose_presented(
        ui,
        name,
        ui.available_width(),
        labels,
        selected,
        size,
        Presentation::Stacked(stack),
    )
}

pub fn choose_document(
    ui: &mut Ui,
    documents: &[(String, String)],
    selected: usize,
) -> Option<usize> {
    let titles: Vec<_> = documents.iter().map(|(title, _)| title.as_str()).collect();
    choose_presented(
        ui,
        "라이선스 문서",
        ui.available_width(),
        &titles,
        selected,
        13.,
        Presentation::Document(documents),
    )
}

// The template aligns a one-DIP stroked Path (0,0 → 4,4 → 8,0) inside
// its border. WPF includes the miter and the far end of the flat cap in
// the Path's desired size, then centers that box vertically.
fn draw_arrow(ui: &Ui, field: Rect, ink: Color32) {
    let diagonal = std::f32::consts::FRAC_1_SQRT_2;
    let size = vec2(8. + diagonal / 2., 4. + diagonal);
    let origin = pos2(field.right() - 11. - size.x, field.center().y - size.y / 2.);
    let bounds = Rect::from_min_max(origin - Vec2::splat(diagonal / 2.), origin + size);
    if !ui.is_rect_visible(bounds) {
        return;
    }
    let scale = ui.pixels_per_point();
    let cap = diagonal / 2.;
    // WPF snaps the widened path vertices to sixteenths of a backing pixel
    // before calculating coverage on eight scan lines per pixel.
    let vertices = [
        vec2(-cap, cap),
        vec2(cap, -cap),
        vec2(4., 4. - diagonal),
        vec2(8. - cap, -cap),
        vec2(8. + cap, cap),
        vec2(4., 4. + diagonal),
    ]
    .map(|offset| {
        let point = (origin + offset) * scale * 16.;
        pos2(point.x.round(), point.y.round()) / 16.
    });
    let bounds = Rect::from_points(&vertices);
    let left = bounds.left().floor() as i32;
    let right = bounds.right().ceil() as i32;
    let mut coverage = vec![0_u8; (right - left) as usize];
    let mut mesh = Mesh::default();
    for y in bounds.top().floor() as i32..bounds.bottom().ceil() as i32 {
        coverage.fill(0);
        for sample in 0..8 {
            let scan_y = y as f32 + sample as f32 / 8.;
            let mut crossings = [0.; 6];
            let mut count = 0;
            let mut start = vertices[5];
            for end in vertices {
                if (start.y <= scan_y) != (end.y <= scan_y) {
                    let x = start.x + (end.x - start.x) * (scan_y - start.y) / (end.y - start.y);
                    crossings[count] = x;
                    count += 1;
                }
                start = end;
            }
            let crossings = &mut crossings[..count];
            crossings.sort_unstable_by(f32::total_cmp);
            for span in crossings.chunks_exact(2) {
                let begin = (span[0] * 8.).ceil() as i32;
                let end = (span[1] * 8.).ceil() as i32;
                for x in begin.div_euclid(8)..=(end - 1).div_euclid(8) {
                    coverage[(x - left) as usize] +=
                        (end.min((x + 1) * 8) - begin.max(x * 8)) as u8;
                }
            }
        }
        for (x, &covered) in (left..right).zip(&coverage) {
            if covered > 0 {
                let pixel = Rect::from_min_size(pos2(x as f32, y as f32), Vec2::splat(1.));
                mesh.add_colored_rect(pixel / scale, ink.gamma_multiply(covered as f32 / 64.));
            }
        }
    }
    ui.painter().add(Shape::mesh(mesh));
}

fn choose_presented(
    ui: &mut Ui,
    name: &str,
    width: f32,
    labels: &[impl AsRef<str>],
    selected: usize,
    size: f32,
    presentation: Presentation<'_>,
) -> Option<usize> {
    let id = ui.make_persistent_id(Id::new(name));
    let popup = id.with("popup");
    let was_open = ComboBox::is_open(ui.ctx(), id);
    let highlight = id.with("highlighted");
    let mut state = Selection {
        open: was_open,
        highlighted: ui
            .data(|d| d.get_temp::<usize>(highlight))
            .filter(|i| *i < labels.len() && was_open)
            .unwrap_or(selected),
        chosen: selected,
    };
    // UI Automation focus and the user's next key can reach the same frame.
    // Register that focus before interpreting keys, as ComboBox::show_ui runs later.
    if ui.is_enabled() && ui.input(|i| i.has_accesskit_action_request(id, accesskit::Action::Focus))
    {
        ui.memory_mut(|m| m.interested_in_focus(id, ui.layer_id()));
    }
    let focused = ui.memory(|m| m.has_focus(id));
    if ui.is_enabled() && (focused || was_open) {
        let search_id = id.with("typed prefix");
        let mut search = ui
            .data(|d| d.get_temp::<TypeSearch>(search_id))
            .unwrap_or_default();
        let mut handled_focus_key = false;
        ui.input_mut(|input| {
            let now = input.time;
            let allow_text =
                !input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.command;
            let composed = input
                .events
                .iter()
                .any(|e| matches!(e, Event::Text(_) | Event::Ime(_)));
            input.events.retain(|e| match e {
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    let handled = state.key(*key, *modifiers, labels.len() - 1);
                    if matches!(
                        key,
                        Key::ArrowUp
                            | Key::ArrowDown
                            | Key::ArrowLeft
                            | Key::ArrowRight
                            | Key::Escape
                            | Key::Tab
                    ) {
                        handled_focus_key = handled;
                    }
                    if handled {
                        search.prefix.clear();
                    } else if !composed
                        && allow_text
                        && key.name().len() == 1
                        && key.name().as_bytes()[0].is_ascii_alphanumeric()
                        && let Some(index) = search.find(labels, state.highlighted, key.name(), now)
                    {
                        state.navigate(index);
                        return false;
                    }
                    !handled
                }
                Event::Text(text) | Event::Ime(ImeEvent::Commit(text)) if allow_text => {
                    if let Some(index) = search.find(labels, state.highlighted, text, now) {
                        state.navigate(index);
                    }
                    false
                }
                _ => true,
            })
        });
        if handled_focus_key {
            // egui schedules directional focus movement before the widget sees
            // input. The focus-lock filter starts one frame after gaining focus.
            ui.memory_mut(|m| m.move_focus(FocusDirection::None));
        }
        ui.data_mut(|d| d.insert_temp(search_id, search));
    }
    if ui.is_enabled() {
        ui.input_mut(|i| {
            i.consume_accesskit_action_requests(id, |r| {
                match r.action {
                    accesskit::Action::Expand => state.open = true,
                    accesskit::Action::Collapse => state.open = false,
                    _ => return false,
                }
                true
            })
        });
    }
    if state.open != was_open {
        if state.open {
            Popup::open_id(ui.ctx(), popup);
        } else {
            Popup::close_id(ui.ctx(), popup);
        }
        ui.memory_mut(|m| m.request_focus(id));
    }
    let ink = ui.visuals().text_color();
    // The WPF selection field keeps its surface when focused, pressed or open.
    // Only the popup items use the highlighted background.
    let widgets = ui.visuals().widgets.clone();
    let surface = widgets.inactive.weak_bg_fill;
    let interact_size = ui.spacing().interact_size;
    let button_padding = ui.spacing().button_padding;
    let icon_width = ui.spacing().icon_width;
    // The selection presenter starts 10 DIPs inside the field. This margin
    // belongs to the ComboBox even when the surrounding page uses wider buttons.
    ui.spacing_mut().button_padding = vec2(10., 7.);
    let line_height = super::windows_text::line_height(size).unwrap_or(size);
    // Both the label and arrow are painted below. Size their empty egui
    // placeholder from the WPF text line so it does not impose a 16-DIP row.
    ui.spacing_mut().icon_width = line_height;
    let document = match presentation {
        Presentation::Document(docs) => Some(&docs[state.chosen]),
        _ => None,
    };
    let lines = document.map_or(1, |(_, body)| {
        body.bytes().filter(|&b| b == b'\n').count() + 1
    });
    ui.spacing_mut().interact_size.y = line_height * lines as f32 + 14.;
    {
        let field = &mut ui.visuals_mut().widgets;
        for state in [
            &mut field.inactive,
            &mut field.hovered,
            &mut field.active,
            &mut field.open,
        ] {
            state.weak_bg_fill = Color32::TRANSPARENT;
            state.bg_stroke = Stroke::NONE;
        }
    }
    let response = ComboBox::from_id_salt(name)
        .width(width)
        .selected_text(RichText::new("").size(1.))
        .icon(|_, _, _, _| {})
        .show_ui(ui, |ui| {
            ui.spacing_mut().interact_size = interact_size;
            ui.spacing_mut().button_padding = button_padding;
            ui.spacing_mut().icon_width = icon_width;
            ui.visuals_mut().widgets = widgets.clone();
            for (index, label) in labels.iter().enumerate() {
                let item = ui.selectable_label(state.highlighted == index, label.as_ref());
                if item.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                    state.highlighted = index;
                }
                ui.ctx().accesskit_node_builder(item.id, |node| {
                    node.set_role(accesskit::Role::ListBoxOption);
                    node.set_selected(index == state.chosen);
                });
                if item.clicked() {
                    state.chosen = index;
                }
            }
        })
        .response;
    ui.spacing_mut().interact_size = interact_size;
    ui.spacing_mut().button_padding = button_padding;
    ui.spacing_mut().icon_width = icon_width;
    let mut border = widgets.inactive.bg_stroke;
    ui.visuals_mut().widgets = widgets;
    let mut bounds = response.rect;
    if let Presentation::Stacked(stack) = presentation {
        // The StackPanel places its selector after the preceding natural text
        // heights. egui rounds its input allocation to a coarser logical grid.
        let scale = ui.pixels_per_point();
        let right = (ui.ctx().content_rect().right() * scale).round() / scale;
        bounds.min.y = stack.top(bounds.top());
        bounds.max.y = bounds.min.y + line_height + 14.;
        bounds.max.x = right - (right - bounds.right()).round();
    }
    if response.hovered() {
        let theme = if ui.visuals().dark_mode {
            "dark"
        } else {
            "light"
        };
        border.color = super::style::Palette::new(ui.ctx(), theme).muted;
    }
    ui.painter()
        .rect(bounds, 7, surface, border, StrokeKind::Inside);
    let start = bounds.min + vec2(10., 7.);
    let content = Rect::from_min_max(start, bounds.right_bottom() - vec2(28., 7.));
    let clip = ui.clip_rect().intersect(content);
    if let Some((title, body)) = document {
        // The WPF ContentPresenter displays the selected record's ToString().
        // DisplayMemberPath supplies just the title to items in the open list.
        // Lay out its complete height but rasterize only visible lines of long notices.
        if clip.is_positive() {
            let first = ((clip.top() - start.y) / line_height).floor().max(0.) as usize;
            let end = ((clip.bottom() - start.y) / line_height).ceil() as usize;
            let painter = ui.painter().with_clip_rect(clip);
            for (index, line) in body
                .split('\n')
                .enumerate()
                .skip(first)
                .take(end.saturating_sub(first))
            {
                let mut text = line.trim_end_matches('\r').to_owned();
                if index == 0 {
                    text = format!("LicenseDocument {{ Title = {title}, Text = {text}");
                }
                if index + 1 == lines {
                    text.push_str(" }");
                }
                if !text.is_empty() {
                    painter.text(
                        start + vec2(0., index as f32 * line_height),
                        Align2::LEFT_TOP,
                        text,
                        FontId::proportional(size),
                        ink,
                    );
                }
            }
        }
    } else if clip.is_positive() {
        // WPF clips the selection to its presenter when the field is narrow.
        let text = labels[state.chosen].as_ref();
        let painter = ui.painter().with_clip_rect(clip);
        if let Some(label) = super::windows_text::MeasuredLabel::natural(ui, text, size, false) {
            label.paint(&painter, start, ink);
        } else {
            painter.text(
                content.left_center(),
                Align2::LEFT_CENTER,
                text,
                FontId::proportional(size),
                ink,
            );
        }
    }
    draw_arrow(ui, bounds, ink);
    let open = ComboBox::is_open(ui.ctx(), id);
    if open {
        ui.data_mut(|d| d.insert_temp(highlight, state.highlighted));
    } else {
        ui.data_mut(|d| d.remove::<usize>(highlight));
    }
    let show_focus = keyboard_cues(ui, &response);
    if response.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                EventFilter {
                    vertical_arrows: true,
                    horizontal_arrows: true,
                    // F4 and Escape can arrive before the open list gets a frame.
                    // An unhandled Escape is still available to the dialog.
                    escape: true,
                    ..Default::default()
                },
            )
        });
        super::windows_border::rounded_rectangle(
            ui,
            bounds,
            7.,
            Color32::TRANSPARENT,
            Stroke::new(1.0_f32, ink),
        );
        if show_focus {
            draw_keyboard_cues(ui, bounds, ink);
        }
    }
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_label(name);
        node.set_value(labels[state.chosen].as_ref());
        node.set_expanded(open);
        if response.enabled() {
            node.add_action(if open {
                accesskit::Action::Collapse
            } else {
                accesskit::Action::Expand
            });
        }
    });
    (state.chosen != selected).then_some(state.chosen)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_selection_stays_inside_the_fixed_width_presenter() {
        let ctx = Context::default();
        let name = "Provider 필터";
        let title = "A selected item wider than the entire field";
        let mut field = Rect::NOTHING;
        let output = ctx.run(RawInput::default(), |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                let id = ui.make_persistent_id(Id::new(name));
                choose(ui, name, 100., &[title], 0, 12.);
                field = ctx.read_response(id).unwrap().rect;
            });
        });
        assert_eq!(field.width(), 100.);
        let line_height = crate::windows_text::line_height(12.).unwrap_or(12.);
        assert!((field.height() - (line_height + 14.)).abs() <= 1. / 32.);
        let label = output
            .shapes
            .iter()
            .find(|entry| matches!(&entry.shape, Shape::Text(text) if text.galley.text() == title))
            .expect("selected text");
        assert_eq!(label.clip_rect.left(), field.left() + 10.);
        assert_eq!(label.clip_rect.right(), field.right() - 28.);
    }

    #[test]
    fn horizontal_focus_dash_covers_one_device_pixel_at_the_native_phase() {
        let mut mesh = Mesh::default();
        paint_focus_dash(
            &mut mesh,
            pos2(30.5, 98.4),
            pos2(31.5, 98.4),
            1.,
            Color32::BLACK,
        );
        assert_eq!(mesh.vertices.len(), 8);
        assert_eq!(
            mesh.calc_bounds(),
            Rect::from_min_max(pos2(30., 98.), pos2(32., 99.))
        );
        assert!(
            mesh.vertices
                .iter()
                .all(|v| v.color == Color32::from_black_alpha(128))
        );
    }

    #[test]
    fn newly_opened_selector_uses_the_previous_input_device() {
        let ctx = Context::default();
        track_input(&ctx);
        let key = Event::Key {
            key: Key::A,
            physical_key: Some(Key::A),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        let dialog = |name: &str, event: Event| {
            // The selector wasn't present when the owner received this input.
            let _ = ctx.run(
                RawInput {
                    events: vec![event],
                    ..Default::default()
                },
                |_| {},
            );
            let mut visible = false;
            let _ = ctx.run(RawInput::default(), |ctx| {
                CentralPanel::default().show(ctx, |ui| {
                    let id = ui.make_persistent_id(Id::new(name));
                    ui.memory_mut(|m| m.request_focus(id));
                    choose(ui, name, 120., &["Codex", "Claude"], 0, 13.);
                    assert!(ui.memory(|m| m.has_focus(id)));
                    visible = ui.data(|d| {
                        d.get_temp::<bool>(id.with("keyboard focus visual"))
                            .unwrap()
                    });
                });
            });
            visible
        };
        assert!(dialog("connecting provider", key));
        let pointer = Event::PointerButton {
            pos: pos2(10., 10.),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        };
        assert!(!dialog("license document", pointer));
    }

    #[test]
    fn first_arrow_after_focus_keeps_keyboard_on_provider() {
        let ctx = Context::default();
        track_input(&ctx);
        let mut selected = 0;
        let mut path = String::new();
        let key = |key| Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        let mut frame = |events: Vec<Event>| {
            let input = RawInput {
                events,
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400., 300.))),
                ..Default::default()
            };
            let mut provider = Id::NULL;
            let _ = ctx.run(input, |ctx| {
                CentralPanel::default().show(ctx, |ui| {
                    provider = ui.make_persistent_id(Id::new("연결 서비스"));
                    if let Some(index) = choose(
                        ui,
                        "연결 서비스",
                        220.,
                        &["Codex", "Claude", "Gemini"],
                        selected,
                        13.,
                    ) {
                        selected = index;
                    }
                    ui.text_edit_singleline(&mut path);
                });
            });
            (provider, selected)
        };
        let (provider, _) = frame(vec![]);
        let focus = Event::AccessKitActionRequest(accesskit::ActionRequest {
            action: accesskit::Action::Focus,
            target: provider.value().into(),
            data: None,
        });
        frame(vec![focus.clone()]);
        assert_eq!(frame(vec![key(Key::ArrowDown), focus.clone()]).1, 1);
        let shows_keyboard_cues = || {
            ctx.data(|d| {
                d.get_temp::<bool>(provider.with("keyboard focus visual"))
                    .unwrap()
            })
        };
        assert!(
            !shows_keyboard_cues(),
            "selecting and refocusing in one frame"
        );
        frame(vec![focus.clone()]);
        frame(vec![]);
        assert!(
            !shows_keyboard_cues(),
            "refocusing the same selection later"
        );
        assert_eq!(ctx.memory(|m| m.focused()), Some(provider));
        assert_eq!(frame(vec![key(Key::Home)]).1, 0);
        frame(vec![key(Key::Tab)]);
        assert!(shows_keyboard_cues());
        assert_ne!(ctx.memory(|m| m.focused()), Some(provider));

        assert_eq!(frame(vec![focus, key(Key::ArrowDown)]).1, 1);
        assert!(
            shows_keyboard_cues(),
            "switching back from another field after Tab"
        );
        assert_eq!(ctx.memory(|m| m.focused()), Some(provider));
        frame(vec![key(Key::Home)]);
        frame(vec![key(Key::F4), key(Key::ArrowDown), key(Key::Escape)]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(provider));
        assert_eq!(
            frame(vec![key(Key::F4), key(Key::ArrowDown), key(Key::Enter)]).1,
            1
        );
    }
}
