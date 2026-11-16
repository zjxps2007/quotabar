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

#[derive(Clone, Copy, Default)]
struct KeyboardCues {
    last_input_keyboard: bool,
    visible: bool,
}

fn keyboard_cues(ui: &Ui, response: &Response) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETKEYBOARDCUES, SystemParametersInfoW};

    let id = response.id.with("keyboard focus visual");
    let mut cues = ui
        .data(|d| d.get_temp::<KeyboardCues>(id))
        .unwrap_or_default();
    if response.gained_focus() {
        cues.visible = cues.last_input_keyboard;
    }
    ui.input(|input| {
        for event in &input.raw.events {
            match event {
                Event::PointerButton { pressed: true, .. } => {
                    cues.last_input_keyboard = false;
                    cues.visible = false;
                }
                Event::AccessKitActionRequest(r)
                    if r.action == accesskit::Action::Focus
                        && r.target == response.id.value().into() =>
                {
                    cues.visible = cues.last_input_keyboard;
                }
                Event::Key {
                    key, pressed: true, ..
                } => {
                    cues.last_input_keyboard = true;
                    // WPF processes navigation after the ComboBox handles its own keys.
                    // Changing the selection in place does not create a focus adorner.
                    if *key == Key::Tab
                        || (matches!(
                            key,
                            Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown
                        ) && input.events.contains(event))
                    {
                        cues.visible = true;
                    }
                }
                _ => {}
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(id, cues));
    response.has_focus()
        && (cues.visible || {
            let mut always = 0_i32;
            unsafe { SystemParametersInfoW(SPI_GETKEYBOARDCUES, 0, (&raw mut always).cast(), 0) };
            always != 0
        })
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
                    ui.painter().line_segment(
                        [from + direction * a, from + direction * b],
                        Stroke::new(1.0_f32, ink),
                    );
                }
                dash += 3.;
            }
        }
        perimeter += length;
    }
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

pub fn choose(
    ui: &mut Ui,
    name: &str,
    width: f32,
    labels: &[impl AsRef<str>],
    selected: usize,
    size: f32,
) -> Option<usize> {
    choose_presented(ui, name, width, labels, selected, size, None)
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
        Some(documents),
    )
}

fn choose_presented(
    ui: &mut Ui,
    name: &str,
    width: f32,
    labels: &[impl AsRef<str>],
    selected: usize,
    size: f32,
    documents: Option<&[(String, String)]>,
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
    let line_height = super::windows_text::line_height(size).unwrap_or(size);
    let document = documents.map(|docs| &docs[state.chosen]);
    let show_default_arrow = document.is_none();
    let lines = document.map_or(1, |(_, body)| {
        body.bytes().filter(|&b| b == b'\n').count() + 1
    });
    if document.is_some() {
        ui.spacing_mut().interact_size.y = line_height * lines as f32 + 14.;
    }
    {
        let field = &mut ui.visuals_mut().widgets;
        field.active.weak_bg_fill = surface;
        field.hovered.weak_bg_fill = surface;
        field.open.weak_bg_fill = surface;
    }
    let response = ComboBox::from_id_salt(name)
        .width(width)
        .selected_text(
            super::style::rich(
                if document.is_some() {
                    ""
                } else {
                    labels[state.chosen].as_ref()
                },
                size,
                false,
                ink,
            )
            .line_height(super::windows_text::line_height(size)),
        )
        .icon(move |ui, rect, visuals, _| {
            if !show_default_arrow {
                return;
            }
            let center = pos2(rect.right() - 5., rect.center().y);
            let arrow = [vec2(-4., -2.), vec2(0., 2.), vec2(4., -2.)].map(|v| center + v);
            ui.painter().add(Shape::line(
                arrow.to_vec(),
                Stroke::new(1.0_f32, visuals.fg_stroke.color),
            ));
        })
        .show_ui(ui, |ui| {
            ui.spacing_mut().interact_size = interact_size;
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
    ui.visuals_mut().widgets = widgets;
    if let Some((title, body)) = document {
        // The WPF ContentPresenter displays the selected record's ToString().
        // DisplayMemberPath supplies just the title to items in the open list.
        // Lay out its complete height but rasterize only visible lines of long notices.
        let bounds = response.rect;
        let start = bounds.min + vec2(10., 7.);
        let content = Rect::from_min_max(start, bounds.right_bottom() - vec2(28., 7.));
        let clip = ui.clip_rect().intersect(content);
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
        let center = pos2(bounds.right() - 14., bounds.center().y);
        let arrow = [vec2(-4., -2.), vec2(0., 2.), vec2(4., -2.)].map(|v| center + v);
        ui.painter()
            .add(Shape::line(arrow.to_vec(), Stroke::new(1.0_f32, ink)));
    }
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
        ui.painter().rect_stroke(
            response.rect,
            7,
            Stroke::new(1.0_f32, ink),
            StrokeKind::Inside,
        );
        if show_focus {
            draw_keyboard_cues(ui, response.rect, ink);
        }
    } else if response.hovered() {
        let theme = if ui.visuals().dark_mode {
            "dark"
        } else {
            "light"
        };
        let muted = super::style::Palette::new(ui.ctx(), theme).muted;
        ui.painter().rect_stroke(
            response.rect,
            7,
            Stroke::new(1.0_f32, muted),
            StrokeKind::Inside,
        );
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
    fn first_arrow_after_focus_keeps_keyboard_on_provider() {
        let ctx = Context::default();
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
        ctx.memory_mut(|m| m.request_focus(provider));
        assert_eq!(frame(vec![key(Key::ArrowDown)]).1, 1);
        let shows_keyboard_cues = || {
            ctx.data(|d| {
                d.get_temp::<KeyboardCues>(provider.with("keyboard focus visual"))
                    .unwrap()
                    .visible
            })
        };
        assert!(!shows_keyboard_cues());
        assert_eq!(ctx.memory(|m| m.focused()), Some(provider));
        assert_eq!(frame(vec![key(Key::Home)]).1, 0);
        frame(vec![key(Key::Tab)]);
        assert!(shows_keyboard_cues());
        assert_ne!(ctx.memory(|m| m.focused()), Some(provider));

        let focus = Event::AccessKitActionRequest(accesskit::ActionRequest {
            action: accesskit::Action::Focus,
            target: provider.value().into(),
            data: None,
        });
        assert_eq!(frame(vec![focus, key(Key::ArrowDown)]).1, 1);
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
