use eframe::egui::Rect;
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{NSAccessibility, NSColor, NSFont, NSTextField, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub struct BannerLabel(Retained<NSTextField>);

impl BannerLabel {
    pub fn attach(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let main = MainThreadMarker::new()?;
        let RawWindowHandle::AppKit(handle) = cc.window_handle().ok()?.as_raw() else {
            return None;
        };
        let parent = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let label = NSTextField::wrappingLabelWithString(&NSString::new(), main);
        label.setSelectable(true);
        label.setTextColor(Some(&NSColor::systemOrangeColor()));
        label.setHidden(true);
        parent.addSubview(&label);
        Some(Self(label))
    }

    pub fn place(&self, message: &str, rect: Rect, available: f32, zoom: f32) {
        let label = &self.0;
        let Some(parent) = (unsafe { label.superview() }) else {
            return;
        };
        let zoom = f64::from(zoom);
        let size = 12. * zoom;
        if label.font().is_none_or(|font| font.pointSize() != size) {
            label.setFont(Some(&NSFont::systemFontOfSize(size)));
        }
        let value = NSString::from_str(message);
        // Do not replace the string while the user is dragging a selection.
        if label.stringValue() != value {
            label.setStringValue(&value);
            label.setAccessibilityLabel(Some(&value));
        }
        // Each edge of the borderless NSTextFieldCell has a two-point inset.
        let maximum = f64::from(available) * zoom + 4.;
        let width = label
            .sizeThatFits(NSSize::new(maximum, f64::MAX))
            .width
            .ceil()
            .min(maximum);
        let bounds = parent.bounds();
        let y = if parent.isFlipped() {
            f64::from(rect.top()) * zoom
        } else {
            bounds.size.height - f64::from(rect.bottom()) * zoom
        };
        let frame = NSRect::new(
            NSPoint::new(
                bounds.origin.x + f64::from(rect.left()) * zoom - 2.,
                bounds.origin.y + y,
            ),
            NSSize::new(width, f64::from(rect.height()) * zoom),
        );
        if label.frame() != frame {
            label.setFrame(frame);
        }
        if label.isHidden() {
            label.setHidden(false);
        }
    }

    pub fn hide(&self) {
        if !self.0.isHidden() {
            self.0.setHidden(true);
            self.0.setStringValue(&NSString::new());
            self.0.setAccessibilityLabel(None);
        }
    }
}

impl Drop for BannerLabel {
    fn drop(&mut self) {
        self.0.removeFromSuperview();
    }
}
