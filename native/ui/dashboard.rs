use crate::{Dashboard, model::*, style::*};
#[cfg(windows)]
use chrono::Datelike;
use chrono::Local;
use eframe::egui::*;
use serde_json::{Value, json};
use std::collections::HashSet;

impl Dashboard {
    pub fn workspace(&mut self, ctx: &Context, p: Palette, titlebar: i8) {
        if !MAC {
            // Both panels sit on the WPF root grid's background at fractional DPI.
            ctx.layer_painter(LayerId::background())
                .rect_filled(ctx.viewport_rect(), 0., p.bg);
        }
        SidePanel::left("navigation")
            .exact_width(210.)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.rail).inner_margin(Margin {
                left: 18,
                right: if MAC { 18 } else { 19 },
                top: 34 + titlebar,
                bottom: 20,
            }))
            .show(ctx, |ui| {
                let side = ui.max_rect();
                let top = side.top() - 34. - f32::from(titlebar);
                let bottom = side.bottom() + 20.;
                if MAC {
                    ui.painter().vline(
                        side.right() + 17.,
                        top..=bottom,
                        Stroke::new(1.0_f32, p.line),
                    );
                } else {
                    let right = side.right() + 19.;
                    ui.painter().rect_filled(
                        Rect::from_min_max(pos2(right - 1., top), pos2(right, bottom)),
                        0.,
                        p.line,
                    );
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.;
                    mark(ui, 38., p);
                    label(ui, "QuotaBar", 18., true, p.ink);
                });
                ui.add_space(30.);
                ui.scope(|ui| {
                    ui.spacing_mut().interact_size.y = 0.;
                    ui.horizontal(|ui| {
                        ui.add_space(12.);
                        let workspace = rich("WORKSPACE", 9., true, p.muted);
                        ui.label(if MAC {
                            workspace.extra_letter_spacing(1.5)
                        } else {
                            workspace
                        });
                    });
                });
                ui.add_space(12.);
                for (key, title) in [
                    ("dashboard", "Dashboard"),
                    ("accounts", "Providers & Accounts"),
                    ("settings", "설정"),
                ] {
                    let selected = self.route == key;
                    #[cfg(target_os = "macos")]
                    let (display, height) = {
                        let width = ui.available_width() - 68.;
                        let wrap = key == "accounts"
                            && crate::mac_text::natural_size(ui, title, 12., selected)
                                .is_some_and(|size| size.x > width);
                        if wrap {
                            ("Providers\n& Accounts", 52.)
                        } else {
                            (title, 37.)
                        }
                    };
                    #[cfg(windows)]
                    let (display, height) = (title, 38.);
                    let (r, mut res) =
                        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
                    let pressed = button_pressed(ui, &mut res);
                    res.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, title));
                    let opacity = if !MAC && pressed {
                        0.6
                    } else if !MAC && res.hovered() {
                        0.78
                    } else {
                        1.
                    };
                    if selected {
                        ui.painter().rect_filled(
                            r,
                            if MAC { 9 } else { 8 },
                            p.rail.lerp_to_gamma(p.accent, opacity),
                        );
                    }
                    let fg = p
                        .rail
                        .lerp_to_gamma(if selected { p.on } else { p.ink }, opacity);
                    let icon =
                        Rect::from_center_size(r.left_center() + vec2(20., 0.), Vec2::splat(16.));
                    glyph(ui.painter(), icon, key, fg, selected);
                    ui.painter().text(
                        r.left_center() + vec2(38., 0.),
                        Align2::LEFT_CENTER,
                        display,
                        FontId::new(
                            12.,
                            if selected {
                                FontFamily::Name("system-semibold".into())
                            } else {
                                FontFamily::Proportional
                            },
                        ),
                        fg,
                    );
                    if res.has_focus() {
                        ui.painter().rect_stroke(
                            if MAC { r } else { r.shrink2(vec2(9., 8.)) },
                            if MAC { 8 } else { 5 },
                            Stroke::new(1.0_f32, fg),
                            StrokeKind::Inside,
                        );
                    }
                    if res.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                        self.navigate(key, true)
                    }
                    ui.add_space(5.);
                }
                let footer = Rect::from_min_max(
                    pos2(side.left() + 12., side.bottom() - 15.),
                    side.right_bottom(),
                );
                ui.scope_builder(UiBuilder::new().max_rect(footer), |ui| {
                    let version = caption(ui, concat!("v", env!("QUOTABAR_VERSION")), p);
                    version.widget_info(|| {
                        WidgetInfo::labeled(
                            WidgetType::Label,
                            true,
                            concat!("앱 버전 ", env!("QUOTABAR_VERSION")),
                        )
                    });
                });
            });
        CentralPanel::default().frame(Frame::new().fill(p.bg)).show(ctx, |ui| {
            Frame::new().inner_margin(Margin { left: 28, right: 28, top: 32 + titlebar, bottom: 22 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let (title, subtitle) = match self.route.as_str() {
                    "accounts" => ("Providers & Accounts", "계정 연결과 로그인, 모니터링 상태를 관리하세요."),
                    "settings" => ("설정", "화면 표시, 모니터링, 앱 실행과 업데이트를 설정하세요."),
                    _ => ("Dashboard", "남은 사용량과 활동을 확인하세요."),
                };
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        label(ui, title, 25., true, p.ink);
                        ui.add_space(5.);
                        label(ui, subtitle, if MAC { 12. } else { 11. }, false, p.muted);
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| match self.route.as_str() {
                        "accounts" => {
                            if action_on(ui, "+  계정 연결", true, p, p.bg).clicked() { self.open_connect("Codex", None); }
                        }
                        "settings" => {}
                        _ => {
                            let shortcut = if MAC { "새로고침 (⌘R)" } else { "새로고침 (Ctrl+R)" };
                            if action_on(ui, "↻  새로고침", false, p, p.bg).on_hover_text(shortcut).clicked() { self.refresh(); }
                            if MAC && self.state.accounts.iter().any(|a| a.loading) { ui.spinner(); }
                        }
                    });
                });
            });
            if self.state.demo {
                let banner = if MAC { p.bg.lerp_to_gamma(p.ink, 0.06) } else { p.track };
                Frame::new().fill(banner).inner_margin(Margin::symmetric(28, 9)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let note = "미리보기 · 샘플 데이터입니다. 실제 계정이나 로그인 파일에 접근하지 않습니다.";
                    #[cfg(target_os = "macos")]
                    ui.horizontal(|ui| {
                        ui.spacing_mut().interact_size.y = 0.;
                        ui.spacing_mut().item_spacing.x = 7.;
                        let (icon, _) = ui.allocate_exact_size(vec2(14., 12.), Sense::hover());
                        crate::mac::symbol(ui.painter(), icon, "eye", 10., false, p.muted);
                        caption(ui, note, p);
                    });
                    #[cfg(windows)]
                    caption(ui, note, p);
                });
                if !MAC { ui.add_space(18.); }
            }
            let logging = self.state.raw["loggingIn"] == true;
            let message = self.state.raw["message"].as_str()
                .or_else(|| logging.then_some("브라우저에서 로그인을 완료하세요.")).map(str::to_owned);
            if let Some(msg) = message {
                Frame::new().outer_margin(Margin::symmetric(28, 0)).inner_margin(14)
                    .fill(p.warning.gamma_multiply(0.08)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        label(ui, msg, 12., false, p.warning);
                        if logging && action_on(ui, "로그인 취소", false, p, p.bg.lerp_to_gamma(p.warning, 0.08)).clicked() { self.wire.action("cancel-login", Value::Null); }
                        if MAC && link(ui, "닫기", p, p.bg).clicked() {
                            self.wire.action("message", Value::Null);
                            self.state.raw["message"] = Value::Null;
                        }
                    });
                });
                ui.add_space(18.);
            }
            #[cfg(target_os = "macos")]
            if self.route == "accounts" {
                Frame::new().inner_margin(Margin::symmetric(28, 0)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    self.accounts(ui, p);
                });
                return;
            }
            let mut scroll = ScrollArea::vertical().auto_shrink([false, false]).id_salt((&self.route, "body"));
            if self.scroll_top {
                scroll = scroll.vertical_scroll_offset(0.);
                self.scroll_top = false;
            }
            let body = |ui: &mut Ui| {
                Frame::new().inner_margin(Margin { left: 28, right: 28, top: 0, bottom: 28 }).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    match self.route.as_str() {
                        "accounts" => self.accounts(ui, p),
                        "settings" => self.settings(ui, p),
                        _ => self.overview(ui, p),
                    }
                });
            };
            #[cfg(windows)]
            crate::windows_scroll::show(ui, scroll, p, body);
            #[cfg(target_os = "macos")]
            mac_scroll(ui, scroll, p, body);
        });
    }
    fn overview(&mut self, ui: &mut Ui, p: Palette) {
        let enabled: Vec<_> = self.state.accounts.iter().filter(|a| a.enabled).collect();
        let providers = enabled.iter().map(|a| a.provider).collect::<HashSet<_>>();
        let attention = enabled
            .iter()
            .filter(|a| a.attention() || a.lowest().is_some_and(|n| n <= 20.))
            .count();
        let now = Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        let month = now.format("%Y-%m").to_string();
        let tokens = |prefix: &str| {
            if !self.state.flag("scanLocalLogs", "scanLocalLogs") || self.state.report.is_null() {
                "—".into()
            } else {
                let count = list(&self.state.report["days"])
                    .iter()
                    .filter(|d| text(d, "day", "").starts_with(prefix))
                    .map(|d| total(&d["counts"]))
                    .sum();
                if MAC && count >= 1_000_000 {
                    format!("{:.1}M", count as f64 / 1_000_000.)
                } else {
                    number(count)
                }
            }
        };
        #[cfg(target_os = "macos")]
        let month_name = crate::platform::activity_date(now.date_naive(), "MMMM");
        #[cfg(windows)]
        let month_name = format!("{}월", now.month());
        let stats = [
            (
                "모니터링 중",
                enabled.len().to_string(),
                format!("{}개 Provider", providers.len()),
                "accounts",
                p.accent,
            ),
            (
                "확인 필요",
                attention.to_string(),
                "잔여량 20% 이하 · 연결 상태".into(),
                "attention",
                if attention > 0 { p.warning } else { p.accent },
            ),
            (
                "오늘의 토큰",
                tokens(&today),
                if MAC {
                    "이 Mac의 로컬 기록".into()
                } else {
                    "이 PC의 로컬 기록".into()
                },
                "bolt",
                p.accent,
            ),
            (
                "이번 달 토큰",
                tokens(&month),
                month_name,
                "chart",
                p.accent,
            ),
        ];
        ui.horizontal_top(|ui| {
            let w = (ui.available_width() - 36.) / 4.;
            ui.spacing_mut().item_spacing.x = 12.;
            for (title, value, sub, symbol, color) in stats {
                p.frame(20).show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_width((w - 42.).max(1.));
                        ui.spacing_mut().interact_size.y = 0.;
                        ui.horizontal(|ui| {
                            caption(ui, title, p);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let (r, _) =
                                    ui.allocate_exact_size(Vec2::splat(14.), Sense::hover());
                                glyph(ui.painter(), r, symbol, color, false);
                            });
                        });
                        ui.add_space(12.);
                        label(ui, value, 28., true, p.ink);
                        ui.add_space(12.);
                        ui.add(Label::new(rich(sub, 10., false, p.muted)).truncate());
                    })
                });
            }
        });
        ui.add_space(24.);
        if self.state.accounts.is_empty() {
            p.frame(20).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    ui.add_space(28.);
                    ui.allocate_ui_with_layout(
                        vec2(if MAC { 152. } else { 162. }, 44.),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = 10.;
                            for a in ["Codex", "Claude", "Gemini"] {
                                self.assets.logo(ui, a, 44.);
                            }
                        },
                    );
                    ui.add_space(15.);
                    const DESCRIPTION: &str =
                        "Codex, Claude, Gemini의 잔여량을 나란히 확인할 수 있습니다.";
                    let title =
                        |ui: &mut Ui| label(ui, "첫 번째 계정을 연결하세요", 18., true, p.ink);
                    #[cfg(windows)]
                    {
                        // WPF centers the stack and left-aligns both TextBlocks inside it.
                        let width = crate::windows_text::natural_size(ui, DESCRIPTION, 11., false)
                            .map_or(ui.available_width(), |v| v.x.min(ui.available_width()));
                        ui.allocate_ui_with_layout(
                            vec2(width, 0.),
                            Layout::top_down(Align::Min),
                            title,
                        );
                    }
                    #[cfg(target_os = "macos")]
                    title(ui);
                    ui.add_space(if MAC { 15. } else { 10. });
                    caption(ui, DESCRIPTION, p);
                    ui.add_space(if MAC { 15. } else { 16. });
                    if action(ui, "계정 관리로 이동", true, p).clicked() {
                        self.navigate("accounts", true);
                    }
                    ui.add_space(28.);
                });
            });
            return;
        }
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                label(ui, "구독 현황", 15., true, p.ink);
                ui.add_space(4.);
                caption(
                    ui,
                    if MAC {
                        "계정별 잔여량 · 상단바에 표시할 계정을 선택하세요"
                    } else {
                        "계정별 잔여량 · 트레이에 표시할 계정을 선택하세요"
                    },
                    p,
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if link(ui, "계정 관리 →", p, p.bg).clicked() {
                    self.navigate("accounts", true);
                }
            });
        });
        ui.add_space(16.);
        let columns = columns(ui.available_width(), 290., 14.);
        let accounts = self.state.accounts.clone();
        for chunk in accounts.chunks(columns) {
            ui.horizontal_top(|ui| {
                let w = cell_width(ui.available_width(), columns, 14.);
                ui.spacing_mut().item_spacing.x = 14.;
                for a in chunk {
                    p.frame(20).show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width((w - 42.).max(1.));
                            self.quota_card(ui, p, a);
                        })
                    });
                }
            });
            ui.add_space(14.);
        }
        self.activity(ui, p);
    }
    fn quota_card(&mut self, ui: &mut Ui, p: Palette, a: &Account) {
        ui.spacing_mut().interact_size.y = 0.;
        if !a.enabled {
            ui.set_opacity(0.65);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.;
            self.assets.logo(ui, a.provider, 36.);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    label(ui, a.provider, 13., true, p.ink);
                    if let Some(plan) = a.snapshot["plan"].as_str() {
                        Frame::new()
                            .fill(p.track)
                            .corner_radius(8)
                            .inner_margin(Margin::symmetric(6, 3))
                            .show(ui, |ui| {
                                label(ui, plan, 9., false, p.ink);
                            });
                    }
                });
                ui.add_space(3.);
                ui.add(Label::new(rich(&a.name, 11., false, p.muted)).truncate())
                    .on_hover_text(&a.name);
            });
            if MAC && a.loading {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(Spinner::new().size(13.))
                        .on_hover_text("사용량 조회 중");
                });
            }
        });
        if !a.enabled {
            ui.add_space(if MAC { 17. } else { 24. });
            caption(ui, "모니터링 일시 정지", p);
            if !MAC {
                ui.add_space(24.);
            }
        } else if a.windows().is_empty() {
            ui.add_space(if MAC { 26. } else { 24. });
            caption(ui, a.quota_message(), p);
            ui.add_space(if MAC { 9. } else { 20. });
        } else {
            for w in a.windows() {
                ui.add_space(17.);
                let n = w["remaining"].as_f64().unwrap_or(0.);
                let percent = n.round();
                let tint = if n <= 20. {
                    p.warning
                } else {
                    p.tint(a.provider)
                };
                ui.horizontal(|ui| {
                    caption(ui, text(w, "title", ""), p);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        label(
                            ui,
                            format!("{percent}% 남음"),
                            11.,
                            true,
                            if n <= 20. { p.warning } else { p.ink },
                        );
                    });
                });
                ui.add_space(if MAC { 6. } else { 7. });
                let (r, res) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 5.), Sense::hover());
                res.widget_info(|| {
                    let spoken = if MAC { n.trunc() } else { percent };
                    WidgetInfo::labeled(
                        WidgetType::ProgressIndicator,
                        true,
                        format!("{} 잔여량 {spoken} 퍼센트", text(w, "title", "")),
                    )
                });
                ui.painter().rect_filled(r, 2, p.track);
                if n > 0. {
                    let color = if a.error.is_some() || a.snapshot["state"] == "stale" {
                        tint.gamma_multiply(0.45)
                    } else {
                        tint
                    };
                    ui.painter().rect_filled(
                        Rect::from_min_size(
                            r.min,
                            vec2(r.width() * (n.clamp(0., 100.) / 100.) as f32, 5.),
                        ),
                        2,
                        color,
                    );
                }
                let reset = match local_date(&w["resetAt"]) {
                    None if MAC => None,
                    None => Some("리셋 시간 미제공".into()),
                    Some(d) if d <= Local::now() => Some("리셋 시간 경과 · 새로고침 필요".into()),
                    #[cfg(windows)]
                    Some(d) => Some(format!("리셋 {}", d.format("%-m월 %-d일 %H:%M"))),
                    #[cfg(target_os = "macos")]
                    Some(d) => Some(format!("리셋 {}", crate::platform::quota_time(d, true))),
                };
                if let Some(reset) = reset {
                    ui.add_space(6.);
                    label(ui, reset, 9., false, p.muted);
                }
            }
        }
        if a.error.is_some() {
            ui.add_space(14.);
            label(
                ui,
                "최신 사용량을 확인하지 못했습니다.",
                11.,
                false,
                p.warning,
            );
            if link(ui, "계정 관리에서 확인 →", p, p.card).clicked() {
                self.navigate("accounts", true);
            }
        } else if a.snapshot["state"] == "stale" || (!MAC && a.snapshot["state"] == "reset") {
            ui.add_space(14.);
            label(
                ui,
                if a.snapshot["state"] == "stale" {
                    "이전 스냅샷 · Claude에서 메시지를 보내면 갱신됩니다."
                } else {
                    "리셋 이후 새 응답을 기다리고 있습니다."
                },
                11.,
                false,
                p.warning,
            );
        }
        if !a.snapshot.is_null() {
            ui.add_space(17.);
            #[cfg(target_os = "macos")]
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.;
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(4.), Sense::hover());
                ui.painter().circle_filled(
                    dot.center(),
                    2.,
                    if a.error.is_some() {
                        p.warning
                    } else {
                        p.tint(a.provider)
                    },
                );
                let observed = local_date(&a.snapshot["observedAt"]).map_or_else(
                    || "스냅샷 대기 중".into(),
                    |date| format!("마지막 데이터 {}", crate::platform::quota_time(date, false)),
                );
                label(ui, observed, 10., false, p.muted);
                if a.error.is_some() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        label(ui, "이전 값", 10., false, p.muted)
                    });
                }
            });
            #[cfg(windows)]
            {
                let fetched = local_date(&a.snapshot["observedAt"])
                    .or_else(|| local_date(&a.snapshot["fetchedAt"]));
                let val = if a.loading {
                    "업데이트 중…".into()
                } else if let Some(d) = fetched {
                    format!(
                        "●  마지막 데이터 {}{}",
                        d.format("%H:%M"),
                        if a.error.is_some() {
                            " · 이전 값"
                        } else {
                            ""
                        }
                    )
                } else {
                    "스냅샷 대기 중".into()
                };
                label(ui, val, 10., false, p.muted);
            }
        }
        divider(ui, p);
        let tray_label = if MAC {
            "상단바에 표시"
        } else {
            "트레이에 표시"
        };
        let name = a.control_label(tray_label);
        #[cfg(target_os = "macos")]
        let switch = {
            let label_width = crate::mac_text::natural_size(ui, tray_label, 11., false)
                .map_or(54., |size| size.x.ceil());
            ui.allocate_ui_with_layout(
                vec2(label_width + 36., 16.),
                Layout::left_to_right(Align::Center),
                |ui| named_toggle(ui, tray_label, &name, a.tray, p.tint(a.provider), p),
            )
            .inner
        };
        #[cfg(windows)]
        let switch = named_toggle(ui, tray_label, &name, a.tray, p.tint(a.provider), p);
        if switch.clicked() {
            self.edit_account(a, &a.name, a.enabled, !a.tray);
        }
    }
    pub fn edit_account(&self, a: &Account, name: &str, enabled: bool, tray: bool) {
        if !MAC {
            self.wire.action(
                "edit",
                json!({"id":a.id,"name":name,"enabled":enabled,"tray":tray}),
            );
        } else if a.enabled != enabled {
            self.wire.action("toggle", json!(a.id));
        } else if a.tray != tray {
            self.wire.action("menuBarVisibility", json!([a.id, tray]));
        }
    }
}
pub fn columns(width: f32, min: f32, gap: f32) -> usize {
    ((width + gap) / (min + gap)).floor().max(1.) as usize
}
pub fn cell_width(width: f32, columns: usize, gap: f32) -> f32 {
    (width - gap * (columns - 1) as f32) / columns as f32
}
