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

pub fn symbol(
    painter: &eframe::egui::Painter,
    bounds: eframe::egui::Rect,
    name: &str,
    points: f32,
    semibold: bool,
    tint: eframe::egui::Color32,
) -> bool {
    use eframe::egui::{Color32, ColorImage, Id, Pos2, Rect, TextureHandle, TextureOptions, pos2};
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSFontWeightRegular, NSFontWeightSemibold,
        NSImage, NSImageSymbolConfiguration,
    };
    use objc2_foundation::NSDictionary;

    let ctx = painter.ctx();
    let scale = ctx.pixels_per_point();
    let pixels = (points * scale).round().max(1.) as u32;
    let id = Id::new(("appkit-symbol", name, pixels, semibold));
    let cached = ctx.data(|d| d.get_temp::<Option<TextureHandle>>(id));
    let texture = cached.unwrap_or_else(|| {
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
            let tiff = image.TIFFRepresentation()?;
            let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
            // An empty encoding options dictionary takes no typed values.
            let png = unsafe {
                rep.representationUsingType_properties(
                    NSBitmapImageFileType::PNG,
                    &NSDictionary::new(),
                )
            }?;
            let bitmap = image::load_from_memory(&png.to_vec()).ok()?.to_rgba8();
            let size = [bitmap.width() as usize, bitmap.height() as usize];
            let mask = bitmap
                .pixels()
                .map(|p| Color32::from_white_alpha(p[3]))
                .collect();
            Some(ctx.load_texture(name, ColorImage::new(size, mask), TextureOptions::LINEAR))
        };
        let texture = load();
        ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
        texture
    });
    let Some(texture) = texture else { return false };
    let size = texture.size_vec2() / scale;
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
                "Codex CLI를 찾지 못했습니다. Codex CLI를 설치하거나 로그인 파일을 가져오세요."
                    .into(),
            );
            return;
        };
        match Attempt::start(&exe, &std::env::temp_dir(), Duration::from_secs(300)) {
            Ok(a) => self.login = Some(a),
            Err(_) => self.error = Some("Codex 로그인을 시작하지 못했습니다.".into()),
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
                self.pending = Some((id, Instant::now()));
            }
            other => {
                self.login.take();
                self.error = Some(
                    if matches!(other, Ok(Progress::TimedOut)) {
                        "로그인 시간이 초과되었습니다. 다시 시도하세요."
                    } else {
                        "로그인을 완료하지 못했습니다. 다시 시도하세요."
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
                quotabar::mac_menu::DASHBOARD => self.navigate("dashboard", true),
                quotabar::mac_menu::SETTINGS => self.navigate("settings", true),
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
