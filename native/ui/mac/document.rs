use block2::RcBlock;
use eframe::egui::{Context, Rect};
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{
    NSAccessibility, NSApplication, NSBorderType, NSColor, NSEvent, NSEventMask, NSFont,
    NSFontWeightRegular, NSLineBreakStrategy, NSScrollView, NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::{cell::Cell, ptr::NonNull, rc::Rc};

pub struct DocumentReader {
    scroll: Retained<NSScrollView>,
    document: Retained<NSView>,
    text: Retained<NSTextField>,
    selected: Option<usize>,
    measured: Option<(NSSize, f64)>,
    cancelled: Rc<Cell<bool>>,
    keyboard: Option<Retained<AnyObject>>,
}

impl DocumentReader {
    fn attach(ctx: &Context, parent: &NSView, main: MainThreadMarker) -> Option<Self> {
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(main), NSRect::ZERO);
        let document = NSView::initWithFrame(NSView::alloc(main), NSRect::ZERO);
        let text = NSTextField::wrappingLabelWithString(&NSString::new(), main);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setHasHorizontalScroller(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        text.setSelectable(true);
        text.setLineBreakStrategy(NSLineBreakStrategy::Standard);
        document.addSubview(&text);
        scroll.setDocumentView(Some(&document));
        parent.addSubview(&scroll);

        // The selectable field uses the window's field editor while copying.
        // Escape dismisses the sheet even when that editor has the keyboard.
        let cancelled = Rc::new(Cell::new(false));
        let flag = Rc::clone(&cancelled);
        let listener = text.clone();
        let context = ctx.clone();
        let monitor = RcBlock::new(move |event: NonNull<NSEvent>| {
            let key = unsafe { event.as_ref() };
            if key.keyCode() == 53
                && listener.window().is_some_and(|w| w.windowNumber() == key.windowNumber())
                && listener.currentEditor().is_some()
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
            document,
            text,
            selected: None,
            measured: None,
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
        let clip = reader.scroll.contentView();
        let visible = clip.bounds();
        let top = reader.document.bounds().size.height - visible.origin.y - visible.size.height;
        if reader.scroll.frame() != frame {
            reader.scroll.setFrame(frame);
        }
        let changed = reader.selected != Some(index);
        if changed {
            reader.text.abortEditing();
            reader.text.setStringValue(&NSString::from_str(&doc.1));
            reader
                .scroll
                .setAccessibilityLabel(Some(&NSString::from_str(&doc.0)));
            reader.selected = Some(index);
        }
        super::selectable_text::set_label_ink(&reader.text, &NSColor::labelColor());
        let viewport = clip.bounds().size;
        if changed || reader.measured != Some((viewport, zoom)) {
            let font = unsafe {
                NSFont::monospacedSystemFontOfSize_weight(12. * zoom, NSFontWeightRegular)
            };
            reader.text.setFont(Some(&font));
            let padding = 16. * zoom;
            // The wrapping cell draws two points inside each horizontal edge.
            let maximum = (viewport.width - 2. * padding + 4.).max(4.);
            let fit = reader.text.sizeThatFits(NSSize::new(maximum, f64::MAX));
            let width = fit.width.ceil().min(maximum);
            let text_height = fit.height.ceil();
            let height = (text_height + 2. * padding).max(viewport.height);
            reader.document.setFrameSize(NSSize::new(viewport.width, height));
            reader.text.setFrame(NSRect::new(
                NSPoint::new(padding - 2., height - padding - text_height),
                NSSize::new(width, text_height),
            ));
            let top = if changed { 0. } else { top.max(0.) };
            reader.document.scrollPoint(NSPoint::new(0., (height - viewport.height - top).max(0.)));
            reader.measured = Some((viewport, zoom));
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
