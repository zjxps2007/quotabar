use super::selectable_text::place_clipped_view;
use crate::{accessibility, style};
use eframe::egui::{Context, Id, Key, Sense, Ui, ViewportId, WidgetInfo, vec2};
use objc2::{
    DefinedClass, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{ProtocolObject, Sel},
    sel,
};
use objc2_app_kit::{
    NSAccessibility, NSColor, NSControl, NSControlTextEditingDelegate, NSFont, NSTextField,
    NSTextFieldBezelStyle, NSTextFieldDelegate, NSTextInputClient, NSTextView, NSView,
};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSRect, NSString};
use std::cell::Cell;

struct Editing {
    context: Context,
    command: Cell<Option<Key>>,
    sheet: bool,
}

define_class!(
    #[unsafe(super=NSObject)]
    #[thread_kind=MainThreadOnly]
    #[ivars=Editing]
    #[name = "QuotaBarConnectionFieldDelegate"]
    struct Delegate;
    unsafe impl NSObjectProtocol for Delegate {}
    unsafe impl NSTextFieldDelegate for Delegate {}
    unsafe impl NSControlTextEditingDelegate for Delegate {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, _: &NSNotification) {
            self.ivars().context.request_repaint_of(ViewportId::ROOT);
        }

        #[unsafe(method(controlTextDidBeginEditing:))]
        fn began_editing(&self, _: &NSNotification) {
            self.ivars().context.request_repaint_of(ViewportId::ROOT);
        }

        #[unsafe(method(controlTextDidEndEditing:))]
        fn ended_editing(&self, _: &NSNotification) {
            self.ivars().context.request_repaint_of(ViewportId::ROOT);
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        unsafe fn editing_command(&self, _: &NSControl, editor: &NSTextView, command: Sel) -> bool {
            // Let the input method commit or cancel its own marked text first.
            let key = if command == sel!(insertNewline:) {
                Some(Key::Enter)
            } else if command == sel!(cancelOperation:) {
                Some(Key::Escape)
            } else {
                None
            };
            if let Some(key) = key.filter(|_| self.ivars().sheet && !editor.hasMarkedText()) {
                self.ivars().command.set(Some(key));
                self.ivars().context.request_repaint_of(ViewportId::ROOT);
                true
            } else {
                false
            }
        }
    }
);

impl Delegate {
    fn new(parent: &NSView, context: &Context, sheet: bool) -> Retained<Self> {
        unsafe {
            msg_send![
                super(Self::alloc(parent.mtm()).set_ivars(Editing {
                    context: context.clone(),
                    command: Cell::new(None),
                    sheet,
                })),
                init
            ]
        }
    }
}

#[derive(Clone, Copy)]
struct FieldSpec {
    id: &'static str,
    label: &'static str,
    placeholder: &'static str,
}
const SEARCH: FieldSpec = FieldSpec {
    id: "accounts-query",
    label: "계정 이름 또는 이메일 검색",
    placeholder: "계정 이름 또는 이메일 검색",
};

pub enum Input {
    Alias,
    Path,
}

impl Input {
    fn spec(&self) -> FieldSpec {
        match self {
            Self::Alias => FieldSpec {
                id: "connect-alias",
                label: "계정 이름",
                placeholder: "예: 개인 계정, 연구실",
            },
            Self::Path => FieldSpec {
                id: "connect-path",
                label: "경로",
                placeholder: "경로",
            },
        }
    }
}

struct Editor {
    field: Retained<NSTextField>,
    clip: Retained<NSView>,
    last_value: Option<String>,
    was_editing: bool,
}

impl Editor {
    fn new(parent: &NSView, delegate: &Delegate, spec: FieldSpec) -> Self {
        let field = NSTextField::textFieldWithString(&NSString::new(), parent.mtm());
        field.setBezeled(true);
        field.setBezelStyle(NSTextFieldBezelStyle::RoundedBezel);
        field.setUsesSingleLineMode(true);
        field.setPlaceholderString(Some(&NSString::from_str(spec.placeholder)));
        field.setAccessibilityLabel(Some(&NSString::from_str(spec.label)));
        // AppKit's delegate is weak. The owner retains it until the field is removed.
        unsafe { field.setDelegate(Some(ProtocolObject::from_ref(delegate))) };
        let clip = NSView::initWithFrame(NSView::alloc(parent.mtm()), NSRect::ZERO);
        clip.setHidden(true);
        clip.setClipsToBounds(true);
        clip.addSubview(&field);
        parent.addSubview(&clip);
        Self {
            field,
            clip,
            last_value: None,
            was_editing: false,
        }
    }

    fn sync(&mut self, value: &mut String) {
        let editor = self.field.currentEditor();
        if self.last_value.as_ref() != Some(value) {
            // Provider changes, the file picker and accessibility may replace
            // the bound value while AppKit's shared field editor is active.
            let replacement = NSString::from_str(value);
            self.field.setStringValue(&replacement);
            if let Some(editor) = editor {
                editor.setString(&replacement);
            }
        } else {
            let current = editor.map_or_else(|| self.field.stringValue(), |e| e.string());
            if current != NSString::from_str(value) {
                *value = current.to_string();
            }
        }
        self.last_value.get_or_insert_default().clone_from(value);
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        unsafe { self.field.setDelegate(None) };
        self.clip.removeFromSuperview();
    }
}

pub struct ConnectionFields {
    parent: Retained<NSView>,
    alias: Editor,
    path: Editor,
    delegate: Retained<Delegate>,
}

impl ConnectionFields {
    pub fn new(parent: Retained<NSView>, context: &Context) -> Self {
        let delegate = Delegate::new(&parent, context, true);
        Self {
            alias: Editor::new(&parent, &delegate, Input::Alias.spec()),
            path: Editor::new(&parent, &delegate, Input::Path.spec()),
            parent,
            delegate,
        }
    }

    pub fn command(&self) -> Option<Key> {
        self.delegate.ivars().command.take()
    }

    pub fn alias_was_editing(&self) -> bool {
        self.alias.was_editing
    }

    pub fn hide_path(&self) {
        self.path.clip.setHidden(true);
    }
}

pub struct AccountSearch {
    parent: Retained<NSView>,
    editor: Editor,
    _delegate: Retained<Delegate>,
}

impl AccountSearch {
    pub fn new(parent: Retained<NSView>, context: &Context) -> Self {
        let delegate = Delegate::new(&parent, context, false);
        Self {
            editor: Editor::new(&parent, &delegate, SEARCH),
            parent,
            _delegate: delegate,
        }
    }

    pub fn show(
        field: Option<&mut Self>,
        ui: &mut Ui,
        value: &mut String,
        width: f32,
        focus: bool,
        palette: style::Palette,
    ) {
        let native = field.map(|field| (&*field.parent, &mut field.editor));
        text_input(native, ui, SEARCH, value, width, focus, palette);
    }
}

pub fn connection_input(
    fields: Option<&mut ConnectionFields>,
    ui: &mut Ui,
    kind: Input,
    value: &mut String,
    width: f32,
    focus: bool,
    palette: style::Palette,
) {
    let spec = kind.spec();
    let native = fields.map(|fields| {
        let editor = match kind {
            Input::Alias => &mut fields.alias,
            Input::Path => &mut fields.path,
        };
        (&*fields.parent, editor)
    });
    text_input(native, ui, spec, value, width, focus, palette);
}

fn text_input(
    native: Option<(&NSView, &mut Editor)>,
    ui: &mut Ui,
    spec: FieldSpec,
    value: &mut String,
    width: f32,
    focus: bool,
    palette: style::Palette,
) {
    let Some((parent, editor)) = native else {
        // An offscreen framebuffer cannot composite an AppKit subview.
        let field = style::mac_field(ui, value, spec.placeholder, spec.id, width, palette);
        if focus {
            field.request_focus();
        }
        accessibility::text_value(ui, field, value, spec.label);
        return;
    };
    let (rect, _) = ui.allocate_exact_size(vec2(width, 22.), Sense::hover());
    let response = ui.interact(rect, Id::new(spec.id), Sense::focusable_noninteractive());
    let zoom = f64::from(ui.ctx().zoom_factor());
    // The native focus ring extends outside the field. Clip only where the
    // containing UI actually clips the control.
    let visible = rect.expand(5.).intersect(ui.clip_rect());
    place_clipped_view(parent, &editor.clip, &editor.field, rect, visible, zoom);
    let font = NSFont::systemFontOfSize(13. * zoom);
    if editor.field.font().as_ref() != Some(&font) {
        editor.field.setFont(Some(&font));
    }
    if editor.field.isEnabled() != ui.is_enabled() {
        if !ui.is_enabled()
            && editor.field.currentEditor().is_some()
            && let Some(window) = parent.window()
        {
            window.makeFirstResponder(None);
        }
        editor.field.setEnabled(ui.is_enabled());
    }
    editor.sync(value);
    response
        .widget_info(|| WidgetInfo::text_edit(ui.is_enabled(), &*value, &*value, spec.placeholder));
    let gain = response.gained_focus();
    let editing = editor.field.currentEditor().is_some();
    if editing && !editor.was_editing {
        response.request_focus();
    }
    editor.was_editing = editing;
    accessibility::text_value(ui, response, value, spec.label);
    if editor.last_value.as_ref() != Some(value) {
        editor.sync(value);
    }
    if ui.is_enabled()
        && visible.is_positive()
        && (focus || gain)
        && editor.field.currentEditor().is_none()
        && let Some(window) = parent.window()
    {
        window.makeFirstResponder(Some(&editor.field));
    }
    if let Some(text_view) = editor
        .field
        .currentEditor()
        .and_then(|text| text.downcast::<NSTextView>().ok())
    {
        // SwiftUI applies the sheet's monochrome accent to the insertion point,
        // independently of the system focus ring.
        let caret = if palette.dark {
            NSColor::whiteColor()
        } else {
            NSColor::blackColor()
        };
        if text_view.insertionPointColor() != caret {
            text_view.setInsertionPointColor(Some(&caret));
        }
    }
}
