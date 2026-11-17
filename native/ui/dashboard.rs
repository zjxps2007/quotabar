#[cfg(target_os = "macos")]
use crate::mac_text::{PreferredFont, preferred_label, symbol_label};
use crate::{Dashboard, model::*, style::*};
#[cfg(windows)]
use chrono::Datelike;
use chrono::Local;
use eframe::egui::*;
use serde_json::{Value, json};
use std::collections::HashSet;

#[cfg(target_os = "macos")]
fn mac_message_banner(
    ui: &mut Ui,
    message: &str,
    palette: Palette,
    field: Option<&crate::mac::BannerLabel>,
) -> bool {
    Frame::new()
        .inner_margin(14)
        .fill(surface_fill(palette.bg, palette.warning, 0.08))
        .show(ui, |ui| {
            let warning = "exclamationmark.triangle";
            let line_height = crate::mac_text::system_line_height(ui, 12.);
            let icon =
                crate::mac::symbol_size(ui.ctx(), warning, 12., false).unwrap_or(Vec2::splat(14.));
            let close_size = vec2(12., line_height);
            let width = ui.available_width();
            let text_width = (width - icon.x - close_size.x - 16.).max(1.);
            let text = WidgetText::from(
                rich(message, 12., false, palette.warning).line_height(Some(line_height)),
            )
            .into_galley(ui, Some(TextWrapMode::Wrap), text_width, TextStyle::Body);
            // SwiftUI centers both symbols against the final wrapped text height.
            let height = text.size().y.max(icon.y).max(close_size.y);
            let (row, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
            let icon_rect = Rect::from_center_size(row.left_center() + vec2(icon.x / 2., 0.), icon);
            crate::mac::symbol(
                ui.painter(),
                icon_rect,
                warning,
                12.,
                false,
                palette.warning,
            );
            let text_origin = pos2(icon_rect.right() + 8., row.center().y - text.size().y / 2.);
            let rect = Rect::from_min_size(text_origin, text.size());
            if let Some(field) = field {
                field.place(message, rect, text_width, ui.ctx().zoom_factor());
            } else {
                // GPU-only captures exclude AppKit subviews.
                ui.place(rect, Label::new(text).selectable(true));
            }
            let close_rect = Rect::from_center_size(
                row.right_center() - vec2(close_size.x / 2., 0.),
                close_size,
            );
            crate::mac::symbol(
                ui.painter(),
                close_rect,
                "xmark",
                12.,
                false,
                palette.warning,
            );
            let close = ui.interact(
                close_rect,
                ui.make_persistent_id("dismiss dashboard message"),
                Sense::click(),
            );
            close.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), "닫기"));
            close.clicked()
        })
        .inner
}

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
                if !MAC {
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
                    #[cfg(target_os = "macos")]
                    crate::mac_text::centered_label(ui, "QuotaBar", 18., true, p.ink)
                        .unwrap_or_else(|| label(ui, "QuotaBar", 18., true, p.ink));
                    #[cfg(windows)]
                    label(ui, "QuotaBar", 18., true, p.ink);
                });
                ui.add_space(30.);
                ui.scope(|ui| {
                    ui.spacing_mut().interact_size.y = 0.;
                    ui.horizontal(|ui| {
                        ui.add_space(12.);
                        let workspace = rich("WORKSPACE", 9., true, p.tertiary);
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
                        #[cfg(target_os = "macos")]
                        crate::mac::rounded_rect(
                            ui,
                            r,
                            9.,
                            p.accent,
                            Stroke::NONE,
                            crate::mac::Curve::Continuous,
                        );
                        #[cfg(windows)]
                        ui.painter()
                            .rect_filled(r, 8, p.rail.lerp_to_gamma(p.accent, opacity));
                    }
                    let fg = p
                        .rail
                        .lerp_to_gamma(if selected { p.on } else { p.ink }, opacity);
                    #[cfg(target_os = "macos")]
                    {
                        // SwiftUI uses 12 points of leading padding, an 18-point
                        // symbol frame and a 9-point gap before the text.
                        let icon = Rect::from_center_size(
                            r.left_center() + vec2(21., 0.),
                            vec2(18., r.height()),
                        );
                        if !crate::mac::symbol(
                            ui.painter(),
                            icon,
                            system_symbol_name(key),
                            12.,
                            selected,
                            fg,
                        ) {
                            glyph(ui, icon, key, fg, selected);
                        }
                        crate::mac_text::navigation_title(
                            ui,
                            r.min + vec2(39., 11.),
                            display,
                            selected,
                            fg,
                        );
                    }
                    #[cfg(windows)]
                    {
                        let icon = Rect::from_center_size(
                            r.left_center() + vec2(20., 0.),
                            Vec2::splat(16.),
                        );
                        glyph(ui, icon, key, fg, selected);
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
                    }
                    if res.has_focus() {
                        ui.painter().rect_stroke(
                            if MAC { r } else { r.shrink2(vec2(9., 8.)) },
                            if MAC { 8 } else { 5 },
                            Stroke::new(1.0_f32, fg),
                            StrokeKind::Inside,
                        );
                    }
                    if res.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                        self.navigate(ui.ctx(), key, true)
                    }
                    ui.add_space(5.);
                }
                #[cfg(target_os = "macos")]
                let footer_height = crate::mac_text::system_line_height(ui, 11.);
                #[cfg(windows)]
                let footer_height = 15.;
                let footer = Rect::from_min_max(
                    pos2(side.left() + 12., side.bottom() - footer_height),
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
        if MAC {
            // SwiftUI places a one-device-pixel Divider next to its 210-point sidebar.
            SidePanel::left("sidebar divider")
                .exact_width(1. / ctx.pixels_per_point())
                .resizable(false)
                .show_separator_line(false)
                .frame(Frame::new().fill(p.bg))
                .show(ctx, |ui| {
                    // The sidebar fills the title bar, while SwiftUI's Divider
                    // starts at the window's content layout rectangle.
                    let mut rect = ui.max_rect();
                    rect.min.y += f32::from(titlebar);
                    ui.painter().rect_filled(rect, 0, p.bg.lerp_to_gamma(p.accent, 0.1));
                });
        }
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
                        #[cfg(target_os = "macos")]
                        ui.set_min_height(crate::mac_text::title_stack_height(ui, 25., 12., 5.));
                        #[cfg(target_os = "macos")]
                        crate::mac_text::dashboard_heading(ui, title, p.ink)
                            .unwrap_or_else(|| system_label(ui, title, 25., true, p.ink));
                        #[cfg(windows)]
                        system_label(ui, title, 25., true, p.ink);
                        ui.add_space(5.);
                        system_label(ui, subtitle, if MAC { 12. } else { 11. }, false, p.muted);
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| match self.route.as_str() {
                        "accounts" => {
                            let button = action_on(ui, "+  계정 연결", true, p, p.bg);
                            #[cfg(windows)]
                            let button = tooltip_text(button, "계정 연결");
                            if button.clicked() { self.open_connect("Codex", None); }
                        }
                        "settings" => {}
                        _ => {
                            let shortcut = if MAC { "새로고침 (⌘R)" } else { "새로고침 (Ctrl+R)" };
                            let button = action_on(ui, "↻  새로고침", false, p, p.bg);
                            if tooltip_text(button, shortcut).clicked() { self.refresh(); }
                            if MAC && self.state.accounts.iter().any(|a| a.loading) { ui.spinner(); }
                        }
                    });
                });
            });
            if self.state.demo {
                let banner = if MAC { surface_fill(p.bg, p.ink, 0.06) } else { p.track };
                Frame::new().fill(banner).inner_margin(Margin::symmetric(28, 9)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let note = "미리보기 · 샘플 데이터입니다. 실제 계정이나 로그인 파일에 접근하지 않습니다.";
                    #[cfg(target_os = "macos")]
                    {
                        ui.spacing_mut().interact_size.y = 0.;
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 7.;
                            if !crate::mac::intrinsic_symbol(ui, "eye", 10., false, p.muted) {
                                ui.allocate_exact_size(vec2(16., 13.), Sense::hover());
                            }
                            if crate::mac_text::paragraph(ui, note, 10., 0., p.muted).is_none() {
                                system_label(ui, note, 10., false, p.muted);
                            }
                        });
                    }
                    #[cfg(windows)]
                    caption(ui, note, p);
                });
                if !MAC { ui.add_space(18.); }
            }
            #[cfg(target_os = "macos")]
            {
                let field = self.banner_label.as_ref();
                let message = self.state.raw["message"].as_str();
                let visible_field = field.filter(|_| self.capture.is_none() && message.is_some());
                if visible_field.is_none() && let Some(field) = field {
                    field.hide();
                }
                if let Some(message) = message && mac_message_banner(ui, message, p, visible_field) {
                    if let Some(field) = visible_field { field.hide(); }
                    self.wire.action("message", Value::Null);
                    self.state.raw["message"] = Value::Null;
                }
            }
            #[cfg(windows)]
            {
                let logging = self.state.raw["loggingIn"] == true;
                let message = self.state.raw["message"].as_str()
                    .or_else(|| logging.then_some("브라우저에서 로그인을 완료하세요."));
                if let Some(msg) = message {
                    Frame::new().outer_margin(Margin::symmetric(28, 0)).inner_margin(14)
                        .fill(p.warning.gamma_multiply(0.08)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            label(ui, msg, 12., false, p.warning);
                            if logging && action_on(ui, "로그인 취소", false, p, p.bg.lerp_to_gamma(p.warning, 0.08)).clicked() { self.wire.action("cancel-login", Value::Null); }
                        });
                    });
                    ui.add_space(18.);
                }
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
            #[cfg(target_os = "macos")]
            let rail = self.scroller.clone();
            #[cfg(target_os = "macos")]
            let capturing = self.capture.is_some();
            let viewport_top = ui.available_rect_before_wrap().top();
            let body = |ui: &mut Ui| {
                Frame::new().inner_margin(Margin { left: 28, right: 28, top: 0, bottom: 28 }).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    match self.route.as_str() {
                        "accounts" => self.accounts(ui, p),
                        "settings" => self.settings(ui, p),
                        _ => self.overview(ui, p, viewport_top),
                    }
                });
            };
            #[cfg(windows)]
            crate::windows_scroll::show(ui, scroll, p, body);
            #[cfg(target_os = "macos")]
            mac_scroll(ui, scroll, p, rail.as_deref(), capturing, body);
        });
    }
    fn overview(&mut self, ui: &mut Ui, p: Palette, viewport_top: f32) {
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
        let w = (ui.available_width() - 36.) / 4.;
        #[cfg(target_os = "macos")]
        let row_id = ui.make_persistent_id("dashboard statistics");
        #[cfg(target_os = "macos")]
        let stat_ids = stats.each_ref().map(|stat| row_id.with(stat.3));
        let tiles = |ui: &mut Ui| {
            ui.spacing_mut().item_spacing.x = 12.;
            let card = p.frame(20);
            let content_width = (w - card.total_margin().sum().x).max(1.);
            for (title, value, sub, symbol, color) in stats {
                let content = |ui: &mut Ui| {
                    ui.vertical(|ui| {
                        ui.set_width(content_width);
                        #[cfg(windows)]
                        {
                            ui.spacing_mut().interact_size.y = 0.;
                        }
                        #[cfg(target_os = "macos")]
                        {
                            let title_height = crate::mac_text::system_line_height(ui, 11.);
                            let symbol = crate::mac::symbol_size(
                                ui.ctx(),
                                system_symbol_name(symbol),
                                13.,
                                false,
                            );
                            ui.spacing_mut().interact_size.y =
                                symbol.map_or(title_height, |s| s.y.max(title_height));
                        }
                        ui.horizontal(|ui| {
                            #[cfg(target_os = "macos")]
                            crate::mac_text::stacked_label(ui, title, 11., false, p.muted)
                                .unwrap_or_else(|| system_label(ui, title, 11., false, p.muted));
                            #[cfg(windows)]
                            system_label(ui, title, 11., false, p.muted);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                #[cfg(target_os = "macos")]
                                if crate::mac::intrinsic_symbol(
                                    ui,
                                    system_symbol_name(symbol),
                                    13.,
                                    false,
                                    color,
                                ) {
                                    return;
                                }
                                let (r, _) =
                                    ui.allocate_exact_size(Vec2::splat(14.), Sense::hover());
                                glyph(ui, r, symbol, color, false);
                            });
                        });
                        ui.add_space(12.);
                        #[cfg(target_os = "macos")]
                        if crate::mac_text::stat_value(ui, &value, p.ink).is_none() {
                            label(ui, value, 28., true, p.ink);
                        }
                        #[cfg(windows)]
                        label(ui, value, 28., true, p.ink);
                        ui.add_space(12.);
                        #[cfg(target_os = "macos")]
                        {
                            let width = crate::mac_text::natural_size(ui, &sub, 10., false)
                                .map_or(0., |size| size.x);
                            let size = if width > ui.available_width() {
                                10. * (ui.available_width() / width).max(0.7)
                            } else {
                                10.
                            };
                            if crate::mac_text::stat_caption(ui, &sub, size, p.tertiary)
                                .is_none()
                            {
                                ui.add(Label::new(rich(sub, size, false, p.tertiary)).truncate());
                            }
                        }
                        #[cfg(windows)]
                        ellipsis_label(ui, &sub, 10., p.muted);
                    })
                };
                #[cfg(target_os = "macos")]
                card.show_centered(ui, row_id.with(symbol), w, content);
                #[cfg(windows)]
                card.show(ui, content);
            }
        };
        #[cfg(target_os = "macos")]
        centered_card_row(ui, stat_ids, tiles);
        #[cfg(windows)]
        ui.horizontal_top(tiles);
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
                    const TITLE: &str = "첫 번째 계정을 연결하세요";
                    const DESCRIPTION: &str =
                        "Codex, Claude, Gemini의 잔여량을 나란히 확인할 수 있습니다.";
                    #[cfg(windows)]
                    {
                        // WPF centers the stack and left-aligns both TextBlocks inside it.
                        let width = crate::windows_text::natural_size(ui, DESCRIPTION, 11., false)
                            .map_or(ui.available_width(), |v| v.x.min(ui.available_width()));
                        ui.allocate_ui_with_layout(
                            vec2(width, 0.),
                            Layout::top_down(Align::Min),
                            |ui| label(ui, TITLE, 18., true, p.ink),
                        );
                        ui.add_space(10.);
                        caption(ui, DESCRIPTION, p);
                    }
                    #[cfg(target_os = "macos")]
                    {
                        preferred_label(ui, TITLE, PreferredFont::Title3Bold, p.ink);
                        ui.add_space(15.);
                        preferred_label(ui, DESCRIPTION, PreferredFont::Body, p.muted);
                    }
                    ui.add_space(if MAC { 15. } else { 16. });
                    if action(ui, "계정 관리로 이동", true, p).clicked() {
                        self.navigate(ui.ctx(), "accounts", true);
                    }
                    ui.add_space(28.);
                });
            });
            return;
        }
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                section_title(
                    ui,
                    "구독 현황",
                    if MAC {
                        "계정별 잔여량 · 상단바에 표시할 계정을 선택하세요"
                    } else {
                        "계정별 잔여량 · 트레이에 표시할 계정을 선택하세요"
                    },
                    p,
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                #[cfg(target_os = "macos")]
                let button = crate::mac_text::plain_button(ui, "계정 관리 →", p.accent);
                #[cfg(windows)]
                let button = link(ui, "계정 관리 →", p, p.bg);
                if button.clicked() {
                    self.navigate(ui.ctx(), "accounts", true);
                }
            });
        });
        ui.add_space(if MAC { 24. } else { 16. });
        let columns = columns(ui.available_width(), 290., 14.);
        let w = cell_width(ui.available_width(), columns, 14.);
        let accounts = self.state.accounts.clone();
        #[cfg(target_os = "macos")]
        let height_id = ui.make_persistent_id("quota card height");
        for chunk in accounts.chunks(columns) {
            let cards = |ui: &mut Ui| {
                ui.spacing_mut().item_spacing.x = 14.;
                let card = p.frame(20);
                let content_width = (w - card.total_margin().sum().x).max(1.);
                for a in chunk {
                    let content = |ui: &mut Ui| {
                        ui.vertical(|ui| {
                            ui.set_width(content_width);
                            self.quota_card(ui, p, a);
                        })
                    };
                    #[cfg(windows)]
                    card.show(ui, content);
                    #[cfg(target_os = "macos")]
                    let drawn = card.show_centered(ui, height_id.with(&a.id), w, content);
                    #[cfg(target_os = "macos")]
                    if !a.enabled {
                        // SwiftUI applies opacity to the entire card after compositing
                        // its background, border, labels and switch.
                        let border = 1. / ui.pixels_per_point();
                        ui.painter().rect_filled(
                            drawn.response.rect.expand(border),
                            0.,
                            p.bg.gamma_multiply(0.35),
                        );
                    }
                }
            };
            #[cfg(target_os = "macos")]
            centered_card_row(ui, chunk.iter().map(|a| height_id.with(&a.id)), cards);
            #[cfg(windows)]
            ui.horizontal_top(cards);
            ui.add_space(14.);
        }
        self.activity(ui, p, viewport_top);
    }
    fn quota_card(&mut self, ui: &mut Ui, p: Palette, a: &Account) {
        ui.spacing_mut().interact_size.y = 0.;
        #[cfg(windows)]
        if !a.enabled {
            ui.set_opacity(0.65);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.;
            self.assets.logo(ui, a.provider, 36.);
            ui.vertical(|ui| {
                #[cfg(windows)]
                if a.snapshot["plan"].is_string()
                    && let Some(plan_line) = crate::windows_text::line_height(9.)
                {
                    // WPF measures the padded plan badge before centering the
                    // provider TextBlock in the shared row.
                    ui.spacing_mut().interact_size.y = plan_line + 6.;
                }
                ui.horizontal(|ui| {
                    #[cfg(target_os = "macos")]
                    {
                        // The title and plan use the inner HStack's default spacing.
                        ui.spacing_mut().item_spacing.x = 8.;
                    }
                    label(ui, a.provider, 13., true, p.ink);
                    if let Some(plan) = a.snapshot["plan"].as_str() {
                        #[cfg(target_os = "macos")]
                        let fill = surface_fill(p.card, p.ink, 0.05);
                        #[cfg(windows)]
                        let fill = p.track;
                        Frame::new()
                            .fill(fill)
                            .corner_radius(if MAC { u8::MAX } else { 8 })
                            .inner_margin(Margin::symmetric(6, 3))
                            .show(ui, |ui| {
                                #[cfg(target_os = "macos")]
                                if crate::mac_text::plan_label(ui, plan, p.ink).is_some() {
                                    return;
                                }
                                label(ui, plan, 9., false, p.ink);
                            });
                    }
                });
                ui.add_space(3.);
                ellipsis_label(ui, &a.name, 11., p.muted);
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
            #[cfg(target_os = "macos")]
            symbol_label(
                ui,
                "pause.circle",
                "모니터링 일시 정지",
                PreferredFont::Callout,
                p.muted,
            );
            #[cfg(windows)]
            {
                caption(ui, "모니터링 일시 정지", p);
                ui.add_space(24.);
            }
        } else if a.windows().is_empty() {
            ui.add_space(if MAC { 26. } else { 24. });
            #[cfg(target_os = "macos")]
            symbol_label(
                ui,
                "clock",
                a.quota_message(),
                PreferredFont::Callout,
                p.muted,
            );
            #[cfg(windows)]
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
                    system_label(ui, text(w, "title", ""), 11., false, p.muted);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let percent = format!("{percent}% 남음");
                        let color = if n <= 20. { p.warning } else { p.ink };
                        #[cfg(target_os = "macos")]
                        if crate::mac_text::quota_percent(ui, &percent, color).is_some() {
                            return;
                        }
                        label(ui, percent, 11., true, color);
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
                    label(ui, reset, 9., false, p.tertiary);
                }
            }
        }
        if a.error.is_some() {
            ui.add_space(if MAC { 17. } else { 14. });
            const ERROR: &str = "최신 사용량을 확인하지 못했습니다.";
            const MANAGE: &str = "계정 관리에서 확인 →";
            #[cfg(target_os = "macos")]
            let button = {
                symbol_label(
                    ui,
                    "exclamationmark.circle",
                    ERROR,
                    PreferredFont::Caption,
                    p.warning,
                );
                ui.add_space(6.);
                crate::mac_text::caption_button(ui, MANAGE, p.system_link)
            };
            #[cfg(windows)]
            let button = {
                label(ui, ERROR, 11., false, p.warning);
                link(ui, MANAGE, p, p.card)
            };
            if button.clicked() {
                self.navigate(ui.ctx(), "accounts", true);
            }
        } else if a.snapshot["state"] == "stale" || (!MAC && a.snapshot["state"] == "reset") {
            ui.add_space(if MAC { 17. } else { 14. });
            let note = if a.snapshot["state"] == "stale" {
                "이전 스냅샷 · Claude에서 메시지를 보내면 갱신됩니다."
            } else {
                "리셋 이후 새 응답을 기다리고 있습니다."
            };
            #[cfg(target_os = "macos")]
            preferred_label(ui, note, PreferredFont::Caption, p.warning);
            #[cfg(windows)]
            label(ui, note, 11., false, p.warning);
        }
        if !a.snapshot.is_null() {
            ui.add_space(17.);
            #[cfg(target_os = "macos")]
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.;
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(4.), Sense::hover());
                let observed = local_date(&a.snapshot["observedAt"]).map_or_else(
                    || "스냅샷 대기 중".into(),
                    |date| format!("마지막 데이터 {}", crate::platform::quota_time(date, false)),
                );
                let caption = system_label(ui, &observed, 10., false, p.tertiary);
                ui.painter().circle_filled(
                    pos2(dot.center().x, caption.rect.center().y),
                    2.,
                    if a.error.is_some() {
                        p.warning
                    } else {
                        p.tint(a.provider)
                    },
                );
                if a.error.is_some() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        label(ui, "이전 값", 10., false, p.tertiary)
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
        #[cfg(target_os = "macos")]
        spaced_divider(ui, surface_fill(p.card, p.ink, 0.12), 17., 17.);
        #[cfg(windows)]
        divider(ui, p);
        let tray_label = if MAC {
            "상단바에 표시"
        } else {
            "트레이에 표시"
        };
        let name = a.control_label(tray_label);
        #[cfg(target_os = "macos")]
        let switch = {
            ui.allocate_ui_with_layout(
                vec2(mini_toggle_width(ui, tray_label), 15.),
                Layout::left_to_right(Align::Center),
                |ui| named_toggle(ui, tray_label, &name, a.tray, p.tint(a.provider), p),
            )
            .inner
        };
        #[cfg(target_os = "macos")]
        let switch = tooltip_text(
            switch,
            if a.enabled {
                "상단바 요약과 팝업에 표시합니다. 꺼도 사용량 조회는 계속됩니다."
            } else {
                "모니터링을 재개하면 이 설정에 따라 상단바에 표시됩니다."
            },
        );
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
pub fn section_title(ui: &mut Ui, title: &str, subtitle: &str, p: Palette) {
    #[cfg(target_os = "macos")]
    {
        if crate::mac_text::section_heading(ui, title, p.ink).is_none() {
            system_label(ui, title, 15., true, p.ink);
        }
        ui.add_space(4.);
        preferred_label(ui, subtitle, PreferredFont::Caption, p.muted);
    }
    #[cfg(windows)]
    {
        let stack = crate::windows_text::TextStack::inline(ui);
        stack.label(ui, title, 15., true, p.ink);
        ui.add_space(4.);
        stack.advance(4.);
        stack.label(ui, subtitle, 11., false, p.muted);
        // The parent measures the entire WPF StackPanel in one pass. Individual
        // label allocations can otherwise shorten it before the next card.
        ui.expand_to_include_y(stack.top(ui.min_rect().bottom()));
    }
}
pub fn columns(width: f32, min: f32, gap: f32) -> usize {
    ((width + gap) / (min + gap)).floor().max(1.) as usize
}
pub fn cell_width(width: f32, columns: usize, gap: f32) -> f32 {
    (width - gap * (columns - 1) as f32) / columns as f32
}
