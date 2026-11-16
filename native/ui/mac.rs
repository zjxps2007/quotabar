use crate::{
    Dashboard,
    dialogs::Connect,
    model::{MAC, State},
    wire::Peer,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{Local as Zone, Utc};
use eframe::egui::Context;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSWorkspace};
use objc2_foundation::{NSString, NSURL};
use quotabar::{
    mac_login::{self, Attempt, Progress},
    usage::Scanner,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

mod buttons;
pub use buttons::{draw_checkbox, draw_segments, draw_switch, popup_choice, standard_button};
mod banner;
pub use banner::BannerLabel;
mod selectable_text;
pub use selectable_text::{SelectableText, TextRole};
mod connection_fields;
pub use connection_fields::{AccountSearch, ConnectionFields, Input, connection_input};
mod rounded;
pub use rounded::{Curve, rounded_rect};
mod document;
pub use document::DocumentReader;
mod scroller;
pub use scroller::NativeScroller;

fn bitmap_rep(
    width: isize,
    height: isize,
) -> Option<objc2::rc::Retained<objc2_app_kit::NSBitmapImageRep>> {
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageRep, NSDeviceRGBColorSpace};
    unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(), std::ptr::null_mut(), width, height, 8, 4, true,
            false, NSDeviceRGBColorSpace, 0, 0,
        )
    }
}

fn bitmap_pixels(rep: &objc2_app_kit::NSBitmapImageRep) -> Option<image::RgbaImage> {
    use objc2_app_kit::NSBitmapImageFileType;
    use objc2_foundation::NSDictionary;
    // AppKit's encoding takes no options for these in-memory PNGs.
    let png = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(image::load_from_memory(&png.to_vec()).ok()?.to_rgba8())
}

fn template_texture(
    ctx: &Context,
    name: &str,
    image: &objc2_app_kit::NSImage,
) -> Option<eframe::egui::TextureHandle> {
    use eframe::egui::{Color32, ColorImage, TextureOptions};
    use objc2_app_kit::NSBitmapImageRep;
    let tiff = image.TIFFRepresentation()?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
    let bitmap = bitmap_pixels(&rep)?;
    let size = [bitmap.width() as usize, bitmap.height() as usize];
    let mask = bitmap
        .pixels()
        .map(|p| Color32::from_white_alpha(p[3]))
        .collect();
    Some(ctx.load_texture(name, ColorImage::new(size, mask), TextureOptions::LINEAR))
}

pub fn provider_logo(
    ctx: &Context,
    provider: &str,
    points: f32,
) -> Option<eframe::egui::TextureHandle> {
    use eframe::egui::{ColorImage, Id, TextureHandle, TextureOptions};
    use objc2_app_kit::{NSGraphicsContext, NSImageInterpolation};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let pixels = (points * ctx.pixels_per_point()).round().max(1.) as isize;
    let key = Id::new(("macOS provider artwork", provider, pixels));
    if let Some(cached) = ctx.data(|d| d.get_temp::<TextureHandle>(key)) {
        return Some(cached);
    }
    let side = pixels as f64;
    let source = quotabar::mac_menu::provider_image(provider, side)?;
    // Allocate the destination at backing resolution. AppKit then selects the
    // source representation and performs SwiftUI BrandIcon's high-quality resize.
    let bitmap = bitmap_rep(pixels, pixels)?;
    let graphics = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&graphics));
    graphics.setImageInterpolation(NSImageInterpolation::High);
    source.drawInRect(NSRect::new(NSPoint::ZERO, NSSize::new(side, side)));
    NSGraphicsContext::restoreGraphicsState_class();
    let rgba = bitmap_pixels(&bitmap)?;
    let texture = ctx.load_texture(
        provider,
        ColorImage::from_rgba_unmultiplied([pixels as usize; 2], rgba.as_raw()),
        TextureOptions::LINEAR,
    );
    ctx.data_mut(|d| d.insert_temp(key, texture.clone()));
    Some(texture)
}

pub fn app_mark(
    ui: &eframe::egui::Ui,
    rect: eframe::egui::Rect,
    ink: eframe::egui::Color32,
) -> bool {
    use eframe::egui::{Color32, ColorImage, Id, Pos2, Rect, TextureHandle, TextureOptions, pos2};
    use objc2_app_kit::NSGraphicsContext;

    let pixels = (rect.height() * ui.ctx().pixels_per_point())
        .round()
        .max(1.) as isize;
    let id = Id::new(("QuotaBar app mark", pixels));
    let cached = ui.data(|data| data.get_temp::<TextureHandle>(id));
    let texture = cached.or_else(|| {
        let side = pixels as f64;
        let source = quotabar::mac_menu::menu_preview("iconOnly", "")?;
        let rep = bitmap_rep(pixels, pixels)?;
        let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&context));
        quotabar::mac_menu::draw_app_mark(&source, side);
        NSGraphicsContext::restoreGraphicsState_class();
        let image = bitmap_pixels(&rep)?;
        let mask = image
            .pixels()
            .map(|p| Color32::from_white_alpha(p[3]))
            .collect();
        let texture = ui.ctx().load_texture(
            "AppMark.svg",
            ColorImage::new([pixels as usize; 2], mask),
            TextureOptions::NEAREST,
        );
        ui.data_mut(|data| data.insert_temp(id, texture.clone()));
        Some(texture)
    });
    if let Some(texture) = texture {
        ui.painter().image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
            ink,
        );
        true
    } else {
        false
    }
}

pub fn menu_preview(
    ui: &mut eframe::egui::Ui,
    mode: &str,
    value: &str,
    ink: eframe::egui::Color32,
) {
    use eframe::egui::{
        Id, Pos2, Rect, Sense, TextureHandle, Vec2, WidgetInfo, WidgetType, pos2, vec2,
    };
    let ctx = ui.ctx();
    let key = Id::new((
        "menu-preview",
        mode,
        value,
        ctx.pixels_per_point().to_bits(),
    ));
    let cached = ctx.data(|d| d.get_temp::<(TextureHandle, Vec2)>(key));
    let item = cached.or_else(|| {
        let image = quotabar::mac_menu::menu_preview(mode, value)?;
        let size = image.size();
        let item = (
            template_texture(ctx, "menu preview", &image)?,
            vec2(size.width as f32, size.height as f32),
        );
        ctx.data_mut(|d| d.insert_temp(key, item.clone()));
        Some(item)
    });
    let spoken = if mode == "iconOnly" {
        "아이콘만"
    } else {
        value
    };
    if let Some((texture, size)) = item {
        let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
        ui.painter().image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
            ink,
        );
        response.widget_info(|| {
            WidgetInfo::labeled(
                WidgetType::Image,
                true,
                format!("상단바 미리보기 · {spoken}"),
            )
        });
    } else {
        ui.label(spoken);
    }
}

fn symbol_texture(
    ctx: &Context,
    name: &str,
    points: f32,
    semibold: bool,
) -> Option<eframe::egui::TextureHandle> {
    use eframe::egui::{Id, TextureHandle};
    use objc2_app_kit::{
        NSFontWeightRegular, NSFontWeightSemibold, NSImage, NSImageSymbolConfiguration,
    };

    let scale = ctx.pixels_per_point();
    let pixels = (points * scale).round().max(1.) as u32;
    let id = Id::new(("appkit-symbol", name, pixels, semibold));
    let cached = ctx.data(|d| d.get_temp::<Option<TextureHandle>>(id));
    cached.unwrap_or_else(|| {
        let load = || {
            let base = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(name),
                None,
            )?;
            let weight = unsafe {
                if semibold {
                    NSFontWeightSemibold
                } else {
                    NSFontWeightRegular
                }
            };
            let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(
                f64::from(pixels),
                weight,
            );
            let image = base.imageWithSymbolConfiguration(&config)?;
            template_texture(ctx, name, &image)
        };
        let texture = load();
        ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
        texture
    })
}

pub fn symbol_size(
    ctx: &Context,
    name: &str,
    points: f32,
    semibold: bool,
) -> Option<eframe::egui::Vec2> {
    Some(symbol_texture(ctx, name, points, semibold)?.size_vec2() / ctx.pixels_per_point())
}

pub fn intrinsic_symbol(
    ui: &mut eframe::egui::Ui,
    name: &str,
    points: f32,
    semibold: bool,
    tint: eframe::egui::Color32,
) -> bool {
    use eframe::egui::{Pos2, Rect, Sense, pos2};
    let Some(texture) = symbol_texture(ui.ctx(), name, points, semibold) else {
        return false;
    };
    let size = texture.size_vec2() / ui.ctx().pixels_per_point();
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().image(
        texture.id(),
        rect,
        Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
        tint,
    );
    true
}

pub fn symbol(
    painter: &eframe::egui::Painter,
    bounds: eframe::egui::Rect,
    name: &str,
    points: f32,
    semibold: bool,
    tint: eframe::egui::Color32,
) -> bool {
    use eframe::egui::{Pos2, Rect, pos2};
    let Some(texture) = symbol_texture(painter.ctx(), name, points, semibold) else {
        return false;
    };
    let size = texture.size_vec2() / painter.ctx().pixels_per_point();
    let fit = (bounds.width() / size.x)
        .min(bounds.height() / size.y)
        .min(1.);
    painter.image(
        texture.id(),
        Rect::from_center_size(bounds.center(), size * fit),
        Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
        tint,
    );
    true
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(Into::into).unwrap_or_default()
}
pub fn data_path() -> PathBuf {
    home().join("Library/Application Support/AIUsage Lite")
}
pub fn open_data() {
    let path = data_path();
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    NSWorkspace::sharedWorkspace().openURL(&url);
}
pub struct AppMenu {
    _menu: quotabar::mac_menu::Menus,
    _target: Box<(mpsc::Sender<i32>, Context)>,
    commands: mpsc::Receiver<i32>,
}
impl AppMenu {
    pub fn install(ctx: &Context) -> Self {
        use std::ffi::c_void;
        extern "C" fn command(pointer: *mut c_void, action: i32) {
            let (tx, ctx) = unsafe { &*pointer.cast::<(mpsc::Sender<i32>, Context)>() };
            let _ = tx.send(action);
            ctx.request_repaint();
        }
        let mtm = MainThreadMarker::new().expect("AppKit main thread");
        NSApplication::sharedApplication(mtm)
            .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        let (send, commands) = mpsc::channel();
        let mut target = Box::new((send, ctx.clone()));
        let menu = quotabar::mac_menu::Menus::install(
            mtm,
            (&mut *target as *mut (mpsc::Sender<i32>, Context)).cast(),
            command,
        );
        Self {
            _menu: menu,
            _target: target,
            commands,
        }
    }
}
impl Connect {
    pub fn start_login(&mut self) {
        self.error = None;
        self.login_url = None;
        let Some(exe) = mac_login::find(&std::env::var("PATH").unwrap_or_default(), &home()) else {
            self.error = Some(
                "Codex 실행 파일을 찾지 못했습니다. Codex CLI 또는 ChatGPT 데스크톱 앱을 설치한 뒤 다시 시도하세요."
                    .into(),
            );
            return;
        };
        match Attempt::start(&exe, &std::env::temp_dir(), Duration::from_secs(300)) {
            Ok(a) => self.login = Some(a),
            Err(_) => {
                self.error =
                    Some("Codex 로그인을 시작하지 못했습니다. 설치된 Codex를 확인하세요.".into())
            }
        }
    }
    pub fn poll(&mut self, wire: &Peer, ctx: &Context) {
        let Some(a) = self.login.as_mut() else { return };
        match a.poll() {
            Ok(Progress::Waiting) => {}
            Ok(Progress::Authorized(url)) => {
                self.login_url = Some(url);
            }
            Ok(Progress::Finished(bytes)) => {
                if let Some(a) = self.login.take() {
                    a.close(false);
                }
                let mut request = json!({"data":STANDARD.encode(&*bytes),"alias":self.alias});
                if let Some(id) = &self.replacing {
                    request["replacing"] = json!(id);
                }
                let id = wire.action("codexLogin", request);
                self.wait_for_reply(id, ctx);
            }
            other => {
                self.login.take();
                self.error = Some(
                    match other {
                        Ok(Progress::TimedOut) => "로그인 대기 시간이 지났습니다. 다시 로그인하세요.",
                        Ok(Progress::Failed) => "로그인이 완료되지 않았습니다. 다른 Codex 로그인 창을 닫고 다시 시도하세요.",
                        _ => "로그인을 완료하지 못했습니다. 다시 시도하세요.",
                    }
                    .into(),
                );
            }
        }
        ctx.request_repaint_after(Duration::from_millis(150));
    }
}
type ScanResult = mpsc::Receiver<std::io::Result<Value>>;
#[derive(Default)]
pub struct Local {
    scanner: Arc<Mutex<Scanner>>,
    running: Option<(Arc<AtomicBool>, ScanResult)>,
    last: Option<Instant>,
    roots: Vec<PathBuf>,
}
impl Drop for Local {
    fn drop(&mut self) {
        if let Some((stop, _)) = &self.running {
            stop.store(true, Ordering::Relaxed);
        }
    }
}
impl Local {
    pub fn invalidate(&mut self) {
        self.last = None;
    }
    fn update(&mut self, ctx: &Context, state: &mut State, visible: bool) {
        if !MAC || state.demo {
            return;
        }
        let enabled = state.flag("scanLocalLogs", "scanLocalLogs");
        let mut claude = vec![PathBuf::from(state.defaults("Claude")).join("projects")];
        claude.extend(
            state
                .accounts
                .iter()
                .filter(|a| a.provider == "Claude" && a.enabled && !a.path.is_empty())
                .map(|a| PathBuf::from(&a.path).join("projects")),
        );
        claude.sort();
        claude.dedup();
        let active = enabled && visible;
        if !active || claude != self.roots {
            if let Some((cancel, _)) = &self.running {
                cancel.store(true, Ordering::Relaxed);
            }
            if self.roots != claude {
                self.last = None;
                self.roots = claude.clone();
            }
            if !enabled {
                state.report = Value::Null;
                self.last = None;
            }
            if !active && let Ok(mut scanner) = self.scanner.try_lock() {
                scanner.release_cache();
            }
        }
        if let Some((cancel, rx)) = &self.running {
            match rx.try_recv() {
                Ok(result) => {
                    if cancel.load(Ordering::Relaxed) {
                        self.last = None;
                    } else {
                        match result {
                            Ok(report) => state.report = report,
                            Err(_) => {
                                state.raw["message"] =
                                    json!("로컬 기록을 읽지 못했습니다. 로그 폴더를 확인하세요.")
                            }
                        }
                    }
                    self.running = None;
                    state.raw["scanning"] = json!(false);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.running = None;
                    state.raw["scanning"] = json!(false);
                }
                _ => {
                    state.raw["scanning"] = json!(true);
                    return;
                }
            }
        }
        if !active {
            return;
        }
        let interval = Duration::from_secs(quotabar::schedule::LOCAL_INTERVAL as u64);
        if let Some(last) = self.last {
            if !state.flag("autoRefresh", "autoRefresh") {
                return;
            }
            if last.elapsed() < interval {
                ctx.request_repaint_after(interval - last.elapsed().min(interval));
                return;
            }
        }
        let dir = PathBuf::from(state.defaults("Codex"));
        let Some(root) = dir.parent() else { return };
        let codex = [root.join("sessions"), root.join("archived_sessions")];
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = stop.clone();
        let (tx, rx) = mpsc::channel();
        let scanner = self.scanner.clone();
        let wake = ctx.clone();
        self.running = Some((stop, rx));
        self.last = Some(Instant::now());
        state.raw["scanning"] = json!(true);
        std::thread::spawn(move || {
            let result = scanner
                .lock()
                .unwrap()
                .scan(&codex, &claude, Utc::now(), &Zone, 182, &cancel)
                .and_then(|r| serde_json::to_value(r).map_err(std::io::Error::other));
            let _ = tx.send(result);
            wake.request_repaint();
        });
    }
}
impl Dashboard {
    pub fn mac_tick(&mut self, ctx: &Context) {
        while let Ok(action) = self.app_menu.commands.try_recv() {
            match action {
                quotabar::mac_menu::REFRESH => self.refresh(),
                quotabar::mac_menu::DASHBOARD => self.navigate(ctx, "dashboard", true),
                quotabar::mac_menu::SETTINGS => self.navigate(ctx, "settings", true),
                quotabar::mac_menu::QUIT => {
                    self.wire.action("quit", Value::Null);
                }
                quotabar::mac_menu::POPOVER => {
                    self.wire.action("showPopover", Value::Null);
                }
                _ => {}
            }
        }
        self.local.update(
            ctx,
            &mut self.state,
            self.visible == Some(true) && self.route == "dashboard",
        );
    }
}
