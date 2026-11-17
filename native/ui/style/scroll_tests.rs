use super::DESKTOP_SCROLL;
use eframe::egui::{self, scroll_area::ScrollSource, *};

struct ScrollInput {
    ctx: Context,
    source: ScrollSource,
    time: f64,
}

impl ScrollInput {
    fn new(source: ScrollSource) -> Self {
        let ctx = Context::default();
        ctx.style_mut(|style| style.spacing.scroll = egui::style::ScrollStyle::solid());
        let mut input = Self {
            ctx,
            source,
            time: 0.,
        };
        input.frame(vec![]);
        input.frame(vec![]);
        input
    }

    fn frame(&mut self, events: Vec<Event>) -> (f32, Rect) {
        self.time += 0.1;
        let mut result = (0., Rect::NOTHING);
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(200., 200.))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let _ = self.ctx.run(raw, |ctx| {
            CentralPanel::default().frame(Frame::NONE).show(ctx, |ui| {
                let area = ScrollArea::vertical()
                    .id_salt("scroll input")
                    .auto_shrink([false; 2])
                    .scroll_bar_visibility(scroll_area::ScrollBarVisibility::AlwaysVisible)
                    .scroll_source(self.source)
                    .show(ui, |ui| {
                        ui.allocate_space(vec2(80., 1000.));
                    });
                result = (area.state.offset.y, area.inner_rect);
            });
        });
        result
    }

    fn drag(&mut self, start: Pos2, end: Pos2) -> f32 {
        self.frame(vec![Event::PointerMoved(start)]);
        self.frame(vec![button(start, true)]);
        self.frame(vec![Event::PointerMoved(end)]);
        self.frame(vec![button(end, false)]);
        self.frame(vec![]).0
    }
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn content_stays_fixed_during_drag_while_wheel_and_scrollbar_work() {
    let mut original = ScrollInput::new(ScrollSource::ALL);
    let (_, view) = original.frame(vec![]);
    let start = view.center() + vec2(0., 40.);
    let end = start - vec2(0., 100.);
    assert!(
        original.drag(start, end) > 0.,
        "The input must reproduce egui's default content dragging"
    );

    let mut desktop = ScrollInput::new(DESKTOP_SCROLL);
    assert_eq!(desktop.drag(start, end), 0.);
    desktop.frame(vec![Event::PointerMoved(view.center())]);
    desktop.frame(vec![Event::MouseWheel {
        unit: MouseWheelUnit::Point,
        delta: vec2(0., -80.),
        modifiers: Modifiers::NONE,
    }]);
    assert!(
        desktop.frame(vec![]).0 > 0.,
        "Mouse wheel scrolling must remain available"
    );

    let mut scrollbar = ScrollInput::new(DESKTOP_SCROLL);
    let (_, view) = scrollbar.frame(vec![]);
    let start = pos2(view.right() + 6., view.top() + 12.);
    assert!(
        scrollbar.drag(start, start + vec2(0., 50.)) > 0.,
        "The scrollbar thumb must remain draggable"
    );
}
