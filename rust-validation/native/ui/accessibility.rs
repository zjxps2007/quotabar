//! Eframe installs a platform accessibility adapter on its root window.
//! Immediate dialog viewports need their own adapter and input queue.
use eframe::egui::{self, accesskit};
use std::{
    cell::RefCell,
    sync::{Arc, Mutex, mpsc},
};

#[cfg(target_os = "macos")]
use accesskit_macos::SubclassingAdapter;
#[cfg(windows)]
use accesskit_windows::SubclassingAdapter;

pub fn viewport() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("dashboard-dialog")
}

/// Native clients may replace the content of a single-line field through
/// UI Automation's ValuePattern or NSAccessibility's value setter.
pub fn text_value(ui: &egui::Ui, field: egui::Response, text: &mut String, label: &str) {
    if field.enabled() {
        let mut replacements = Vec::new();
        ui.input_mut(|input| {
            input.consume_accesskit_action_requests(field.id, |request| {
                if request.action == accesskit::Action::SetValue
                    && let Some(accesskit::ActionData::Value(value)) = &request.data
                {
                    replacements.push(value.clone());
                    true
                } else {
                    false
                }
            })
        });
        for value in replacements {
            #[cfg(windows)]
            set_windows_text(ui, field.id, text, &value);
            #[cfg(target_os = "macos")]
            value
                .split(['\r', '\n'])
                .next()
                .unwrap_or_default()
                .clone_into(text);
        }
    }
    ui.ctx().accesskit_node_builder(field.id, |node| {
        node.set_label(label);
        node.set_value(text.as_str());
        if field.enabled() {
            node.add_action(accesskit::Action::SetValue);
        }
    });
}

#[cfg(windows)]
fn set_windows_text(ui: &egui::Ui, id: egui::Id, text: &mut String, value: &str) {
    use egui::{TextEdit, text::CCursorRange};
    if text == value {
        return;
    }
    // WPF's ValuePattern assigns Text, including line breaks and existing names
    // above MaxLength. It resets the caret and makes the replacement undoable.
    let mut state = TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    let mut history = state.undoer();
    let before = state.cursor.char_range().unwrap_or_default();
    let after = CCursorRange::default();
    history.add_undo(&(before, std::mem::replace(text, value.to_owned())));
    let replacement = (after, text.clone());
    history.feed_state(ui.input(|i| i.time), &replacement);
    history.add_undo(&replacement);
    state.set_undoer(history);
    state.cursor.set_char_range(Some(after));
    state.store(ui.ctx(), id);
}

struct Handlers {
    actions: mpsc::Sender<accesskit::ActionRequest>,
    ctx: egui::Context,
}
impl accesskit::ActivationHandler for Handlers {
    fn request_initial_tree(&mut self) -> Option<accesskit::TreeUpdate> {
        self.ctx.enable_accesskit();
        self.ctx.request_repaint_of(egui::ViewportId::ROOT);
        None
    }
}
impl accesskit::ActionHandler for Handlers {
    fn do_action(&mut self, request: accesskit::ActionRequest) {
        let _ = self.actions.send(request);
        self.ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}
struct DialogTree {
    actions: Mutex<mpsc::Receiver<accesskit::ActionRequest>>,
    output: Arc<Mutex<Option<accesskit::TreeUpdate>>>,
    in_dialog: bool,
}
impl egui::Plugin for DialogTree {
    fn debug_name(&self) -> &'static str {
        "QuotaBar dialog accessibility"
    }
    fn input_hook(&mut self, input: &mut egui::RawInput) {
        if input.viewport_id == viewport() {
            input.events.extend(
                self.actions
                    .get_mut()
                    .unwrap()
                    .try_iter()
                    .map(egui::Event::AccessKitActionRequest),
            );
        }
    }
    fn on_end_pass(&mut self, ctx: &egui::Context) {
        self.in_dialog = ctx.viewport_id() == viewport();
    }
    fn output_hook(&mut self, output: &mut egui::FullOutput) {
        if self.in_dialog {
            *self.output.lock().unwrap() = output.platform_output.accesskit_update.take();
        }
    }
}
pub struct DialogAccess {
    native: RefCell<Option<SubclassingAdapter>>,
    actions: mpsc::Sender<accesskit::ActionRequest>,
    output: Arc<Mutex<Option<accesskit::TreeUpdate>>>,
}
impl DialogAccess {
    pub fn new(ctx: &egui::Context) -> Self {
        let (actions, input) = mpsc::channel();
        let output = Arc::new(Mutex::new(None));
        ctx.add_plugin(DialogTree {
            actions: Mutex::new(input),
            output: output.clone(),
            in_dialog: false,
        });
        Self {
            native: RefCell::new(None),
            actions,
            output,
        }
    }
    pub fn install(&self, ctx: &egui::Context, _frame: &eframe::Frame, title: &str) {
        if self.native.borrow().is_some() {
            return;
        }
        let handlers = || Handlers {
            ctx: ctx.clone(),
            actions: self.actions.clone(),
        };
        #[cfg(windows)]
        let adapter = {
            use windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible;
            let Some(handle) = crate::platform::dialog_hwnd(_frame, title) else {
                return;
            };
            if unsafe { IsWindowVisible(handle) } != 0 {
                return;
            }
            SubclassingAdapter::new(
                windows::Win32::Foundation::HWND(handle),
                handlers(),
                handlers(),
            )
        };
        #[cfg(target_os = "macos")]
        let adapter = {
            use objc2::{MainThreadMarker, rc::Retained};
            use objc2_app_kit::NSApplication;
            let Some(m) = MainThreadMarker::new() else {
                return;
            };
            let Some(view) = NSApplication::sharedApplication(m)
                .windows()
                .iter()
                .find(|w| w.title().to_string() == title)
                .and_then(|w| w.contentView())
            else {
                return;
            };
            unsafe {
                SubclassingAdapter::new(
                    Retained::as_ptr(&view).cast_mut().cast(),
                    handlers(),
                    handlers(),
                )
            }
        };
        *self.native.borrow_mut() = Some(adapter);
    }
    pub fn publish(&self) {
        let tree = self.output.lock().unwrap().take();
        let events =
            tree.and_then(|tree| self.native.borrow_mut().as_mut()?.update_if_active(|| tree));
        if let Some(events) = events {
            events.raise();
        }
    }
    pub fn close(&self) {
        self.native.borrow_mut().take();
        self.output.lock().unwrap().take();
    }
}
