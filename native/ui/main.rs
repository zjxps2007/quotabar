#![cfg_attr(windows, windows_subsystem = "windows")]

mod accessibility;
mod account_text;
mod activity;
mod dashboard;
mod dialogs;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
mod mac_fonts;
#[cfg(target_os = "macos")]
mod mac_text;
mod management;
mod model;
mod platform;
mod style;
#[cfg(windows)]
mod windows_border;
#[cfg(windows)]
mod windows_choice;
#[cfg(windows)]
mod windows_scroll;
#[cfg(windows)]
mod windows_text;
#[cfg(windows)]
mod windows_viewport_capture;
mod wire;

use eframe::egui;
use model::State;
use serde_json::{Value, json};
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct Capture {
    path: String,
    #[cfg(target_os = "macos")]
    requested_height: f32,
    #[cfg(target_os = "macos")]
    backing_size: Option<[usize; 2]>,
    after: Instant,
    requested: bool,
    painted: std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<egui::ColorImage>>>>,
}
impl Capture {
    fn dialog_image(
        &mut self,
        ctx: &egui::Context,
        _frame: &eframe::Frame,
    ) -> Option<std::sync::Arc<egui::ColorImage>> {
        if let Some(image) = self.painted.lock().unwrap().take() {
            return Some(image);
        }
        #[cfg(windows)]
        if _frame.gl().is_none() {
            if !self.requested && Instant::now() >= self.after {
                self.requested = true;
                windows_viewport_capture::ViewportFrame::request(ctx);
            }
            ctx.request_repaint_after_for(Duration::from_millis(25), egui::ViewportId::ROOT);
            return None;
        }
        if !self.requested && Instant::now() >= self.after {
            self.requested = true;
            let result = self.painted.clone();
            let wake = ctx.clone();
            // Immediate child viewports are drawn directly and do not deliver
            // ViewportCommand::Screenshot. Read their actual back buffer after text and controls.
            let callback = eframe::egui_glow::CallbackFn::new(move |info, painter| {
                *result.lock().unwrap() = Some(std::sync::Arc::new(
                    painter.read_screen_rgba(info.screen_size_px),
                ));
                wake.request_repaint_of(egui::ViewportId::ROOT);
            });
            ctx.layer_painter(egui::LayerId::new(
                egui::Order::Debug,
                egui::Id::new("dialog-readback"),
            ))
            .add(egui::PaintCallback {
                rect: ctx.content_rect(),
                callback: std::sync::Arc::new(callback),
            });
        }
        ctx.request_repaint_after_for(Duration::from_millis(25), egui::ViewportId::ROOT);
        None
    }
    fn image(
        &mut self,
        ctx: &egui::Context,
        _frame: &eframe::Frame,
    ) -> Option<std::sync::Arc<egui::ColorImage>> {
        let pixels = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        #[cfg(target_os = "macos")]
        let pixels = {
            let Some((expected, screen_limited)) =
                platform::capture_backing_size(_frame, self.requested_height)
            else {
                ctx.request_repaint_after(Duration::from_millis(25));
                return None;
            };
            if self.backing_size.replace(expected) != Some(expected) {
                // A moving Dock changes the work area after the first resize.
                // Use the reference capture's settling interval while constrained.
                let settle = if screen_limited {
                    Duration::from_secs(2)
                } else {
                    Duration::from_millis(200)
                };
                self.after = Instant::now() + settle;
                self.requested = false;
            }
            pixels.filter(|image| {
                if image.size != expected {
                    // WindowServer resized the view while its OpenGL frame was in flight.
                    self.requested = false;
                }
                self.requested
            })
        };
        if pixels.is_none() {
            ctx.request_repaint_after(Duration::from_millis(25));
            #[cfg(windows)]
            if let Some(state) = _frame.wgpu_render_state() {
                if self.requested {
                    return match windows_viewport_capture::ViewportFrame::read(ctx, state) {
                        Ok(image) => image,
                        Err(error) => {
                            record_probe_error(&error.to_string());
                            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                                Default::default(),
                            ));
                            None
                        }
                    };
                }
                if Instant::now() >= self.after {
                    self.requested = true;
                    windows_viewport_capture::ViewportFrame::request(ctx);
                }
                return None;
            }
            if !self.requested && Instant::now() >= self.after {
                self.requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
        }
        pixels
    }
}

struct Dashboard {
    dialog_access: accessibility::DialogAccess,
    wire: wire::Peer,
    inbox: mpsc::Receiver<wire::Event>,
    state: State,
    route: String,
    scroll_top: bool,
    activity_scroll_frame: Option<u64>,
    #[cfg(target_os = "macos")]
    activity_selection: activity::DaySelection,
    filter: String,
    query: String,
    #[cfg(target_os = "macos")]
    focus_query: bool,
    #[cfg(target_os = "macos")]
    account_search: Option<mac::AccountSearch>,
    attention: bool,
    dialog: Option<dialogs::Dialog>,
    assets: style::Assets,
    capture: Option<Capture>,
    capture_dialog: bool,
    visible: Option<bool>,
    focused: bool,
    taskbar: Option<bool>,
    #[cfg(target_os = "macos")]
    app_menu: mac::AppMenu,
    #[cfg(target_os = "macos")]
    scroller: Option<std::rc::Rc<mac::NativeScroller>>,
    #[cfg(target_os = "macos")]
    banner_label: Option<mac::BannerLabel>,
    #[cfg(target_os = "macos")]
    selectable_text: Option<mac::SelectableText>,
    #[cfg(target_os = "macos")]
    local: mac::Local,
}

impl Dashboard {
    fn navigate(&mut self, ctx: &egui::Context, page: &str, notify: bool) {
        #[cfg(windows)]
        style::reset_disclosures(ctx);
        #[cfg(target_os = "macos")]
        if notify && page == self.route {
            // Showing an existing SwiftUI page keeps its scroll position. A
            // minimized or hidden window has detached its hosting view.
            if self.visible == Some(false) {
                self.scroll_top = true;
            }
            return;
        }
        #[cfg(target_os = "macos")]
        if page == "accounts" && self.route != page {
            self.focus_query = true;
        } else if self.route == "accounts" && page != "accounts" {
            // The reference owns these filters in the AccountsView's local state.
            self.account_search = None;
            self.query.clear();
            self.filter = "전체 Provider".into();
            self.attention = false;
        }
        #[cfg(target_os = "macos")]
        {
            self.activity_selection = activity::DaySelection::default();
        }
        // WPF dispatches the activity anchor at Loaded priority, after the
        // window and its ScrollViewer have measured their final client area.
        self.activity_scroll_frame =
            (page == "activity").then(|| ctx.cumulative_frame_nr() + u64::from(cfg!(windows)));
        self.route = match page {
            "accounts" | "settings" => page,
            _ => "dashboard",
        }
        .into();
        self.scroll_top = true;
        if notify {
            self.wire.action("route", json!(self.route));
            if self.route == "settings" {
                self.wire.action("startup-status", Value::Null);
            }
        }
    }

    fn refresh(&mut self) {
        self.wire.action("refresh", Value::Null);
        #[cfg(target_os = "macos")]
        self.local.invalidate();
    }

    fn visibility(&mut self, value: bool) {
        if self.visible != Some(value) {
            self.visible = Some(value);
            self.wire.action("visibility", json!(value));
        }
    }

    fn incoming(&mut self, ctx: &egui::Context, frame: &eframe::Frame, event: wire::Event) {
        match event {
            wire::Event::State(value) => {
                #[cfg(windows)]
                style::reset_disclosures(ctx);
                #[cfg(target_os = "macos")]
                {
                    let theme = value["settings"]["appearance"].as_str().unwrap_or("system");
                    if theme != self.state.setting("appearance", "system") {
                        quotabar::mac_menu::apply_appearance(theme);
                    }
                }
                self.state.apply(value);
            }
            wire::Event::Show(page) => {
                if cfg!(windows) && self.visible.is_none() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(client_size(
                        frame,
                        ctx,
                        [1120., 810.],
                    )));
                }
                self.navigate(ctx, &page, true);
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                #[cfg(windows)]
                platform::activate_dashboard(frame);
                #[cfg(target_os = "macos")]
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                self.visibility(true);
            }
            wire::Event::Close => {
                self.visibility(false);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            wire::Event::Capture {
                route,
                width,
                height,
                path,
            } => {
                self.capture_dialog = route.starts_with("connect");
                self.navigate(
                    ctx,
                    if self.capture_dialog {
                        "accounts"
                    } else {
                        &route
                    },
                    false,
                );
                if self.capture_dialog {
                    self.open_connect(
                        match route.as_str() {
                            "connect-claude" => "Claude",
                            "connect-gemini" => "Gemini",
                            _ => "Codex",
                        },
                        None,
                    );
                } else {
                    self.dialog = None;
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(client_size(
                    frame,
                    ctx,
                    [width, height],
                )));
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                self.capture = Some(Capture {
                    path,
                    #[cfg(target_os = "macos")]
                    requested_height: height,
                    #[cfg(target_os = "macos")]
                    backing_size: None,
                    after: Instant::now() + Duration::from_millis(200),
                    requested: false,
                    painted: Default::default(),
                });
                self.visibility(true);
            }
            wire::Event::Reply(id, error) => self.finish_request(&id, error),
        }
    }
}

impl eframe::App for Dashboard {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Some(event) = self.wire.initial.take() {
            self.incoming(ctx, frame, event);
        }
        while let Ok(event) = self.inbox.try_recv() {
            self.incoming(ctx, frame, event);
        }
        #[cfg(windows)]
        if let Some(renderer) = ctx.plugin_opt::<windows_text::TextRenderer>() {
            renderer.lock().bitmap_target = self.capture.is_some();
        }
        #[cfg(windows)]
        if let Some(renderer) = ctx.plugin_opt::<windows_border::BorderRenderer>() {
            renderer.lock().window_target = self.capture.is_none();
        }
        let (minimized, focused, closing) = ctx.input(|i| {
            (
                i.viewport().minimized,
                i.viewport().focused.unwrap_or(false),
                i.viewport().close_requested(),
            )
        });
        if closing {
            self.visibility(false);
        } else if self.visible.is_some()
            && let Some(min) = minimized
        {
            self.visibility(!min);
        }
        if focused && !self.focused && self.route == "settings" {
            self.wire.action("startup-status", Value::Null);
        }
        self.focused = focused;
        let taskbar = self.state.flag("showTaskbarIcon", "showDockIcon");
        if self.taskbar != Some(taskbar) {
            self.taskbar = Some(taskbar);
            set_taskbar(frame, taskbar);
        }
        #[cfg(target_os = "macos")]
        self.mac_tick(ctx);
        let colors = style::Palette::new(ctx, self.state.setting("appearance", "system"));
        colors.apply(ctx);
        if self.route == "dashboard"
            && self.dialog.is_none()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::R))
        {
            self.refresh();
        }
        #[cfg(target_os = "macos")]
        let titlebar = platform::dashboard_titlebar_height(frame);
        #[cfg(not(target_os = "macos"))]
        let titlebar = 0;
        self.workspace(ctx, colors, titlebar);
        #[cfg(target_os = "macos")]
        if let Some(labels) = &mut self.selectable_text {
            labels.finish_frame();
        }
        self.show_dialog(ctx, frame, colors);
        if let Some(frame_due) = self.activity_scroll_frame {
            // ScrollArea commits a programmatic scroll after laying out its body.
            // The next pass paints the content at the same offset as the thumb.
            if ctx.cumulative_frame_nr() >= frame_due {
                self.activity_scroll_frame = None;
            }
            ctx.request_repaint();
        } else if !self.capture_dialog
            && let Some(image) = self.capture.as_mut().and_then(|c| c.image(ctx, frame))
        {
            self.save_capture(image);
        }
    }
}
impl Dashboard {
    fn save_capture(&mut self, image: std::sync::Arc<egui::ColorImage>) {
        if let Some(capture) = self.capture.take() {
            #[cfg(windows)]
            let to_rgba = windows_viewport_capture::png_rgba;
            #[cfg(target_os = "macos")]
            let to_rgba = egui::Color32::to_srgba_unmultiplied;
            let rgba: Vec<u8> = image.pixels.iter().flat_map(to_rgba).collect();
            if image::save_buffer_with_format(
                &capture.path,
                &rgba,
                image.width() as u32,
                image.height() as u32,
                image::ColorType::Rgba8,
                image::ImageFormat::Png,
            )
            .is_ok()
            {
                self.wire.action("captured", json!(capture.path));
            }
            if self.capture_dialog {
                self.dialog = None;
            }
            self.capture_dialog = false;
        }
    }
}

fn client_size(_frame: &eframe::Frame, _ctx: &egui::Context, outer: [f32; 2]) -> egui::Vec2 {
    #[cfg(windows)]
    if let Some(hwnd) = hwnd(_frame) {
        use windows_sys::Win32::{
            Foundation::RECT,
            UI::WindowsAndMessaging::{
                GetClientRect, GetSystemMetrics, GetWindowRect, SM_CXMAXTRACK, SM_CYMAXTRACK,
            },
        };
        let mut window = RECT::default();
        let mut client = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut window) != 0 && GetClientRect(hwnd, &mut client) != 0 }
        {
            let dpi = _ctx.pixels_per_point();
            // WPF windows honor the maximum tracking dimensions, including the
            // virtual sizing frame, on desktops smaller than the requested size.
            let outer = [
                outer[0].min(unsafe { GetSystemMetrics(SM_CXMAXTRACK) } as f32 / dpi),
                outer[1].min(unsafe { GetSystemMetrics(SM_CYMAXTRACK) } as f32 / dpi),
            ];
            return egui::vec2(
                outer[0] - (window.right - window.left - client.right) as f32 / dpi,
                outer[1] - (window.bottom - window.top - client.bottom) as f32 / dpi,
            );
        }
    }
    outer.into()
}

#[cfg(windows)]
fn hwnd(frame: &eframe::Frame) -> Option<windows_sys::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as _),
        _ => None,
    }
}

fn set_taskbar(_frame: &eframe::Frame, _show: bool) {
    #[cfg(windows)]
    if let Some(hwnd) = hwnd(_frame) {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        unsafe {
            let current = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let style = if _show {
                current & !WS_EX_TOOLWINDOW | WS_EX_APPWINDOW
            } else {
                current & !WS_EX_APPWINDOW | WS_EX_TOOLWINDOW
            };
            if current != style {
                SetWindowLongW(hwnd, GWL_EXSTYLE, style as i32);
            }
        }
    }
}

#[cfg(windows)]
fn record_probe_error(message: &str) {
    if let Some(path) = std::env::var_os("QUOTABAR_PROBE_UI_ERROR") {
        let _ = std::fs::write(path, message);
    }
}

fn main() -> eframe::Result {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let Some((wire, inbox, wake)) = wire::Peer::start(&args) else {
        return Ok(());
    };
    // Wait on the private pipe before creating a visible OS window. A disconnected
    // or idle monitor must never leave an empty dashboard on screen.
    let mut state = State::new(args.iter().any(|s| s == "--demo"));
    let first = loop {
        match inbox.recv() {
            Ok(wire::Event::State(value)) => state.apply(value),
            Ok(event @ (wire::Event::Show(_) | wire::Event::Capture { .. })) => break event,
            Err(_) | Ok(wire::Event::Close) => return Ok(()),
            _ => {}
        }
    };
    let icon = image::load_from_memory(include_bytes!("../../windows/assets/QuotaBar.ico"))
        .expect("app icon")
        .into_rgba8();
    let viewport = egui::ViewportBuilder::default()
        .with_title("QuotaBar")
        .with_icon(egui::IconData {
            width: icon.width(),
            height: icon.height(),
            rgba: icon.into_raw(),
        })
        .with_inner_size(if cfg!(windows) {
            [1105., 773.]
        } else {
            [1120., 810.]
        })
        .with_min_inner_size(if cfg!(windows) {
            [884., 621.]
        } else {
            [900., 660.]
        });
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_clamp_size_to_monitor_size(false)
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false);
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport,
        centered: cfg!(windows),
        persist_window: false,
        vsync: true,
        // Native control surfaces have a uniform fill at every pixel.
        dithering: false,
        ..Default::default()
    };
    let create: eframe::AppCreator<'_> = Box::new(move |cc| {
        #[cfg(target_os = "macos")]
        {
            platform::restore_window(cc);
            quotabar::mac_menu::apply_appearance(state.setting("appearance", "system"));
            cc.egui_ctx.add_plugin(mac_text::Renderer::default());
        }
        style::fonts(&cc.egui_ctx);
        #[cfg(windows)]
        {
            platform::constrain_dashboard_window(cc);
            windows_choice::track_input(&cc.egui_ctx);
            cc.egui_ctx
                .tessellation_options_mut(|options| options.round_rects_to_pixels = false);
            cc.egui_ctx
                .add_plugin(windows_text::TextRenderer::default());
            cc.egui_ctx
                .add_plugin(windows_border::BorderRenderer::default());
        }
        let assets = style::Assets::new(&cc.egui_ctx);
        *wake.lock().unwrap() = Some(cc.egui_ctx.clone());
        let mut app = Dashboard {
            dialog_access: accessibility::DialogAccess::new(&cc.egui_ctx),
            wire,
            inbox,
            state,
            assets,
            route: "dashboard".into(),
            scroll_top: true,
            activity_scroll_frame: None,
            #[cfg(target_os = "macos")]
            activity_selection: activity::DaySelection::default(),
            filter: "전체 Provider".into(),
            query: String::new(),
            #[cfg(target_os = "macos")]
            focus_query: false,
            #[cfg(target_os = "macos")]
            account_search: None,
            attention: false,
            dialog: None,
            capture: None,
            capture_dialog: false,
            visible: None,
            focused: false,
            taskbar: None,
            #[cfg(target_os = "macos")]
            local: Default::default(),
            #[cfg(target_os = "macos")]
            app_menu: mac::AppMenu::install(&cc.egui_ctx),
            #[cfg(target_os = "macos")]
            scroller: mac::NativeScroller::attach(cc),
            #[cfg(target_os = "macos")]
            banner_label: mac::BannerLabel::attach(cc),
            #[cfg(target_os = "macos")]
            selectable_text: mac::SelectableText::attach(cc),
        };
        // Queue the initial event ahead of subsequent messages.
        app.wire.initial = Some(first);
        Ok(Box::new(app))
    });
    #[cfg(target_os = "macos")]
    let result = eframe::run_native("QuotaBar", options, create);
    #[cfg(windows)]
    let result = {
        let mut directx = options.clone();
        directx.renderer = eframe::Renderer::Wgpu;
        if let eframe::egui_wgpu::WgpuSetup::CreateNew(settings) =
            &mut directx.wgpu_options.wgpu_setup
        {
            settings.instance_descriptor.backends = wgpu::Backends::DX12;
        }
        // A packaged UI probe can exercise the fallback even on a machine with modern OpenGL.
        let options = if std::env::var_os("QUOTABAR_PROBE_UI_ERROR").is_some()
            && wgpu::Backends::from_env() == Some(wgpu::Backends::DX12)
        {
            directx.clone()
        } else {
            options
        };
        // OpenGL may initialize with only Microsoft's legacy 1.1 driver.
        // Its painter rejects that driver before consuming the application or IPC state.
        let mut pending = Some(create);
        let first = eframe::run_native(
            "QuotaBar",
            options,
            Box::new(|cc| pending.take().expect("one dashboard per helper")(cc)),
        );
        match (first, pending) {
            (
                Err(
                    eframe::Error::OpenGL(_)
                    | eframe::Error::Glutin(_)
                    | eframe::Error::NoGlutinConfigs(..),
                ),
                Some(create),
            ) => eframe::run_native("QuotaBar", directx, create),
            (result, _) => result,
        }
    };
    #[cfg(windows)]
    if let Err(error) = &result {
        record_probe_error(&error.to_string());
    }
    result
}
