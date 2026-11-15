//! The dashboard's ScrollViewer pages on a track press and drags from the grabbed point.
use crate::style::Palette;
use eframe::egui::{scroll_area::ScrollSource, *};
use std::time::Duration;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SPI_GETKEYBOARDDELAY, SPI_GETKEYBOARDSPEED, SystemParametersInfoW,
};

#[derive(Clone, Copy)]
enum Press {
    Thumb(f32),
    Page {
        direction: f32,
        next: f64,
        interval: f64,
    },
}

fn repeat_timing() -> (f64, f64) {
    let (mut delay, mut speed) = (0_u32, 31_u32);
    unsafe {
        SystemParametersInfoW(SPI_GETKEYBOARDDELAY, 0, (&mut delay as *mut u32).cast(), 0);
        SystemParametersInfoW(SPI_GETKEYBOARDSPEED, 0, (&mut speed as *mut u32).cast(), 0);
    }
    // WPF RepeatButton uses the system keyboard delay and repeat interval.
    (
        0.25 * f64::from(1 + delay.min(3)),
        f64::from((31 - speed.min(31)) * (400 - 33) / 31 + 33) / 1000.,
    )
}

pub fn show(ui: &mut Ui, scroll: ScrollArea, palette: Palette, body: impl FnOnce(&mut Ui)) {
    let original = ui.visuals().clone();
    // Keep egui's content measurement, scroll-to-widget, touch and mouse wheel.
    // The track retains its allocated width even while we draw its inset thumb.
    let visuals = ui.visuals_mut();
    visuals.extreme_bg_color = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    let mut area = scroll
        // ScrollToVerticalOffset also applies activity navigation immediately on Windows.
        .animated(false)
        .scroll_source(ScrollSource {
            scroll_bar: false,
            ..Default::default()
        })
        .show(ui, |ui| {
            *ui.visuals_mut() = original.clone();
            body(ui);
        });
    *ui.visuals_mut() = original;
    let view = area.inner_rect;
    let track = Rect::from_min_max(
        view.right_top(),
        pos2(
            ui.max_rect().right().min(ui.clip_rect().right()),
            view.bottom(),
        ),
    );
    let maximum = area.content_size.y - view.height();
    if maximum <= 0. || track.width() <= 4. || track.height() <= 6. {
        return;
    }
    let length = (track.height() * view.height() / area.content_size.y)
        .max(24.)
        .min(track.height());
    let travel = track.height() - length;
    if travel <= 0. {
        return;
    }
    let thumb_at = |offset: f32| {
        Rect::from_min_size(
            pos2(
                track.left(),
                track.top() + travel * (offset / maximum).clamp(0., 1.),
            ),
            vec2(track.width(), length),
        )
    };
    let mut offset = area.state.offset.y;
    let thumb = thumb_at(offset);
    let id = area.id.with("page-and-drag");
    let response = ui.interact(
        track,
        id,
        Sense::click_and_drag() - Sense::focusable_noninteractive(),
    );
    let mut press = ui.data(|d| d.get_temp::<Press>(id));
    let (now, pos, origin, primary) = ui.input(|i| {
        (
            i.time,
            i.pointer.interact_pos(),
            i.pointer.press_origin(),
            i.pointer.button_down(PointerButton::Primary),
        )
    });
    let held = primary && response.is_pointer_button_down_on();
    if let Some(pos) = pos
        && ui.is_enabled()
        && (held
            || response.clicked_by(PointerButton::Primary)
            || response.drag_stopped_by(PointerButton::Primary))
    {
        if press.is_none() {
            let origin = origin.unwrap_or(pos);
            if thumb.contains(origin) {
                press = Some(Press::Thumb(origin.y - thumb.top()));
            } else {
                let direction = if origin.y < thumb.top() { -1. } else { 1. };
                let (delay, interval) = repeat_timing();
                offset += direction * view.height();
                press = Some(Press::Page {
                    direction,
                    next: now + delay,
                    interval,
                });
            }
        }
        match &mut press {
            Some(Press::Thumb(grab)) => offset = (pos.y - track.top() - *grab) / travel * maximum,
            Some(Press::Page {
                direction,
                next,
                interval,
            }) if held => {
                if now >= *next {
                    if track.contains(pos)
                        && ((*direction < 0. && pos.y < thumb.top())
                            || (*direction > 0. && pos.y > thumb.bottom()))
                    {
                        offset += *direction * view.height();
                    }
                    *next = now + *interval;
                }
                ui.ctx()
                    .request_repaint_after(Duration::from_secs_f64((*next - now).max(0.)));
            }
            _ => {}
        }
    }
    if let Some(press) = press {
        ui.data_mut(|d| {
            if held {
                d.insert_temp(id, press)
            } else {
                d.remove::<Press>(id)
            }
        });
    }
    if ui.is_enabled() {
        ui.input_mut(|i| {
            i.consume_accesskit_action_requests(id, |request| {
                match (request.action, &request.data) {
                    (accesskit::Action::SetValue, Some(accesskit::ActionData::NumericValue(y)))
                        if y.is_finite() =>
                    {
                        offset = *y as f32
                    }
                    (accesskit::Action::ScrollUp, _) => offset -= view.height(),
                    (accesskit::Action::ScrollDown, _) => offset += view.height(),
                    _ => return false,
                }
                true
            })
        });
    }
    offset = offset.clamp(0., maximum);
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(accesskit::Role::ScrollBar);
        node.set_orientation(accesskit::Orientation::Vertical);
        node.set_numeric_value(offset as f64);
        node.set_min_numeric_value(0.);
        node.set_max_numeric_value(maximum as f64);
        node.set_numeric_value_jump(view.height() as f64);
        if ui.is_enabled() {
            node.add_action(accesskit::Action::SetValue);
            node.add_action(accesskit::Action::ScrollUp);
            node.add_action(accesskit::Action::ScrollDown);
        }
    });
    if offset != area.state.offset.y {
        area.state.offset.y = offset;
        area.state.store(ui.ctx(), area.id);
        ui.ctx().request_repaint();
    }
    let thumb = thumb_at(offset);
    let hovered = response.hovered() && pos.is_some_and(|p| thumb.contains(p));
    // Premultiply the brush and backdrop separately as in the original WPF control.
    let alpha = if hovered || held && matches!(press, Some(Press::Thumb(_))) {
        179
    } else {
        77
    };
    let channel = |bg: u8, ink: u8| {
        (((bg as u32 * (255 - alpha) + 127) / 255) + ((ink as u32 * alpha + 127) / 255)).min(255)
            as u8
    };
    let fill = Color32::from_gray(channel(palette.bg.r(), palette.muted.r()));
    ui.painter()
        .rect_filled(thumb.shrink2(vec2(2., 3.)), 3., fill);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_press_pages_and_thumb_drag_keeps_its_grab_point() {
        let ctx = Context::default();
        let palette = Palette::new(&ctx, "light");
        palette.apply(&ctx);
        let frame = |time, events| {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(200., 200.))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut offset = 0.;
            let _ = ctx.run(input, |ctx| {
                CentralPanel::default().frame(Frame::NONE).show(ctx, |ui| {
                    let id = ui.make_persistent_id(Id::new("body"));
                    show(
                        ui,
                        ScrollArea::vertical()
                            .auto_shrink([false; 2])
                            .id_salt("body"),
                        palette,
                        |ui| {
                            ui.allocate_space(vec2(80., 1000.));
                        },
                    );
                    offset = scroll_area::State::load(ctx, id).unwrap().offset.y;
                });
            });
            offset
        };
        let point = |y| pos2(193., y);
        let button = |y, pressed| Event::PointerButton {
            pos: point(y),
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(0., vec![]);
        frame(0.5, vec![]);
        frame(1., vec![Event::PointerMoved(point(175.))]);
        assert_eq!(frame(1.01, vec![button(175., true)]), 200.);
        assert_eq!(frame(1.03, vec![button(175., false)]), 200.);
        frame(1.1, vec![Event::PointerMoved(point(60.))]);
        assert_eq!(frame(1.11, vec![button(60., true)]), 200.);
        assert_eq!(frame(1.2, vec![Event::PointerMoved(point(80.))]), 300.);
        assert_eq!(frame(1.21, vec![button(80., false)]), 300.);
        frame(1.3, vec![Event::PointerMoved(point(10.))]);
        assert_eq!(frame(1.31, vec![button(10., true)]), 100.);
        assert_eq!(frame(2.5, vec![]), 0.);
    }
}
