use block2::StackBlock;
use eframe::egui::{
    Color32, ColorImage, Context, Id, Pos2, Rect, TextureHandle, TextureOptions, Ui, pos2,
    scroll_area,
};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, sel,
};
use objc2_app_kit::{
    NSAppearanceCustomization, NSControlSize, NSScroller, NSScrollerPart, NSScrollerStyle, NSView,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

struct Binding {
    ctx: Context,
    area: Cell<Option<(Id, f32, f32)>>,
}

define_class!(
    #[unsafe(super=NSObject)]
    #[thread_kind=MainThreadOnly]
    #[ivars=Binding]
    #[name="QuotaBarDashboardScrollerTarget"]
    struct Target;
    unsafe impl NSObjectProtocol for Target {}
    impl Target {
        #[unsafe(method(scrollDashboard:))]
        fn scroll_dashboard(&self, bar: &NSScroller) {
            let binding = self.ivars();
            let Some((id, page, range)) = binding.area.get() else { return };
            if let Some(mut state) = scroll_area::State::load(&binding.ctx, id) {
                let offset = match bar.hitPart() {
                    NSScrollerPart::IncrementPage => state.offset.y + page,
                    NSScrollerPart::DecrementPage => state.offset.y - page,
                    NSScrollerPart::Knob | NSScrollerPart::KnobSlot | NSScrollerPart::NoPart =>
                        bar.doubleValue() as f32 * range,
                    _ => return,
                };
                state.offset.y = offset.clamp(0., range);
                bar.setDoubleValue(f64::from(state.offset.y / range));
                state.store(&binding.ctx, id);
                binding.ctx.request_repaint();
            }
        }
    }
);

pub struct NativeScroller {
    bar: Retained<NSScroller>,
    target: Retained<Target>,
    capture: RefCell<Option<(Id, TextureHandle)>>,
}

impl NativeScroller {
    pub fn attach(cc: &eframe::CreationContext<'_>) -> Option<Rc<Self>> {
        let main = MainThreadMarker::new()?;
        let RawWindowHandle::AppKit(handle) = cc.window_handle().ok()?.as_raw() else {
            return None;
        };
        let parent = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let bar = NSScroller::initWithFrame(
            NSScroller::alloc(main),
            NSRect::new(NSPoint::ZERO, NSSize::new(15., 100.)),
        );
        bar.setScrollerStyle(NSScrollerStyle::Legacy);
        bar.setHidden(true);
        bar.setEnabled(true);
        let target: Retained<Target> = unsafe {
            msg_send![
                super(Target::alloc(main).set_ivars(Binding {
                    ctx: cc.egui_ctx.clone(),
                    area: Cell::new(None)
                })),
                init
            ]
        };
        unsafe {
            bar.setTarget(Some(&target));
            bar.setAction(Some(sel!(scrollDashboard:)));
        }
        parent.addSubview(&bar);
        Some(Rc::new(Self {
            bar,
            target,
            capture: RefCell::default(),
        }))
    }

    pub fn legacy_width(&self) -> Option<f32> {
        let main = self.bar.mtm();
        let style = NSScroller::preferredScrollerStyle(main);
        (style == NSScrollerStyle::Legacy).then(|| {
            NSScroller::scrollerWidthForControlSize_scrollerStyle(
                NSControlSize::Regular,
                style,
                main,
            ) as f32
                / self.target.ivars().ctx.zoom_factor()
        })
    }

    pub fn hide(&self) {
        self.target.ivars().area.set(None);
        self.bar.setHidden(true);
    }

    // A framebuffer export cannot include NSView subviews. Draw the same live
    // scroller into that export, including its native insets and appearance.
    pub fn paint_capture(&self, ui: &Ui, rect: Rect) -> Option<()> {
        if self.bar.isHidden() {
            return None;
        }
        let bar = &self.bar;
        let appearance = bar.effectiveAppearance();
        let scale = ui.ctx().pixels_per_point();
        let width = (rect.width() * scale).round() as isize;
        let height = (rect.height() * scale).round() as isize;
        let key = Id::new((
            width,
            height,
            scale.to_bits(),
            bar.knobProportion().to_bits(),
            bar.doubleValue().to_bits(),
            appearance.name().to_string(),
        ));
        let mut capture = self.capture.borrow_mut();
        if capture.as_ref().is_none_or(|(id, _)| *id != key) {
            let rep = super::bitmap_rep(width, height)?;
            rep.setSize(bar.bounds().size);
            appearance.performAsCurrentDrawingAppearance(&StackBlock::new(|| {
                bar.cacheDisplayInRect_toBitmapImageRep(bar.bounds(), &rep);
            }));
            let rgba = super::bitmap_pixels(&rep)?;
            let texture = ui.ctx().load_texture(
                "AppKit scroller",
                ColorImage::from_rgba_unmultiplied(
                    [rgba.width() as usize, rgba.height() as usize],
                    rgba.as_raw(),
                ),
                TextureOptions::NEAREST,
            );
            *capture = Some((key, texture));
        }
        let (_, texture) = capture.as_ref()?;
        ui.painter().image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
            Color32::WHITE,
        );
        Some(())
    }

    pub fn place(&self, id: Id, view: Rect, width: f32, content: f32, offset: f32) {
        // eframe owns the dashboard's root view throughout this main-thread update.
        let Some(parent) = (unsafe { self.bar.superview() }) else {
            return;
        };
        if content <= view.height() || view.height() <= 0. || width <= 0. {
            self.hide();
            return;
        }
        let binding = self.target.ivars();
        let zoom = binding.ctx.zoom_factor();
        let points = |n: f32| f64::from(n * zoom);
        let bounds = parent.bounds();
        let y = if parent.isFlipped() {
            points(view.top())
        } else {
            bounds.size.height - points(view.bottom())
        };
        let frame = NSRect::new(
            NSPoint::new(bounds.origin.x + points(view.right()), bounds.origin.y + y),
            NSSize::new(points(width), points(view.height())),
        );
        if self.bar.frame() != frame {
            self.bar.setFrame(frame);
        }
        let range = content - view.height();
        // The egui viewport owns the content offset. Only a user action on
        // the AppKit scroller may change it outside an egui frame.
        self.bar
            .setKnobProportion(f64::from(view.height() / content));
        self.bar
            .setDoubleValue(f64::from((offset / range).clamp(0., 1.)));
        self.bar.setHidden(false);
        binding.area.set(Some((id, view.height(), range)));
    }
}

impl Drop for NativeScroller {
    fn drop(&mut self) {
        unsafe { self.bar.setTarget(None) };
        self.bar.removeFromSuperview();
    }
}
