use block2::StackBlock;
use eframe::egui::{
    Color32, ColorImage, Context, Id, Pos2, Rect, TextureHandle, TextureOptions, Ui, pos2,
    scroll_area,
};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, sel,
};
use objc2_app_kit::{
    NSAppearanceCustomization, NSBorderType, NSClipView, NSControlSize, NSScrollView, NSScroller,
    NSScrollerStyle, NSView, NSViewBoundsDidChangeNotification,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSObjectProtocol, NSPoint, NSRect, NSSize,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

struct Binding {
    ctx: Context,
    area: Cell<Option<(Id, f32)>>,
}

define_class!(
    #[unsafe(super=NSView)]
    #[thread_kind=MainThreadOnly]
    #[ivars=Binding]
    #[name="QuotaBarScrollerDocument"]
    struct Document;
    unsafe impl NSObjectProtocol for Document {}
    impl Document {
        #[unsafe(method(isFlipped))]
        fn flipped(&self) -> bool { true }

        #[unsafe(method(clipMoved:))]
        fn clip_moved(&self, notification: &NSNotification) {
            let binding = self.ivars();
            let Some((id, zoom)) = binding.area.get() else { return };
            let Some(clip) = notification.object().and_then(|view| view.downcast::<NSClipView>().ok()) else { return };
            if let Some(mut state) = scroll_area::State::load(&binding.ctx, id) {
                let offset = clip.bounds().origin.y as f32 / zoom;
                if (state.offset.y - offset).abs() < 0.001 { return; }
                state.offset.y = offset;
                state.store(&binding.ctx, id);
                binding.ctx.request_repaint();
            }
        }
    }
);

pub struct NativeScroller {
    holder: Retained<NSView>,
    scroll: Retained<NSScrollView>,
    doc: Retained<Document>,
    capture: RefCell<Option<(Id, TextureHandle)>>,
}

impl NativeScroller {
    pub fn attach(cc: &eframe::CreationContext<'_>) -> Option<Rc<Self>> {
        let main = MainThreadMarker::new()?;
        let RawWindowHandle::AppKit(handle) = cc.window_handle().ok()?.as_raw() else {
            return None;
        };
        let parent = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let holder = NSView::initWithFrame(NSView::alloc(main), NSRect::ZERO);
        holder.setWantsLayer(true);
        holder.setClipsToBounds(true);
        holder.setHidden(true);
        parent.addSubview(&holder);
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(main), NSRect::ZERO);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setHasHorizontalScroller(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(false);
        scroll.setDrawsBackground(false);
        scroll.setScrollerStyle(NSScrollerStyle::Legacy);
        let doc: Retained<Document> = unsafe {
            msg_send![super(Document::alloc(main).set_ivars(Binding { ctx: cc.egui_ctx.clone(), area: Cell::new(None) })), initWithFrame: NSRect::ZERO]
        };
        scroll.setDocumentView(Some(&doc));
        holder.addSubview(&scroll);
        let clip = scroll.contentView();
        clip.setPostsBoundsChangedNotifications(true);
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &doc,
                sel!(clipMoved:),
                Some(NSViewBoundsDidChangeNotification),
                Some(&clip),
            );
        }
        Some(Rc::new(Self {
            holder,
            scroll,
            doc,
            capture: RefCell::default(),
        }))
    }

    pub fn legacy_width(&self) -> Option<f32> {
        let main = self.holder.mtm();
        let style = NSScroller::preferredScrollerStyle(main);
        (style == NSScrollerStyle::Legacy).then(|| {
            NSScroller::scrollerWidthForControlSize_scrollerStyle(
                NSControlSize::Regular,
                style,
                main,
            ) as f32
                / self.doc.ivars().ctx.zoom_factor()
        })
    }

    pub fn hide(&self) {
        self.doc.ivars().area.set(None);
        self.holder.setHidden(true);
    }

    // A framebuffer export cannot include NSView subviews. Draw the same live
    // scroller into that export, including its native insets and appearance.
    pub fn paint_capture(&self, ui: &Ui, rect: Rect) -> Option<()> {
        if self.holder.isHidden() {
            return None;
        }
        let bar = self.scroll.verticalScroller()?;
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
        let Some(parent) = (unsafe { self.holder.superview() }) else {
            return;
        };
        if content <= view.height() || view.height() <= 0. || width <= 0. {
            self.hide();
            return;
        }
        let binding = self.doc.ivars();
        binding.area.set(None);
        let zoom = binding.ctx.zoom_factor();
        let points = |n: f32| f64::from(n * zoom);
        let bounds = parent.bounds();
        let y = if parent.isFlipped() {
            points(view.top())
        } else {
            bounds.size.height - points(view.bottom())
        };
        let holder = NSRect::new(
            NSPoint::new(bounds.origin.x + points(view.right()), bounds.origin.y + y),
            NSSize::new(points(width), points(view.height())),
        );
        if self.holder.frame() != holder {
            self.holder.setFrame(holder);
        }
        // Leave a point of document inside the scroll view. Clip that point
        // off the leading edge so the entire visible rail belongs to AppKit.
        let frame = NSRect::new(
            NSPoint::new(-1., 0.),
            NSSize::new(1. + points(width), points(view.height())),
        );
        let size = NSSize::new(1., points(content));
        if self.scroll.frame() != frame || self.doc.frame().size != size {
            self.scroll.setFrame(frame);
            self.doc.setFrameSize(size);
            self.scroll.tile();
        }
        let clip = self.scroll.contentView();
        if (clip.bounds().origin.y - points(offset)).abs() >= 0.001 {
            clip.scrollToPoint(NSPoint::new(0., points(offset)));
            self.scroll.reflectScrolledClipView(&clip);
        }
        self.holder.setHidden(false);
        binding.area.set(Some((id, zoom)));
    }
}

impl Drop for NativeScroller {
    fn drop(&mut self) {
        self.doc.ivars().area.set(None);
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(&self.doc) };
        self.holder.removeFromSuperview();
    }
}
