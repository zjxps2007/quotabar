use eframe::egui::{Id, Sense, Ui, vec2};
use objc2::{MainThreadOnly, rc::Retained};
use objc2_app_kit::{NSAccessibility, NSColor, NSFont, NSLineBreakMode, NSTextField, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::collections::HashMap;

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub enum AccountLabel {
    Name,
    Email,
}

struct Field {
    clip: Retained<NSView>,
    label: Retained<NSTextField>,
    used: bool,
}

impl Drop for Field {
    fn drop(&mut self) {
        self.clip.removeFromSuperview();
    }
}

pub struct AccountLabels {
    parent: Retained<NSView>,
    fields: HashMap<Id, Field>,
}

impl AccountLabels {
    pub fn attach(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let RawWindowHandle::AppKit(handle) = cc.window_handle().ok()?.as_raw() else {
            return None;
        };
        // The creation context supplies a live, main-thread AppKit view.
        let parent = unsafe { Retained::retain(handle.ns_view.cast::<NSView>().as_ptr()) }?;
        Some(Self {
            parent,
            fields: HashMap::new(),
        })
    }

    pub fn show(&mut self, ui: &mut Ui, kind: AccountLabel, text: &str) {
        let id = ui.make_persistent_id(kind);
        let main = self.parent.mtm();
        let field = self.fields.entry(id).or_insert_with(|| {
            let label = match kind {
                AccountLabel::Name => {
                    let label = NSTextField::labelWithString(&NSString::new(), main);
                    label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                    label.setMaximumNumberOfLines(1);
                    label
                }
                AccountLabel::Email => {
                    let label = NSTextField::wrappingLabelWithString(&NSString::new(), main);
                    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
                    label
                }
            };
            label.setSelectable(true);
            let clip = NSView::initWithFrame(NSView::alloc(main), NSRect::ZERO);
            clip.setClipsToBounds(true);
            clip.addSubview(&label);
            self.parent.addSubview(&clip);
            Field {
                label,
                clip,
                used: true,
            }
        });
        field.used = true;
        let zoom = f64::from(ui.ctx().zoom_factor());
        let points = zoom
            * match kind {
                AccountLabel::Name => 12.,
                AccountLabel::Email => 10.,
            };
        if field.label.font().is_none_or(|f| f.pointSize() != points) {
            field.label.setFont(Some(&NSFont::systemFontOfSize(points)));
        }
        let value = NSString::from_str(text);
        if field.label.stringValue() != value {
            field.label.setStringValue(&value);
            field.label.setAccessibilityLabel(Some(&value));
            if kind == AccountLabel::Name {
                field.label.setToolTip(Some(&value));
                field.label.setAccessibilityHelp(Some(&value));
            }
        }

        // NSTextFieldCell has a two-point inset at each end. Align its text
        // with the stack and keep the native selection alive across repaints.
        let maximum = f64::from(ui.available_width().max(1.)) * zoom + 4.;
        let size = field.label.sizeThatFits(NSSize::new(maximum, f64::MAX));
        let width = (size.width.ceil().min(maximum) - 4.).max(0.);
        let (text_rect, _) = ui.allocate_exact_size(
            vec2((width / zoom) as f32, (size.height.ceil() / zoom) as f32),
            Sense::hover(),
        );
        let full = text_rect.expand2(vec2((2. / zoom) as f32, 0.));
        let visible = full.intersect(ui.clip_rect());
        field.clip.setHidden(!visible.is_positive());
        if !visible.is_positive() {
            return;
        }
        let bounds = self.parent.bounds();
        let y = if self.parent.isFlipped() {
            f64::from(visible.top()) * zoom
        } else {
            bounds.size.height - f64::from(visible.bottom()) * zoom
        };
        let clip_frame = NSRect::new(
            NSPoint::new(
                bounds.origin.x + f64::from(visible.left()) * zoom,
                bounds.origin.y + y,
            ),
            NSSize::new(
                f64::from(visible.width()) * zoom,
                f64::from(visible.height()) * zoom,
            ),
        );
        if field.clip.frame() != clip_frame {
            field.clip.setFrame(clip_frame);
        }
        // The clip view uses standard AppKit coordinates, even for a flipped host.
        let text_frame = NSRect::new(
            NSPoint::new(
                f64::from(full.left() - visible.left()) * zoom,
                f64::from(visible.bottom() - full.bottom()) * zoom,
            ),
            NSSize::new(
                f64::from(full.width()) * zoom,
                f64::from(full.height()) * zoom,
            ),
        );
        if field.label.frame() != text_frame {
            field.label.setFrame(text_frame);
        }
    }

    pub fn finish_frame(&mut self) {
        // Filtering, navigation and GPU captures can all remove a field.
        self.fields
            .retain(|_, field| std::mem::take(&mut field.used));
    }
}
