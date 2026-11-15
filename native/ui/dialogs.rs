#[cfg(target_os = "macos")]
use crate::style::label as form_text;
#[cfg(windows)]
use crate::windows_text::text_block as form_text;
use crate::{Dashboard, management::segments, model::*, style::*};
use eframe::egui::*;
#[cfg(windows)]
use serde_json::Value;
use serde_json::json;
use std::time::{Duration, Instant};

fn full_button(ui: &mut Ui, title: &str, primary: bool, p: Palette) -> bool {
    let fill = if primary { p.accent } else { p.card };
    let ink = if primary { p.on } else { p.ink };
    #[cfg(windows)]
    let height = crate::windows_text::line_height(13.).unwrap_or(17.) + 16.;
    #[cfg(target_os = "macos")]
    let height = 32.;
    button_feedback(ui, p.bg, ink, vec2(10., 5.), |ui| {
        ui.add_sized(
            [ui.available_width(), height],
            Button::new(rich(title, if MAC { 12. } else { 13. }, false, ink))
                .corner_radius(if MAC { 7 } else { 8 })
                .fill(fill)
                .stroke(Stroke::new(
                    1.0_f32,
                    if primary { p.accent } else { p.line },
                )),
        )
    })
    .clicked()
}

pub struct Connect {
    pub provider: &'static str,
    pub path: String,
    pub alias: String,
    pub replacing: Option<String>,
    pub consent: bool,
    pub browser: bool,
    pub pending: Option<(String, Instant)>,
    pub error: Option<String>,
    #[cfg(target_os = "macos")]
    pub login: Option<quotabar::mac_login::Attempt>,
    #[cfg(target_os = "macos")]
    pub login_url: Option<String>,
    content_height: Option<f32>,
}
pub enum Dialog {
    Connect(Box<Connect>),
    Rename(Account, String),
    Remove {
        account: Account,
        decision: Option<std::sync::mpsc::Receiver<bool>>,
    },
    Licenses {
        docs: Vec<(String, String)>,
        selected: usize,
    },
}
impl Dialog {
    pub fn remove(account: Account) -> Self {
        Self::Remove {
            account,
            decision: None,
        }
    }

    pub fn licenses() -> Self {
        let docs = vec![
            (
                "저작권 및 변경 고지".into(),
                include_str!("../../NOTICE").into(),
            ),
            (
                "Apache License 2.0 전문".into(),
                include_str!("../../LICENSE").into(),
            ),
        ];
        #[cfg(windows)]
        let mut docs = docs;
        #[cfg(windows)]
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent()?.parent().map(|p| p.join("licenses")))
            && let Ok(frameworks) = std::fs::read_dir(dir)
        {
            let mut files = vec![];
            for framework in frameworks.flatten() {
                if let Ok(entries) = std::fs::read_dir(framework.path()) {
                    for f in entries.flatten() {
                        let name = f.file_name().to_string_lossy().to_uppercase();
                        if name.starts_with("LICENSE")
                            || name.contains("THIRD") && name.contains("NOTICE")
                        {
                            files.push((
                                format!(
                                    "{} · {}",
                                    framework.file_name().to_string_lossy(),
                                    f.file_name().to_string_lossy()
                                ),
                                f.path(),
                            ));
                        }
                    }
                }
            }
            files.sort_by_cached_key(|(title, _)| title.to_lowercase());
            for (title, path) in files {
                let mut content = std::fs::read_to_string(path).unwrap_or_else(|_| {
                    "고지 파일을 읽을 수 없습니다. 앱을 다시 설치해 주세요.".into()
                });
                if content.starts_with('\u{feff}') {
                    content.drain(..'\u{feff}'.len_utf8());
                }
                docs.push((title, content));
            }
        }
        Self::Licenses { docs, selected: 0 }
    }
}
impl Dashboard {
    pub fn open_connect(&mut self, p: &str, target: Option<&Account>) {
        self.dialog = Some(Dialog::Connect(Box::new(Connect {
            provider: provider(p),
            path: self.state.defaults(p),
            alias: target.map(|a| a.name.clone()).unwrap_or_default(),
            replacing: target.map(|a| a.id.clone()),
            consent: false,
            browser: true,
            pending: None,
            error: None,
            #[cfg(target_os = "macos")]
            login: None,
            #[cfg(target_os = "macos")]
            login_url: None,
            content_height: None,
        })));
    }
    pub fn finish_request(&mut self, id: &str, error: Option<String>) {
        if let Some(Dialog::Connect(c)) = &mut self.dialog
            && c.pending
                .as_ref()
                .is_some_and(|(key, _)| id.eq_ignore_ascii_case(key))
        {
            c.pending = None;
            if error.is_none() {
                self.dialog = None;
            } else {
                c.error = error;
            }
        }
    }
    pub fn show_dialog(&mut self, ctx: &Context, frame: &eframe::Frame, p: Palette) {
        let Some(mut dialog) = self.dialog.take() else {
            // A monitor reply can dismiss the sheet before this frame starts.
            self.dialog_access.close();
            super::platform::dialog_owner(frame, "QuotaBar", false);
            return;
        };
        if let Dialog::Remove { account, decision } = &mut dialog {
            let reply = decision.get_or_insert_with(|| {
                let (send, receive) = std::sync::mpsc::channel();
                let wake = ctx.clone();
                super::platform::confirm_remove(frame, account, move |confirmed| {
                    let _ = send.send(confirmed);
                    wake.request_repaint();
                });
                receive
            });
            match reply.try_recv() {
                Ok(true) => {
                    self.wire.action("remove", json!(account.id));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => self.dialog = Some(dialog),
                _ => {}
            }
            return;
        }
        let dialog_width = if MAC {
            550.
        } else {
            super::client_size(frame, ctx, [590., 500.]).x
        };
        let (title, width, height) = match &dialog {
            Dialog::Connect(c) => (
                "QuotaBar · 계정 연결",
                dialog_width,
                c.content_height.unwrap_or(if MAC { 496. } else { 460. }),
            ),
            Dialog::Rename(..) => {
                #[cfg(windows)]
                let height = {
                    let scale = ctx.pixels_per_point();
                    let title = crate::windows_text::line_height(20.).unwrap_or(27.);
                    let text = crate::windows_text::line_height(13.).unwrap_or(17.);
                    // Two 18-DIP gaps, window margins, text field and button padding.
                    ((142. + title + text * 2.) * scale).ceil() / scale
                };
                #[cfg(target_os = "macos")]
                let height = 199.;
                ("계정 이름", dialog_width, height)
            }
            Dialog::Remove { .. } => unreachable!(),
            Dialog::Licenses { .. } => {
                let size = if MAC {
                    vec2(720., 580.)
                } else {
                    super::client_size(frame, ctx, [760., 620.])
                };
                ("오픈소스 라이선스", size.x, size.y)
            }
        };
        let resizable = !MAC && matches!(dialog, Dialog::Licenses { .. });
        let mut close = false;
        let mut browse = false;
        let mut capture = if self.capture_dialog {
            self.capture.take()
        } else {
            None
        };
        let mut content = |ctx: &Context, capture: Option<&mut super::Capture>| {
            self.dialog_access.install(ctx, frame, title);
            ctx.send_viewport_cmd(ViewportCommand::Visible(true));
            if ctx.input(|i| i.viewport().close_requested())
                || !Popup::is_any_open(ctx)
                    && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
            {
                close = true;
            }
            CentralPanel::default()
                .frame(
                    Frame::new()
                        .fill(if MAC { p.card } else { p.bg })
                        .inner_margin(if matches!(&dialog, Dialog::Licenses { .. }) {
                            24
                        } else {
                            28
                        }),
                )
                .show(ctx, |ui| match &mut dialog {
                    Dialog::Connect(c) => {
                        #[cfg(target_os = "macos")]
                        let limit = (ctx
                            .input(|i| i.viewport().monitor_size.map(|size| size.y))
                            .unwrap_or(f32::INFINITY)
                            - 80.)
                            .max(160.);
                        #[cfg(windows)]
                        let limit = super::client_size(frame, ctx, [590., 720.]).y;
                        let scroll = ScrollArea::vertical().auto_shrink([false, true]);
                        // The Windows dialog sizes itself to the controls. A fractional
                        // pixel at its natural height must not reserve scrollbar space.
                        #[cfg(windows)]
                        let scroll = if c.content_height.is_none_or(|height| height < limit) {
                            scroll.scroll_bar_visibility(
                                scroll_area::ScrollBarVisibility::AlwaysHidden,
                            )
                        } else {
                            scroll
                        };
                        let scroll = scroll.show(ui, |ui| {
                            close |= self.connect_panel(ui, c, p, &mut browse);
                        });
                        let scale = if MAC { 1. } else { ctx.pixels_per_point() };
                        let pixels = (scroll.content_size.y + 56.) * scale;
                        let measured =
                            (if MAC { pixels.ceil() } else { pixels.round() } / scale).min(limit);
                        if c.content_height != Some(measured) {
                            c.content_height = Some(measured);
                            ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(
                                width, measured,
                            )));
                            ctx.request_repaint_of(ViewportId::ROOT);
                        }
                    }
                    Dialog::Rename(a, name) => {
                        form_text(ui, "계정 이름", 20., true, p.ink);
                        ui.add_space(18.);
                        let field = ui.add(
                            TextEdit::singleline(&mut crate::account_text::AccountField(name))
                                .font(FontId::proportional(13.))
                                .margin(Margin::symmetric(if MAC { 18 } else { 19 }, 17))
                                .desired_width(f32::INFINITY),
                        );
                        crate::accessibility::text_value(ui, field, name, "계정 이름");
                        ui.add_space(18.);
                        if full_button(ui, "저장", true, p) && !name.trim().is_empty() {
                            self.edit_account(a, name, a.enabled, a.tray);
                            close = true;
                        }
                    }
                    Dialog::Remove { .. } => unreachable!(),
                    Dialog::Licenses { docs, selected } => {
                        #[cfg(windows)]
                        ui.shrink_clip_rect(ui.max_rect());
                        form_text(ui, "오픈소스 라이선스", 22., true, p.ink);
                        ui.add_space(16.);
                        #[cfg(target_os = "macos")]
                        {
                            let items = [
                                ("notice", docs[0].0.as_str()),
                                ("license", docs[1].0.as_str()),
                            ];
                            if let Some(key) = segments(ui, &items, items[*selected].0, p) {
                                *selected = usize::from(key == "license");
                            }
                        }
                        #[cfg(windows)]
                        if let Some(index) =
                            crate::windows_choice::choose_document(ui, docs, *selected)
                        {
                            *selected = index;
                        }
                        ui.add_space(16.);
                        let height = (ui.available_height() - 48.).max(80.);
                        Frame::new()
                            .fill(if MAC { p.bg } else { p.card })
                            .stroke(Stroke::new(1.0_f32, p.line))
                            .corner_radius(if MAC { 10 } else { 7 })
                            .inner_margin(if MAC { 16 } else { 14 })
                            .show(ui, |ui| {
                                ScrollArea::vertical()
                                    .id_salt(*selected)
                                    .max_height(height - 34.)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        #[cfg(target_os = "macos")]
                                        let reader = ui.add(
                                            Label::new(
                                                RichText::new(&docs[*selected].1)
                                                    .monospace()
                                                    .size(12.),
                                            )
                                            .selectable(true),
                                        );
                                        #[cfg(target_os = "macos")]
                                        ui.ctx().accesskit_node_builder(reader.id, |node| {
                                            node.set_label(docs[*selected].0.as_str())
                                        });
                                        #[cfg(windows)]
                                        {
                                            let mut text = docs[*selected].1.as_str();
                                            let reader = ui.add(
                                                TextEdit::multiline(&mut text)
                                                    .font(FontId::monospace(12.))
                                                    .frame(false)
                                                    .margin(0.)
                                                    .id_salt(("license-text", *selected))
                                                    .desired_width(f32::INFINITY),
                                            );
                                            ui.ctx().accesskit_node_builder(reader.id, |node| {
                                                node.set_label("라이선스 전문");
                                                node.set_read_only();
                                                node.set_value(text);
                                            });
                                        }
                                    });
                            });
                        ui.add_space(12.);
                        if ui
                            .with_layout(Layout::right_to_left(Align::Min), |ui| {
                                action_on(ui, "닫기", false, p, p.bg)
                            })
                            .inner
                            .clicked()
                        {
                            close = true;
                        }
                    }
                });
            capture.and_then(|c| c.dialog_image(ctx))
        };
        {
            let id = crate::accessibility::viewport();
            let center = ctx.input(|i| i.viewport().inner_rect.map(|r| r.center()));
            let mut builder = ViewportBuilder::default()
                .with_title(title)
                .with_visible(false)
                .with_inner_size([width, height])
                .with_resizable(resizable)
                .with_minimize_button(resizable)
                .with_maximize_button(resizable)
                .with_taskbar(false);
            #[cfg(windows)]
            if resizable {
                builder = builder.with_min_inner_size(super::client_size(frame, ctx, [600., 420.]));
            }
            if let Some(c) = center {
                builder = builder.with_position(c - vec2(width, height) * 0.5);
            }
            let pixels = ctx.show_viewport_immediate(id, builder, |dialog_ctx, _| {
                content(dialog_ctx, capture.as_mut())
            });
            self.dialog_access.publish();
            if self.capture_dialog {
                self.capture = capture;
                if let Some(image) = pixels {
                    self.save_capture(image);
                    close = true;
                }
            }
        }
        super::platform::dialog_owner(frame, title, !close);
        if browse
            && !close
            && let Dialog::Connect(c) = &mut dialog
        {
            // A native picker runs a nested OS event loop. Enter it only after
            // egui finished its child pass and published the accessibility tree.
            if let Some(path) = crate::platform::choose_path(frame, c.provider, &c.path) {
                c.path = path;
            }
            ctx.request_repaint();
        }
        if close {
            self.dialog_access.close();
        }
        if !close {
            self.dialog = Some(dialog);
        }
    }
    fn connect_panel(
        &self,
        ui: &mut Ui,
        c: &mut Connect,
        p: Palette,
        browse_requested: &mut bool,
    ) -> bool {
        if c.pending
            .as_ref()
            .is_some_and(|(_, start)| start.elapsed() > Duration::from_secs(30))
        {
            c.pending = None;
            c.error = Some("모니터가 응답하지 않습니다. 다시 시도하세요.".into());
        }
        #[cfg(target_os = "macos")]
        c.poll(&self.wire, ui.ctx());
        let locked = c.pending.is_some() || c.logging();
        let title = if c.replacing.is_some() {
            "Codex 다시 로그인"
        } else {
            "계정 연결"
        };
        if MAC {
            let dismissed = ui
                .horizontal(|ui| {
                    label(ui, title, 20., true, p.ink);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        link(ui, "×", p, p.card).on_hover_text("닫기").clicked()
                    })
                    .inner
                })
                .inner;
            if dismissed {
                return true;
            }
        } else {
            form_text(ui, title, 24., true, p.ink);
        }
        ui.add_space(if MAC { 20. } else { 0. });
        form_text(
            ui,
            if !MAC {
                "구독 사용량을 확인할 서비스를 선택하세요."
            } else if c.browser && c.provider == "Codex" {
                "브라우저에서 로그인하면 계정 연결과 토큰 갱신을 앱이 관리합니다."
            } else {
                "CLI에 로그인한 계정을 가져와 모니터링합니다."
            },
            12.,
            false,
            p.muted,
        );
        ui.add_space(20.);
        let prev = c.provider;
        ui.add_enabled_ui(!locked && c.replacing.is_none(), |ui| {
            if MAC {
                if let Some(s) = segments(
                    ui,
                    &[
                        ("Codex", "Codex"),
                        ("Claude", "Claude"),
                        ("Gemini", "Gemini"),
                    ],
                    c.provider,
                    p,
                ) {
                    c.provider = provider(s);
                }
            } else {
                ui.spacing_mut().button_padding = vec2(10., 7.);
                let providers = ["Codex", "Claude", "Gemini"];
                let selected = providers.iter().position(|s| *s == c.provider).unwrap_or(0);
                if let Some(index) = choose(
                    ui,
                    "연결 서비스",
                    ui.available_width(),
                    &providers,
                    selected,
                    13.,
                ) {
                    c.provider = providers[index];
                }
            }
        });
        if prev != c.provider {
            c.path = self.state.defaults(c.provider);
            c.consent = false;
            c.error = None;
        }
        ui.add_space(20.);
        if MAC && c.provider == "Codex" && c.replacing.is_none() {
            ui.add_enabled_ui(!locked, |ui| {
                if let Some(s) = segments(
                    ui,
                    &[("browser", "브라우저 로그인"), ("file", "파일 가져오기")],
                    if c.browser { "browser" } else { "file" },
                    p,
                ) {
                    c.browser = s == "browser";
                    c.error = None;
                }
            });
            ui.add_space(20.);
        }
        if MAC {
            label(ui, "계정 이름", 11., true, p.ink);
            ui.add_space(8.);
            let field = ui.add_enabled(
                !locked,
                TextEdit::singleline(&mut c.alias)
                    .desired_width(f32::INFINITY)
                    .hint_text("예: 개인 계정, 연구실"),
            );
            crate::accessibility::text_value(ui, field, &mut c.alias, "계정 이름");
            ui.add_space(20.);
        }
        let browser = MAC && c.provider == "Codex" && c.browser;
        let full = |ui: &mut Ui, s: &str, primary: bool| full_button(ui, s, primary, p);
        #[cfg(windows)]
        if c.provider == "Codex" {
            form_text(
                ui,
                "브라우저 로그인은 Codex CLI가 필요합니다. 앱 계정은 CLI의 현재 로그인과 별도로 유지됩니다.",
                12.,
                false,
                p.muted,
            );
            ui.add_space(12.);
            let busy = self.state.raw["loggingIn"] == true;
            if ui
                .add_enabled_ui(!busy, |ui| {
                    full(
                        ui,
                        if busy {
                            "로그인 진행 중"
                        } else {
                            "브라우저로 Codex 로그인"
                        },
                        true,
                    )
                })
                .inner
            {
                self.wire.action("login", Value::Null);
                return true;
            }
            ui.add_space(18.);
        }
        if !browser {
            let description = if c.provider == "Claude" {
                "Claude 설정 폴더"
            } else if MAC {
                "로그인 JSON 파일"
            } else {
                "또는 CLI 로그인 파일 연결"
            };
            form_text(ui, description, 11., MAC, if MAC { p.ink } else { p.muted });
            ui.add_space(if MAC { 8. } else { 7. });
            let browse = ui
                .add_enabled_ui(!locked, |ui| {
                    if MAC {
                        ui.horizontal(|ui| {
                            let width = (ui.available_width() - 92.).max(40.);
                            let field = ui.add(
                                TextEdit::singleline(&mut c.path)
                                    .desired_width(width)
                                    .hint_text("경로"),
                            );
                            crate::accessibility::text_value(ui, field, &mut c.path, "경로");
                            action(ui, "찾아보기…", false, p).clicked()
                        })
                        .inner
                    } else {
                        let editor = TextEdit::singleline(&mut c.path)
                            .desired_width(f32::INFINITY)
                            .font(FontId::proportional(13.))
                            .margin(Margin::symmetric(19, 17));
                        #[cfg(windows)]
                        let editor = editor.min_size(vec2(
                            0.,
                            crate::windows_text::line_height(13.).unwrap_or(17.) + 34.,
                        ));
                        let field = ui.add(editor);
                        crate::accessibility::text_value(ui, field, &mut c.path, "경로");
                        ui.add_space(10.);
                        full(ui, "찾아보기", false)
                    }
                })
                .inner;
            *browse_requested = browse;
            ui.add_space(if MAC { 20. } else { 18. });
        }
        #[cfg(target_os = "macos")]
        c.connection_help(ui, browser, p);
        #[cfg(windows)]
        if c.provider == "Claude" {
            form_text(
                ui,
                "settings.json의 status line을 연결합니다. 기존 명령은 계속 실행되며, 연결을 해제하면 복원됩니다. Claude Code를 다시 시작하고 응답을 받아야 사용량이 표시됩니다.",
                12.,
                false,
                p.muted,
            );
            // The TextBlock's bottom margin meets the CheckBox's own top margin.
            ui.add_space(15.);
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = 0.;
                checkbox(
                    ui,
                    &mut c.consent,
                    "기존 status line을 보존하는 연결에 동의합니다",
                    13.,
                    p,
                );
            });
        } else {
            let note = if c.provider == "Gemini" {
                "Gemini Code Assist의 현재 지원 대상 조직 계정만 연결할 수 있습니다. 개인용 종료 및 서버 정책에 따라 조회가 거절될 수 있습니다."
            } else {
                "연결한 파일은 읽기만 합니다. 만료되면 CLI에서 다시 로그인하세요."
            };
            form_text(ui, note, 12., false, p.muted);
        }
        if let Some(error) = &c.error {
            ui.add_space(if MAC { 20. } else { 10. });
            let color = if MAC {
                if p.dark {
                    Color32::from_rgb(255, 69, 58)
                } else {
                    Color32::from_rgb(255, 59, 48)
                }
            } else {
                p.warning
            };
            ui.add(Label::new(rich(error, 12., false, color)).selectable(true));
        }
        ui.add_space(if MAC { 20. } else { 22. });
        let enabled = (!MAC || !self.state.demo)
            && (c.provider != "Claude" || c.consent)
            && (!MAC || browser || !c.path.is_empty())
            && !locked;
        let mut close = false;
        let submit = if MAC {
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 31.),
                Layout::right_to_left(Align::Center),
                |ui| {
                    let submit = ui
                        .add_enabled_ui(enabled, |ui| {
                            action(
                                ui,
                                if browser {
                                    "ChatGPT로 로그인"
                                } else {
                                    "연결"
                                },
                                true,
                                p,
                            )
                        })
                        .inner
                        .clicked();
                    close = action(ui, "취소", false, p).clicked();
                    submit
                },
            )
            .inner
                || enabled && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter))
        } else {
            ui.add_enabled_ui(enabled, |ui| full(ui, "연결", true))
                .inner
        };
        if !submit {
            return close;
        }
        if browser {
            #[cfg(target_os = "macos")]
            c.start_login();
        } else if MAC {
            let id = self.wire.action(
                "connect",
                json!({"provider":c.provider.to_ascii_lowercase(),"path":c.path,"alias":c.alias}),
            );
            c.pending = Some((id, Instant::now()));
        } else {
            if c.provider == "Claude" {
                self.wire
                    .action("claude", json!({"path":c.path,"consent":true}));
            } else {
                self.wire
                    .action("import", json!({"provider":c.provider,"path":c.path}));
            }
            return true;
        }
        false
    }
}
impl Connect {
    #[cfg(target_os = "macos")]
    fn connection_help(&mut self, ui: &mut Ui, browser: bool, p: Palette) {
        Frame::new().inner_margin(15).corner_radius(10)
            .fill(p.card.lerp_to_gamma(p.ink, 0.035))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 10.;
                let note = |ui: &mut Ui, text: &str, color: Color32| {
                    ui.label(rich(text, 12., false, color).line_height(Some(18.)));
                };
                if browser {
                    note(ui, "ChatGPT 계정으로 로그인하세요. 인증 정보는 이 Mac의 사용자 전용 파일에 저장하고, 만료 시 자동 갱신합니다.", p.ink);
                    note(ui, "키체인 승인창은 사용하지 않습니다. 로그인 파일은 별도 암호화 없이 파일 접근 권한으로 보호됩니다.", p.muted);
                    note(ui, "각 계정을 별도로 연결할 수 있습니다. 기존 Codex CLI의 로그인 상태는 그대로 유지됩니다.", p.muted);
                    if self.replacing.is_some() {
                        note(ui, "선택한 카드와 같은 계정·워크스페이스로 로그인하세요. 연결이 완료되면 자동 갱신으로 전환됩니다.", p.muted);
                    }
                    if self.login.is_some() {
                        ui.horizontal(|ui| {
                            ui.add(Spinner::new().size(13.));
                            note(ui, if self.login_url.is_some() { "브라우저에서 로그인을 완료하세요." } else { "로그인을 시작하고 있습니다…" }, p.ink);
                        });
                        if let Some(url) = &self.login_url {
                            ui.hyperlink_to("로그인 페이지 다시 열기", url);
                        }
                    }
                } else if self.provider == "Claude" {
                    note(ui, "공식 CLI의 statusLine에서 5시간·주간 한도를 받습니다. 연결 후 Claude에서 메시지를 보내면 표시됩니다. 계정마다 별도 설정 폴더를 사용하세요.", p.ink);
                    checkbox(ui, &mut self.consent, "이 폴더의 statusLine 연결 허용 (기존 명령은 보존)", 12., p);
                } else {
                    note(ui, "로그인은 이 Mac의 사용자 전용 파일에 저장합니다. 파일 연결은 CLI가 갱신한 인증을 읽습니다. Codex 자동 갱신은 브라우저 로그인에서 사용할 수 있습니다.", p.ink);
                    if self.provider == "Gemini" {
                        note(ui, "Code Assist Standard · Enterprise 전용. 개인용 Google 로그인은 2026년 6월 18일에 종료되었습니다.", p.warning);
                        ui.hyperlink_to("Google 지원 범위 확인", "https://developers.google.com/gemini-code-assist/docs/deprecations/code-assist-individuals");
                    }
                    note(ui, if self.provider == "Codex" { "먼저 터미널에서 codex login을 실행하세요." } else { "먼저 Gemini CLI에서 지원되는 계정으로 로그인하세요." }, p.muted);
                }
            });
    }

    fn logging(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.login.is_some()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
}
