use crate::{Dashboard, dialogs::Dialog, model::*, style::*};
use eframe::egui::*;
use serde_json::{Value, json};

#[cfg(target_os = "macos")]
use crate::mac_text::{PreferredFont, preferred_label};
#[cfg(windows)]
use crate::account_text::AccountField;
use crate::account_text::{contains_query, email_matches_alias};

fn provider_filter(ui: &mut Ui, filter: &mut String, width: f32) {
    let providers = ["전체 Provider", "Codex", "Claude", "Gemini"];
    let selected = providers
        .iter()
        .position(|s| *s == filter.as_str())
        .unwrap_or(0);
    let label = if MAC { "Provider" } else { "Provider 필터" };
    if let Some(index) = choose(ui, label, width, &providers, selected, 12.) {
        *filter = providers[index].into();
    }
}

impl Dashboard {
    pub fn accounts(&mut self, ui: &mut Ui, p: Palette) {
        let width = ui.available_width();
        #[cfg(windows)]
        let filters = |app: &Dashboard| Id::new((&app.query, &app.filter, app.attention));
        #[cfg(windows)]
        let previous_filters = filters(self);
        let matches = |a: &Account, filter: &str, query: &str, attention: bool| {
            (filter == "전체 Provider" || filter == a.provider)
                && (!attention || a.attention())
                && (query.is_empty()
                    || contains_query(
                        &format!(
                            "{} {} {}",
                            a.name,
                            a.provider,
                            text(&a.snapshot, "email", "")
                        ),
                        query,
                    ))
        };
        ui.horizontal(|ui| {
            if MAC {
                ui.spacing_mut().item_spacing.x = 12.;
            }
            let w = (width - 300.).clamp(64., 280.);
            #[cfg(target_os = "macos")]
            {
                if self.capture.is_some() {
                    self.account_search = None;
                } else if self.account_search.is_none() && let Some(host) = &self.selectable_text {
                    self.account_search = Some(crate::mac::AccountSearch::new(host.content_view(), ui.ctx()));
                }
                crate::mac::AccountSearch::show(
                    self.account_search.as_mut(), ui, &mut self.query, w,
                    std::mem::take(&mut self.focus_query), p,
                );
            }
            #[cfg(windows)]
            let field = {
                // The WPF editor and its template each apply the seven-DIP
                // vertical padding, inside a one-DIP border on either side.
                let height = crate::windows_text::TextStack::line_height(12.).unwrap_or(16.) + 30.;
                ui.add(
                    TextEdit::singleline(&mut AccountField(&mut self.query))
                        .hint_text(rich("계정 이름 또는 이메일 검색", 11., false, p.muted))
                        .desired_width((w - 22.).max(0.))
                        .min_size(vec2(w, height))
                        .vertical_align(Align::Center)
                        .margin(Margin::symmetric(11, 8)),
                )
            };
            #[cfg(windows)]
            crate::accessibility::text_value(
                ui,
                tooltip_text(field, "계정 이름 또는 이메일 검색"),
                &mut self.query,
                "계정 이름 또는 이메일 검색",
            );
            if MAC {
                provider_filter(ui, &mut self.filter, 150.);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 12.;
                let count = self
                    .state
                    .accounts
                    .iter()
                    .filter(|a| matches(a, &self.filter, &self.query, self.attention))
                    .count();
                caption(ui, format!("{count} 계정"), p);
                checkbox(ui, &mut self.attention, "연결 확인 필요", 12., p);
                if !MAC {
                    provider_filter(ui, &mut self.filter, 128.);
                }
            });
        });
        #[cfg(windows)]
        if filters(self) != previous_filters {
            reset_disclosures(ui.ctx());
        }
        ui.add_space(18.);
        let rows: Vec<_> = self
            .state
            .accounts
            .iter()
            .filter(|a| matches(a, &self.filter, &self.query, self.attention))
            .cloned()
            .collect();
        if rows.is_empty() {
            ui.add_space(60.);
            ui.vertical_centered(|ui| {
                let empty = self.state.accounts.is_empty();
                #[cfg(target_os = "macos")]
                {
                    let (bounds, _) = ui.allocate_exact_size(vec2(56., 44.), Sense::hover());
                    crate::mac::symbol(ui.painter(), bounds, "person.2", 36., false, p.muted);
                    ui.add_space(8.);
                }
                label(
                    ui,
                    if empty {
                        "연결된 계정이 없습니다"
                    } else {
                        "조건에 맞는 계정이 없습니다"
                    },
                    18.,
                    true,
                    p.ink,
                );
                ui.add_space(10.);
                caption(
                    ui,
                    if empty {
                        "오른쪽 위의 계정 연결에서 Codex, Claude, Gemini를 추가하세요."
                    } else {
                        "다른 검색어나 Provider를 선택하세요."
                    },
                    p,
                );
                if !empty {
                    ui.add_space(16.);
                    if action(ui, "필터 초기화", false, p).clicked() {
                        self.query.clear();
                        self.filter = "전체 Provider".into();
                        self.attention = false;
                    }
                }
            });
        }
        #[cfg(target_os = "macos")]
        let scroll = {
            let scroll = ScrollArea::vertical()
                .id_salt("account cards")
                .auto_shrink([false, true]);
            if std::mem::take(&mut self.scroll_top) {
                scroll.vertical_scroll_offset(0.)
            } else {
                scroll
            }
        };
        #[cfg(target_os = "macos")]
        let rail = self.scroller.clone();
        #[cfg(target_os = "macos")]
        let capturing = self.capture.is_some();
        let cards = |ui: &mut Ui| {
            for account in rows {
                ui.push_id(&account.id, |ui| {
                    p.frame(20)
                        .show(ui, |ui| self.connection_card(ui, p, &account))
                });
                ui.add_space(14.);
            }
        };
        #[cfg(target_os = "macos")]
        mac_scroll(ui, scroll, p, rail.as_deref(), capturing, cards);
        #[cfg(windows)]
        cards(ui);
    }
    #[cfg(target_os = "macos")]
    fn native_selectable_text(&mut self, ui: &Ui) -> Option<&mut crate::mac::SelectableText> {
        self.selectable_text
            .as_mut()
            .filter(|_| self.capture.is_none() && !Popup::is_any_open(ui.ctx()))
    }

    fn connection_card(&mut self, ui: &mut Ui, p: Palette, a: &Account) {
        ui.set_width(ui.available_width());
        let tint = p.tint(a.provider);
        let status = if !a.enabled {
            "일시 정지"
        } else if a.loading {
            "확인 중"
        } else if a.attention() {
            "확인 필요"
        } else if a.snapshot.is_null() {
            "조회 대기"
        } else {
            "연결됨"
        };
        let color = if MAC && a.loading {
            p.muted
        } else if a.enabled && a.attention() {
            p.warning
        } else if !a.enabled || a.snapshot.is_null() {
            p.muted
        } else {
            tint
        };
        #[cfg(windows)]
        let header_id = ui.id().with("account header height");
        #[cfg(windows)]
        let labels_height = ui.data(|d| d.get_temp::<f32>(header_id)).unwrap_or(36.);
        #[cfg(target_os = "macos")]
        let labels_height = 36.;
        let header = |ui: &mut Ui| {
            ui.spacing_mut().item_spacing.x = 12.;
            self.assets.logo(ui, a.provider, 36.);
            #[cfg(target_os = "macos")]
            let width = (ui.available_width() - 112.).max(48.);
            #[cfg(windows)]
            let status_text = format!("●  {status}");
            #[cfg(windows)]
            let width = {
                let status_width = crate::windows_text::natural_size(ui, &status_text, 11., false)
                    .unwrap_or_else(|| {
                        ui.painter()
                            .layout_no_wrap(status_text.clone(), FontId::proportional(11.), color)
                            .size()
                    })
                    .x;
                ui.spacing_mut().item_spacing.x = 0.;
                (ui.available_width() - status_width).max(0.)
            };
            let size = vec2(width, labels_height);
            let labels = ui.allocate_ui_with_layout(size, Layout::top_down(Align::Min), |ui| {
                ui.set_max_width(width);
                let email = a.snapshot["email"]
                    .as_str()
                    .filter(|s| !email_matches_alias(s, &a.name));
                #[cfg(target_os = "macos")]
                {
                    if email.is_none() {
                        let stack = crate::mac_text::title_stack_height(ui, 13., 12., 4.);
                        ui.add_space(((36. - stack) / 2.).max(0.));
                    }
                    crate::mac_text::stacked_label(ui, a.provider, 13., true, p.ink)
                        .unwrap_or_else(|| label(ui, a.provider, 13., true, p.ink));
                }
                #[cfg(windows)]
                crate::windows_text::TextStack::inline(ui).label(ui, a.provider, 13., true, p.ink);
                ui.add_space(4.);
                #[cfg(windows)]
                ui.scope(|ui| {
                    ui.set_max_width((width - 16.).max(0.));
                    ellipsis_label(ui, &a.name, 12., p.ink);
                });
                #[cfg(target_os = "macos")]
                {
                    if let Some(fields) = self.native_selectable_text(ui) {
                        fields.show(ui, crate::mac::TextRole::AccountName, &a.name);
                        if let Some(email) = email {
                            ui.add_space(4.);
                            fields.show(ui, crate::mac::TextRole::AccountEmail, email);
                        }
                    } else {
                        ellipsis_label(ui, &a.name, 12., p.ink);
                        if let Some(email) = email {
                            ui.add_space(4.);
                            ui.add(Label::new(rich(email, 10., false, p.muted)).selectable(true));
                        }
                    }
                }
                #[cfg(windows)]
                if let Some(email) = email {
                    caption(ui, email, p);
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                #[cfg(target_os = "macos")]
                {
                    let (bounds, button) = ui.allocate_exact_size(vec2(32., 20.), Sense::click());
                    button.widget_info(|| {
                        WidgetInfo::labeled(
                            WidgetType::Button,
                            ui.is_enabled(),
                            format!("{} 계정 메뉴", a.name),
                        )
                    });
                    let icon = |x, width| {
                        Rect::from_center_size(
                            pos2(bounds.left() + x, bounds.center().y),
                            vec2(width, 16.),
                        )
                    };
                    crate::mac::symbol(ui.painter(), icon(11., 14.), "ellipsis", 13., true, tint);
                    crate::mac::symbol(ui.painter(), icon(24., 9.), "chevron.down", 9., true, tint);
                    if button.has_focus() {
                        ui.painter().rect_stroke(
                            bounds.expand(1.),
                            4.,
                            Stroke::new(2.0_f32, tint),
                            StrokeKind::Outside,
                        );
                    }
                    if button.clicked() {
                        use quotabar::mac_menu::{AccountMenuAction, account_menu};
                        let zoom = f64::from(ui.ctx().zoom_factor());
                        match account_menu(
                            f64::from(bounds.left()) * zoom,
                            f64::from(bounds.bottom()) * zoom,
                            a.enabled,
                        ) {
                            Some(AccountMenuAction::Refresh) => {
                                self.wire.action("refresh-account", json!(a.id));
                            }
                            Some(AccountMenuAction::Remove) => {
                                self.dialog = Some(Dialog::remove(a.clone()))
                            }
                            None => {}
                        }
                    }
                    ui.spacing_mut().item_spacing.x = 6.;
                    label(ui, status, 11., false, color);
                    if a.loading {
                        ui.add(Spinner::new().size(10.));
                    } else {
                        let (dot, _) = ui.allocate_exact_size(Vec2::splat(6.), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 3., color);
                    }
                }
                #[cfg(windows)]
                {
                    label(ui, status_text, 11., false, color);
                }
            });
            labels.response.rect.height().max(36.)
        };
        #[cfg(target_os = "macos")]
        ui.horizontal(header);
        #[cfg(windows)]
        {
            // WPF's DockPanel measures the label stack before centering its
            // 36-DIP logo. The stack can grow again when an email wraps.
            let size = vec2(ui.available_width(), labels_height);
            let measured = ui
                .allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), header)
                .inner;
            if measured != labels_height {
                ui.data_mut(|d| d.insert_temp(header_id, measured));
                ui.ctx()
                    .request_discard("Center the provider logo in the measured account row");
            }
        }
        if let Some(message) = a.connection_message() {
            ui.add_space(16.);
            label(ui, message, 11., false, p.warning);
        }
        if let Some(note) = &a.note {
            ui.add_space(8.);
            caption(ui, note, p);
        }
        ui.add_space(if MAC { 16. } else { 18. });
        ui.scope(|ui| {
            if !MAC {
                ui.spacing_mut().interact_size.y = 16.;
            }
            ui.horizontal(|ui| {
                let connection = if a.managed {
                    "앱 로그인 · 자동 갱신"
                } else if a.provider == "Claude" {
                    "Claude CLI 연결"
                } else {
                    "로그인 파일 연결"
                };
                #[cfg(target_os = "macos")]
                {
                    let (icon, _) = ui.allocate_exact_size(Vec2::splat(11.), Sense::hover());
                    let name = if a.managed {
                        "arrow.triangle.2.circlepath"
                    } else if a.provider == "Claude" {
                        "folder"
                    } else {
                        "doc"
                    };
                    crate::mac::symbol(ui.painter(), icon, name, 11., false, p.muted);
                }
                system_label(ui, connection, if MAC { 10. } else { 11. }, false, p.muted);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    #[cfg(target_os = "macos")]
                    let toggle_size = {
                        ui.spacing_mut().item_spacing.x = 16.;
                        vec2(mini_toggle_width(ui, "모니터링"), 15.)
                    };
                    #[cfg(windows)]
                    let toggle_size = switch_natural_size(ui, "모니터링");
                    ui.allocate_ui_with_layout(toggle_size, Layout::top_down(Align::Min), |ui| {
                        let switch = named_toggle(
                            ui,
                            "모니터링",
                            &a.control_label("모니터링"),
                            a.enabled,
                            tint,
                            p,
                        );
                        #[cfg(target_os = "macos")]
                        let switch = tooltip_text(switch, "끄면 이 계정의 조회를 일시 정지합니다.");
                        if switch.clicked() {
                            self.edit_account(a, &a.name, !a.enabled, a.tray);
                        }
                    });
                    #[cfg(target_os = "macos")]
                    if a.provider == "Codex" {
                        let login = if a.managed {
                            "다시 로그인…"
                        } else {
                            "앱에서 로그인…"
                        };
                        if ui
                            .add_enabled_ui(!self.state.demo, |ui| small_button(ui, login, p))
                            .inner
                            .clicked()
                        {
                            self.open_connect("Codex", Some(a));
                        }
                    }
                });
            });
        });
        if !a.path.is_empty() {
            ui.add_space(if MAC { 15. } else { 14. });
            let title = if a.provider == "Claude" {
                "연결한 설정 폴더"
            } else {
                "연결한 로그인 파일"
            };
            let size = if MAC { 10. } else { 11. };
            disclosure(ui, (&a.id, "source"), title, size, p, |ui| {
                #[cfg(target_os = "macos")]
                ui.add_space(8.);
                #[cfg(target_os = "macos")]
                if let Some(fields) = self.native_selectable_text(ui) {
                    fields.show(ui, crate::mac::TextRole::SourcePath, &a.path);
                } else {
                    ui.add(
                        Label::new(RichText::new(&a.path).monospace().size(11.).color(p.muted))
                            .selectable(true),
                    );
                }
                #[cfg(windows)]
                caption(ui, &a.path, p);
                let detail = match (MAC, a.provider) {
                    (true, "Claude") => {
                        "Claude에서 메시지를 보내면 잔여량이 갱신됩니다. 계정 삭제 시 기존 statusLine 명령을 복구합니다."
                    }
                    (false, "Claude") => {
                        "Claude에서 메시지를 보내면 잔여량이 갱신됩니다. 연결 해제 시 기존 status line을 복구합니다."
                    }
                    _ => {
                        "CLI가 갱신한 로그인 파일을 읽습니다. 로그인 만료 시 CLI에서 다시 로그인하세요."
                    }
                };
                #[cfg(target_os = "macos")]
                {
                    ui.add_space(8.);
                    system_label(ui, detail, 10., false, p.muted);
                }
                #[cfg(windows)]
                caption(ui, detail, p);
            });
        }
        if !MAC {
            ui.add_space(16.);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled_ui(a.enabled && !a.loading, |ui| {
                        action(ui, "새로고침", false, p)
                    })
                    .inner
                    .clicked()
                {
                    self.wire.action("refresh-account", json!(a.id));
                }
                if action(ui, "이름 변경", false, p).clicked() {
                    self.dialog = Some(Dialog::Rename(a.clone(), a.name.clone()));
                }
                if action(ui, "연결 해제", false, p).clicked() {
                    self.dialog = Some(Dialog::remove(a.clone()));
                }
            });
        }
    }
    fn preference(&mut self, key: &str, mac: &str, value: Value) {
        if MAC {
            // The monitor publishes the saved state back to AppKit. Applying it
            // here would hide the appearance change from the incoming-state handler.
            self.wire.action(mac, value);
            return;
        }
        self.state.raw["settings"][key] = value;
        let s = &self.state.raw["settings"];
        self.wire.action("preferences",json!({"seconds":s["refreshSeconds"].as_i64().unwrap_or(300),"autoRefresh":s["autoRefresh"].as_bool().unwrap_or(true),
            "theme":text(s,"appearance","system"),"scan":s["scanLocalLogs"].as_bool().unwrap_or(true),"taskbar":s["showTaskbarIcon"].as_bool().unwrap_or(true),
            "trayDisplay":text(s,"trayDisplayMode","remaining"),"automaticUpdates":s["automaticUpdates"].as_bool().unwrap_or(true)}));
    }
    fn preview(&self, p: Palette, ui: &mut Ui) {
        let native = quotabar::tray::State {
            accounts: self
                .state
                .accounts
                .iter()
                .map(|a| quotabar::tray::Account {
                    provider: a.provider.into(),
                    name: a.name.clone(),
                    enabled: a.enabled,
                    visible: a.tray,
                    remaining: a
                        .windows()
                        .iter()
                        .filter_map(|w| w["remaining"].as_f64())
                        .collect(),
                    error: a.error.is_some(),
                    state: a.snapshot["state"].as_str().map(str::to_owned),
                })
                .collect(),
            icon: self.state.setting("trayDisplayMode", "remaining") == "icon",
            demo: self.state.demo,
        };
        let summary = if MAC {
            serde_json::to_value(quotabar::tray::mac(&native))
        } else {
            serde_json::to_value(quotabar::tray::windows(&native))
        }
        .unwrap_or_default();
        #[cfg(target_os = "macos")]
        {
            let mode = self
                .state
                .setting("menuBarDisplayMode", "providerAndRemaining");
            let value = text(
                &summary,
                if mode == "remainingOnly" {
                    "remaining"
                } else {
                    "title"
                },
                "",
            );
            let menu_height = if mode == "iconOnly" {
                14.
            } else {
                crate::mac_text::system_line_height(
                    ui,
                    objc2_app_kit::NSFont::menuBarFontOfSize(0.).pointSize(),
                )
            };
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), menu_height + 14.),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 12.;
                    label(ui, "미리보기", 10., false, p.muted);
                    Frame::new()
                        .fill(p.bg)
                        .inner_margin(Margin::symmetric(12, 7))
                        .corner_radius(7)
                        .show(ui, |ui| crate::mac::menu_preview(ui, mode, value, p.ink));
                },
            );
        }
        #[cfg(windows)]
        Frame::new()
            .fill(p.bg)
            .inner_margin(Margin {
                left: 12,
                right: 22,
                top: 8,
                bottom: 8,
            })
            .corner_radius(7)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.;
                    if summary["useLogo"] == true {
                        mark(ui, 22., p);
                        caption(ui, "QuotaBar", p);
                    } else {
                        let value = text(&summary, "text", "—");
                        label(
                            ui,
                            value,
                            22.,
                            true,
                            if summary["warning"] == true {
                                p.warning
                            } else {
                                p.ink
                            },
                        );
                        caption(
                            ui,
                            if matches!(value, "!" | "—") {
                                "조회 대기 · 연결 상태 확인".into()
                            } else {
                                format!("최소 잔여 {value}%")
                            },
                            p,
                        );
                    }
                })
            });
    }
    pub fn settings(&mut self, ui: &mut Ui, p: Palette) {
        // PreferencesView uses the same 18-point VStack spacing on both sides of each rule.
        let section_divider = |ui: &mut Ui| {
            if MAC {
                let contrast = if p.dark {
                    Color32::WHITE
                } else {
                    Color32::BLACK
                };
                spaced_divider(ui, p.card.lerp_to_gamma(contrast, 0.10), 18., 18.);
            } else {
                divider(ui, p);
            }
        };
        p.frame(20).show(ui,|ui|{ui.set_width(ui.available_width());settings_title(ui,"화면 및 표시","menubar.dock.rectangle",p);ui.add_space(18.);
            label(ui,"화면 모드",if MAC {13.} else {12.},false,p.ink);ui.add_space(if MAC {18.} else {12.});
            if let Some(v)=segments(ui,&[("system","시스템 설정 따르기"),("light","라이트"),("dark","다크")],self.state.setting("appearance","system"),p,420.){
                self.preference("appearance","appearance",json!(v));}
            section_divider(ui);let dock=self.state.flag("showTaskbarIcon","showDockIcon");
            #[cfg(target_os = "macos")]
            let dock_toggle = mac_setting_toggle(ui,"Dock에 앱 아이콘 표시",Some("끄면 상단바에서 Dashboard와 설정을 열 수 있습니다."),dock,p.accent,p);
            #[cfg(windows)]
            let dock_toggle = toggle(ui,"작업 표시줄에 앱 아이콘 표시",dock,p.accent,p);
            if dock_toggle.clicked(){
                self.preference(if MAC{"showDockIcon"}else{"showTaskbarIcon"},"dockIcon",json!(!dock));}
            #[cfg(windows)]
            {ui.add_space(6.);caption(ui,"끄면 트레이에서 Dashboard와 설정을 열 수 있습니다.",p);}
            section_divider(ui);label(ui,if MAC{"상단바 표시 형식"}else{"트레이 표시"},if MAC {13.} else {12.},false,p.ink);ui.add_space(12.);
            let (key,modes):(&str,&[(&str,&str)])=if MAC{("menuBarDisplayMode",&[("providerAndRemaining","이름 + 잔여량"),("remainingOnly","잔여량만"),("iconOnly","아이콘만")])}
                else{("trayDisplayMode",&[("remaining","남은 비율 숫자"),("icon","로고만")])};
            if let Some(v)=segments(ui,modes,self.state.setting(key,modes[0].0),p,420.){self.preference(key,"menuBarMode",json!(v));}
            ui.add_space(if MAC {12.} else {24.});self.preview(p,ui);ui.add_space(12.);
            #[cfg(target_os = "macos")]
            crate::mac_text::preferred_label(ui, "잔여량은 Codex · Claude · Gemini 순서로 표시하며, 등록한 계정만 포함합니다. 표시할 계정은 Dashboard에서 선택하세요.", crate::mac_text::PreferredFont::Caption, p.muted);
            #[cfg(windows)]
            {caption(ui,"숫자는 표시 대상 계정·기간 중 가장 낮은 잔여 비율(%)입니다. 작은 아이콘에서는 % 기호를 생략합니다.",p);ui.add_space(6.);
                caption(ui,"Dashboard에서 표시할 계정을 선택하세요. 마우스를 올리거나 클릭하면 계정별 내역을 확인할 수 있습니다.",p);}
        });
        ui.add_space(20.);
        p.frame(20).show(ui,|ui|{ui.set_width(ui.available_width());settings_title(ui,"모니터링","arrow.triangle.2.circlepath",p);ui.add_space(18.);
            let auto=self.state.flag("autoRefresh","autoRefresh");let seconds=self.state.raw["settings"]["refreshSeconds"].as_i64().unwrap_or(300);
            ui.horizontal(|ui|{
                #[cfg(windows)]
                let w=(ui.available_width()-146.).max(120.);
                #[cfg(target_os = "macos")]
                let w=crate::mac_text::natural_size(ui,"자동 새로고침",13.,false).map_or(74., |size| size.x.ceil()) + 46.;
                ui.spacing_mut().item_spacing.x = if MAC {12.} else {16.};
                let layout = if MAC { Layout::top_down(Align::Min) } else { Layout::left_to_right(Align::Center) };
                ui.allocate_ui_with_layout(vec2(w,if MAC {22.} else {31.}),layout,|ui|{
                    if toggle(ui,"자동 새로고침",auto,p.accent,p).clicked(){self.preference("autoRefresh","autoRefresh",json!(!auto));}
                });
                ui.add_enabled_ui(auto, |ui| {
                    let intervals = quotabar::schedule::INTERVALS;
                    let labels = intervals.map(|n| format!("{}분", n / 60));
                    let selected = intervals.iter().position(|&n| i64::from(n) == seconds).unwrap_or(0);
                    if let Some(index) = choose(ui, "자동 새로고침 간격", 130., &labels, selected, 12.) {
                        self.preference("refreshSeconds", "interval", json!(intervals[index]));
                    }
                });
            });ui.add_space(if MAC {18.} else {10.});
            let help = "창을 닫아도 선택한 주기로 조회합니다. 꺼도 앱 시작 시 조회와 수동 새로고침은 사용할 수 있습니다.";
            #[cfg(target_os = "macos")]
            crate::mac_text::preferred_label(ui, help, crate::mac_text::PreferredFont::Caption, p.muted);
            #[cfg(windows)]
            caption(ui, help, p);
            section_divider(ui);let scan=self.state.flag("scanLocalLogs","scanLocalLogs");
            let scan_detail="Dashboard가 열려 있을 때만 로그를 확인합니다. 대화 본문은 저장하지 않습니다.";
            #[cfg(target_os = "macos")]
            let scan_toggle=mac_setting_toggle(ui,"로컬 토큰 통계",Some(scan_detail),scan,p.accent,p);
            #[cfg(windows)]
            let scan_toggle=toggle(ui,"로컬 토큰 통계",scan,p.accent,p);
            if scan_toggle.clicked(){self.preference("scanLocalLogs","scanLogs",json!(!scan));}
            #[cfg(windows)]
            {ui.add_space(6.);caption(ui,scan_detail,p);}
        });
        ui.add_space(20.);
        self.app_settings(ui, p);
        ui.add_space(20.);
        let button_card_padding = Margin {
            bottom: if MAC { 19 } else { 20 },
            ..Margin::same(20)
        };
        p.frame(button_card_padding).show(ui, |ui| {
            ui.set_width(ui.available_width());
            settings_title(ui, "데이터 저장", "lock.shield", p);
            #[cfg(windows)]
            {
                ui.add_space(14.);
                caption(ui, "연결 정보는 현재 Windows 사용자만 열 수 있도록 보호됩니다.", p);
                if let Some(dir) = self.state.raw["dataDirectory"].as_str() {
                    ui.add_space(6.);
                    caption(ui, format!("데이터 위치: {dir}"), p);
                }
            }
            #[cfg(target_os = "macos")]
            {
                ui.add_space(12.);
                let notice = "Codex · Gemini 로그인은 이 Mac의 사용자 전용 파일에 저장됩니다. 키체인 승인창을 사용하지 않으며, 별도 암호화 대신 파일 접근 권한으로 보호됩니다.";
                if crate::mac_text::paragraph(ui, notice, 12., 0., p.ink).is_none() {
                    label(ui, notice, 12., false, p.ink);
                }
                ui.add_space(12.);
                system_label(ui, "설정 위치", 10., false, p.muted);
                ui.add_space(13.);
                let path = crate::mac::data_path();
                let path = path.to_string_lossy();
                if let Some(fields) = self.native_selectable_text(ui) {
                    fields.show(ui, crate::mac::TextRole::DataPath, &path);
                } else {
                    ui.add(Label::new(RichText::new(path).monospace().size(11.)).selectable(true));
                }
                ui.add_space(12.);
                if ui.add_enabled_ui(!self.state.demo, |ui| {
                    crate::mac::standard_button(ui, "데이터 폴더 열기", None,
                        objc2_app_kit::NSControlSize::Regular, Some(12.), p.focus_ring)
                        .unwrap_or_else(|| action(ui, "데이터 폴더 열기", false, p))
                }).inner.clicked() {
                    crate::mac::open_data();
                }
            }
        });
        ui.add_space(20.);
        p.frame(button_card_padding).show(ui, |ui| {
            ui.set_width(ui.available_width());
            settings_title(ui, "오픈소스 라이선스", "doc.text", p);
            ui.add_space(if MAC { 12. } else { 14. });
            let credit = "AIUsage · sylearn 및 기여자";
            #[cfg(target_os = "macos")]
            preferred_label(ui, credit, PreferredFont::CalloutBold, p.ink);
            #[cfg(not(target_os = "macos"))]
            label(ui, credit, 12., true, p.ink);
            ui.add_space(if MAC { 12. } else { 6. });
            system_label(ui, "Apache License 2.0", if MAC { 10. } else { 11. }, false, p.muted);
            ui.add_space(if MAC { 13. } else { 12. });
            label(ui, "QuotaBar는 AIUsage의 사용량 조회 코드와 Claude 연결 기능 등을 재사용·수정한 독립 프로젝트입니다.", 12., false, p.ink);
            ui.add_space(if MAC { 12. } else { 14. });
            ui.horizontal(|ui| {
                if MAC { ui.spacing_mut().item_spacing.x = 16.; }
                if sheet_button(ui, "라이선스 및 고지 보기", p).clicked() {
                    self.dialog = Some(Dialog::licenses());
                }
                let url = "https://github.com/sylearn/AIUsage";
                #[cfg(target_os = "macos")]
                crate::mac_text::hyperlink(ui, "원본 프로젝트", url, 13., p.accent, false);
                #[cfg(windows)]
                ui.hyperlink_to("원본 프로젝트", url);
            });
        });
    }
    fn app_settings(&mut self, ui: &mut Ui, p: Palette) {
        let app_caption = |ui: &mut Ui, text: &str| {
            #[cfg(target_os = "macos")]
            preferred_label(ui, text, PreferredFont::Caption, p.muted);
            #[cfg(windows)]
            caption(ui, text, p);
        };
        #[cfg(target_os = "macos")]
        let app_divider = |ui: &mut Ui| {
            let contrast = if p.dark { Color32::WHITE } else { Color32::BLACK };
            spaced_divider(ui, p.card.lerp_to_gamma(contrast, 0.10), 18., 18.);
        };
        p.frame(20).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                mark(ui, 24., p);
                #[cfg(target_os = "macos")]
                preferred_label(ui, "QuotaBar", PreferredFont::Headline, p.ink);
                #[cfg(windows)]
                label(ui, "QuotaBar", 14., true, p.ink);
                let prefix = if MAC { "" } else { "Windows · " };
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    app_caption(ui, &format!("{prefix}버전 {}", env!("QUOTABAR_VERSION")));
                });
            });
            ui.add_space(18.);
            let startup = text(&self.state.raw, "launchAtLogin", "disabled");
            let enabled = matches!(startup, "enabled" | "requiresApproval");
            if ui.add_enabled_ui(startup != "unavailable", |ui| {
                toggle(ui, "로그인 시 자동 실행", enabled, p.accent, p)
            }).inner.clicked() {
                self.wire.action("startup", json!(!enabled));
            }
            ui.add_space(if MAC { 14. } else { 6. });
            app_caption(ui, if MAC { "Mac 재시동 후 로그인하면 Dashboard를 열지 않고 상단바에서 시작합니다." }
                            else { "Windows 재시작 후 로그인하면 Dashboard를 열지 않고 트레이에서 시작합니다." });
            if let Some(note) = match (MAC, startup) {
                (true, "requiresApproval") => Some("macOS 로그인 항목 설정에서 QuotaBar의 실행을 허용해 주세요."),
                (true, "unavailable") => Some("빌드된 QuotaBar.app에서 자동 실행을 설정할 수 있습니다."),
                (true, _) => None,
                (false, "unavailable") => Some("설치된 QuotaBar 앱에서 자동 실행을 설정하세요."),
                (false, _) => Some("Windows의 시작 앱 설정에서도 QuotaBar를 허용해야 합니다. 앱을 옮겼다면 이 옵션을 다시 켜세요."),
            } {
                ui.add_space(if MAC { 14. } else { 6. });
                app_caption(ui, note);
            }
            #[cfg(target_os = "macos")]
            {
                if startup == "requiresApproval" {
                    ui.add_space(14.);
                    if sheet_button(ui, "로그인 항목 설정 열기", p).clicked() {
                        self.wire.action("startup-settings", Value::Null);
                    }
                }
                app_divider(ui);
            }
            #[cfg(windows)]
            {
                if link(ui, "Windows 시작 앱 설정 열기", p, p.card).clicked() {
                    self.wire.action("startup-settings", Value::Null);
                }
                divider(ui, p);
            }
            settings_title(ui, "앱 업데이트", "arrow.triangle.2.circlepath", p);
            ui.add_space(if MAC { 14. } else { 18. });
            let automatic = self.state.flag("automaticUpdates", "automaticUpdates");
            if toggle(ui, "자동 업데이트", automatic, p.accent, p).clicked() {
                self.preference("automaticUpdates", "automaticUpdates", json!(!automatic));
            }
            ui.add_space(if MAC { 14. } else { 6. });
            app_caption(ui, if MAC { "새 정식 버전을 자동으로 내려받고, Dashboard가 닫혀 있으면 설치 후 다시 시작합니다." }
                            else { "새 버전을 자동으로 내려받고, Dashboard를 닫으면 설치 후 다시 시작합니다." });
            if MAC { ui.add_space(14.); } else { divider(ui, p); }
            let update = &self.state.raw["appUpdate"];
            let phase = text(update, "phase", "idle");
            let version = text(update, "version", "");
            let progress = count(update, "progress");
            let busy = matches!(phase, "checking" | "downloading" | "installing");
            let status = update["message"].as_str().filter(|s| !s.trim().is_empty()).map(str::to_owned)
                .unwrap_or_else(|| match phase {
                    "checking" if MAC => "새 버전을 확인하고 있습니다…".into(),
                    "checking" => "새 버전 확인 중…".into(),
                    "current" => "최신 버전입니다.".into(),
                    "available" if MAC => format!("새 버전 {version}을 사용할 수 있습니다."),
                    "available" => format!("버전 {version} 업데이트가 있습니다."),
                    "downloading" if MAC => format!("{version} 다운로드 중 · {progress}%"),
                    "downloading" => format!("버전 {version} 다운로드 중 · {progress}%"),
                    "ready" if automatic => format!("Dashboard를 닫으면 {version}을 설치하고 다시 시작합니다."),
                    "ready" if MAC => format!("{version} 설치 준비가 끝났습니다."),
                    "ready" => format!("버전 {version} 설치 준비 완료"),
                    "installing" if MAC => "업데이트 설치를 준비하고 있습니다…".into(),
                    "installing" => "설치를 준비하고 있습니다…".into(),
                    "failed" if MAC => "업데이트를 완료하지 못했습니다. 현재 앱을 유지합니다.".into(),
                    "failed" => "업데이트를 완료하지 못했습니다.".into(),
                    _ if automatic => "앱 시작 후와 하루에 한 번 새 정식 버전을 확인합니다.".into(),
                    _ => "아래에서 새 버전을 직접 확인할 수 있습니다.".into(),
                });
            ui.horizontal(|ui| {
                if MAC && busy { ui.spinner(); }
                let ink = if phase == "failed" { p.warning } else { p.ink };
                #[cfg(target_os = "macos")]
                preferred_label(ui, &status, PreferredFont::Callout, ink);
                #[cfg(windows)]
                label(ui, status, 12., false, ink);
            });
            if phase == "downloading" {
                ui.add_space(if MAC { 14. } else { 12. });
                ui.add(ProgressBar::new(progress as f32 / 100.).desired_height(5.));
            }
            ui.add_space(if MAC { 14. } else { 16. });
            ui.horizontal(|ui| {
                if ui.add_enabled_ui(!busy && (MAC || !self.state.demo), |ui| {
                    sheet_button(ui, "업데이트 확인", p)
                }).inner.clicked() {
                    self.wire.action("check-app-update", Value::Null);
                }
                if matches!(phase, "available" | "ready") {
                    let enabled = !self.state.demo && (MAC || self.state.raw["loggingIn"] != true);
                    if ui.add_enabled_ui(enabled, |ui| sheet_button(ui, "설치 후 다시 시작", p)).inner.clicked() {
                        self.wire.action("install-app-update", Value::Null);
                    }
                }
            });
            #[cfg(target_os = "macos")]
            {
                app_divider(ui);
                crate::mac_text::hyperlink(ui, "QuotaBar · GitHub", "https://github.com/zjxps2007/quotabar", 13., p.accent, false);
            }
        });
    }
}
fn settings_title(ui: &mut Ui, text: &str, _symbol: &str, p: Palette) {
    #[cfg(target_os = "macos")]
    ui.horizontal(|ui| {
        crate::mac::intrinsic_symbol(ui, _symbol, 13., true, p.ink);
        crate::mac_text::preferred_label(ui, text, crate::mac_text::PreferredFont::Headline, p.ink);
    });
    #[cfg(windows)]
    label(ui, text, 14., true, p.ink);
}

#[cfg(target_os = "macos")]
pub fn segments<'a>(
    ui: &mut Ui,
    items: &[(&'a str, &str)],
    current: &str,
    p: Palette,
    maximum_width: f32,
) -> Option<&'a str> {
    let mut change = None;
    let (bounds, _) = ui.allocate_exact_size(
        vec2(ui.available_width().min(maximum_width), 22.),
        Sense::hover(),
    );
    let width = bounds.width() / items.len() as f32;
    let cell_rect = |index: usize| {
        Rect::from_min_size(
            bounds.min + vec2(index as f32 * width, 0.),
            vec2(width, bounds.height()),
        )
    };
    let mut pressed = None;
    let mut focus = None;
    for (index, &(key, title)) in items.iter().enumerate() {
        let cell = cell_rect(index);
        let response = ui.interact(cell, ui.auto_id_with(key), Sense::click());
        response.widget_info(|| {
            WidgetInfo::selected(
                WidgetType::RadioButton,
                ui.is_enabled(),
                key == current,
                title,
            )
        });
        if response.is_pointer_button_down_on() {
            pressed = Some(index);
        }
        if response.has_focus() {
            focus = Some(cell);
        }
        if response.clicked() {
            change = Some(key);
        }
    }
    if !crate::mac::draw_segments(ui, bounds, items, current, pressed) {
        let field = Color32::from_gray(if p.dark { 52 } else { 241 });
        let border = Stroke::new(1.0_f32, p.line);
        ui.painter()
            .rect(bounds, 5., field, border, StrokeKind::Inside);
        for (index, &(key, title)) in items.iter().enumerate() {
            let cell = cell_rect(index);
            if key == current {
                let fill = if p.dark {
                    Color32::from_gray(88)
                } else {
                    p.card
                };
                ui.painter()
                    .rect(cell, 5., fill, border, StrokeKind::Inside);
            }
            ui.painter().text(
                cell.center(),
                Align2::CENTER_CENTER,
                title,
                FontId::proportional(13.),
                p.ink,
            );
        }
    }
    if let Some(cell) = focus {
        ui.painter()
            .rect_stroke(cell, 5., Stroke::new(1.0_f32, p.muted), StrokeKind::Inside);
    }
    change
}

#[cfg(not(target_os = "macos"))]
pub fn segments<'a>(
    ui: &mut Ui,
    items: &[(&'a str, &str)],
    current: &str,
    p: Palette,
    maximum_width: f32,
) -> Option<&'a str> {
    if items.is_empty() {
        return None;
    }
    let font = FontId::proportional(12.);
    let labels: Vec<_> = items
        .iter()
        .map(|&(_, title)| {
            crate::windows_text::natural_size(ui, title, 12., false).unwrap_or_else(|| {
                ui.painter()
                    .layout_no_wrap(title.to_owned(), font.clone(), p.ink)
                    .size()
            })
        })
        .collect();
    // WPF's UniformGrid measures the largest ContentPresenter, then arranges equal
    // cells with a one-DIP border and the Button's horizontal and vertical padding.
    let appearance = items.len() == 3;
    let padding = if appearance { 14. } else { 12. };
    let mut cell = labels
        .iter()
        .fold(Vec2::ZERO, |size, &label| size.max(label))
        + 2. * vec2(padding + 1., 8.);
    let count = items.len() as f32;
    let available = (ui.available_width() - 6.).max(0.);
    let limit = if appearance {
        available.min(maximum_width)
    } else {
        available
    };
    cell.x = cell.x.min(limit / count);
    let (track, _) = ui.allocate_exact_size(vec2(cell.x * count + 6., cell.y + 6.), Sense::hover());
    ui.painter().rect_filled(track, 9, p.track);
    let first = track.min + vec2(3., 3.);
    let mut change = None;
    for (index, (&(key, title), measured)) in items.iter().zip(labels).enumerate() {
        let area = Rect::from_min_size(first + vec2(cell.x * index as f32, 0.), cell);
        if button_feedback(ui, p.track, p.ink, vec2(padding - 2., 5.), |ui| {
            let response = ui.interact(
                area,
                ui.make_persistent_id(("setting-choice", key)),
                Sense::click(),
            );
            if key == current {
                ui.painter().rect(
                    area,
                    8,
                    p.card,
                    Stroke::new(1.0_f32, p.line),
                    StrokeKind::Inside,
                );
            }
            let origin = area.center() - measured * 0.5;
            ui.painter()
                .with_clip_rect(area.intersect(ui.clip_rect()))
                .text(origin, Align2::LEFT_TOP, title, font.clone(), p.ink);
            response
                .widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), title));
            response
        })
        .on_hover_cursor(CursorIcon::PointingHand)
        .clicked()
        {
            change = Some(key);
        }
    }
    change
}
