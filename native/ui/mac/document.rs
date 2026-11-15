use block2::RcBlock;
use eframe::egui::{Context, Rect};
use objc2::{MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{
    NSAccessibility, NSApplication, NSAutoresizingMaskOptions, NSBorderType, NSColor, NSEvent,
    NSEventMask, NSFont, NSFontWeightRegular, NSScrollView, NSTextView, NSView,
};
use objc2_foundation::{NSPoint, NSRange, NSRect, NSSize, NSString};
use std::{cell::Cell, ptr::NonNull, rc::Rc};

pub struct DocumentReader {
    scroll: Retained<NSScrollView>,
    text: Retained<NSTextView>,
    selected: Option<usize>,
    cancelled: Rc<Cell<bool>>,
    keyboard: Option<Retained<AnyObject>>,
}

impl DocumentReader {
    fn attach(ctx: &Context, parent: &NSView, main: MainThreadMarker) -> Option<Self> {
        let scroll = NSTextView::scrollableTextView(main);
        let text = scroll.documentView()?.downcast::<NSTextView>().ok()?;
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setHasHorizontalScroller(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setWantsLayer(true);
        if let Some(layer) = scroll.layer() {
            layer.setCornerRadius(10.);
            layer.setMasksToBounds(true);
        }
        text.setEditable(false);
        text.setSelectable(true);
        text.setRichText(false);
        text.setDrawsBackground(false);
        text.setHorizontallyResizable(false);
        text.setVerticallyResizable(true);
        text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        // The scrollable text view's TextKit stack owns this container.
        let container = unsafe { text.textContainer() }?;
        container.setWidthTracksTextView(true);
        container.setLineFragmentPadding(0.);
        text.setTextContainerInset(NSSize::new(16., 16.));
        let font = unsafe { NSFont::monospacedSystemFontOfSize_weight(12., NSFontWeightRegular) };
        text.setFont(Some(&font));
        text.setTextColor(Some(&NSColor::labelColor()));
        parent.addSubview(&scroll);

        // Once the text view has the keyboard, Escape must still dismiss this sheet.
        let cancelled = Rc::new(Cell::new(false));
        let flag = Rc::clone(&cancelled);
        let listener = text.clone();
        let context = ctx.clone();
        let monitor = RcBlock::new(move |event: NonNull<NSEvent>| {
            let key = unsafe { event.as_ref() };
            if key.keyCode() == 53
                && listener
                    .window()
                    .filter(|w| w.windowNumber() == key.windowNumber())
                    .and_then(|w| w.firstResponder())
                    .is_some_and(|r| {
                        Retained::as_ptr(&r).cast::<AnyObject>()
                            == Retained::as_ptr(&listener).cast::<AnyObject>()
                    })
            {
                flag.set(true);
                context.request_repaint_of(eframe::egui::ViewportId::ROOT);
                return std::ptr::null_mut();
            }
            event.as_ptr()
        });
        let keyboard = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &monitor)
        };
        Some(Self {
            scroll,
            text,
            selected: None,
            cancelled,
            keyboard,
        })
    }

    pub fn show(
        slot: &mut Option<Self>,
        ctx: &Context,
        rect: Rect,
        index: usize,
        doc: &(String, String),
    ) -> bool {
        let Some(main) = MainThreadMarker::new() else {
            return false;
        };
        let Some(parent) = NSApplication::sharedApplication(main)
            .windows()
            .iter()
            .find(|w| w.title().to_string() == "오픈소스 라이선스")
            .and_then(|w| w.contentView())
        else {
            return false;
        };
        if slot
            .as_ref()
            .is_none_or(|r| !r.scroll.isDescendantOf(&parent))
        {
            *slot = Self::attach(ctx, &parent, main);
        }
        let Some(reader) = slot.as_mut() else {
            return false;
        };
        let view = parent.bounds();
        let zoom = f64::from(ctx.zoom_factor());
        let y = if parent.isFlipped() {
            f64::from(rect.top()) * zoom
        } else {
            view.size.height - f64::from(rect.bottom()) * zoom
        };
        let frame = NSRect::new(
            NSPoint::new(
                view.origin.x + f64::from(rect.left()) * zoom,
                view.origin.y + y,
            ),
            NSSize::new(
                f64::from(rect.width()) * zoom,
                f64::from(rect.height()) * zoom,
            ),
        );
        if reader.scroll.frame() != frame {
            reader.scroll.setFrame(frame);
        }
        if reader.selected != Some(index) {
            reader.text.setString(&NSString::from_str(&doc.1));
            reader
                .scroll
                .setAccessibilityLabel(Some(&NSString::from_str(&doc.0)));
            reader.text.setSelectedRange(NSRange::new(0, 0));
            reader.text.scrollRangeToVisible(NSRange::new(0, 0));
            reader.selected = Some(index);
        }
        true
    }

    pub fn cancelled(&self) -> bool {
        self.cancelled.get()
    }
}

impl Drop for DocumentReader {
    fn drop(&mut self) {
        if let Some(monitor) = self.keyboard.take() {
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
        self.scroll.removeFromSuperview();
    }
}
