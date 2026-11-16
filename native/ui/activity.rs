use crate::{
    Dashboard,
    dashboard::{cell_width, columns},
    model::*,
    style::*,
};
use chrono::{Datelike, Days, Local, NaiveDate};
use eframe::egui::*;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default, Clone, Copy)]
struct Tokens {
    input: u64,
    cached: u64,
    output: u64,
}
impl Tokens {
    fn add(&mut self, c: &Value) {
        self.input += count(c, "input");
        self.cached += count(c, "cached");
        self.output += count(c, "output");
    }
    fn total(self) -> u64 {
        self.input + self.output
    }
    fn detail(self) -> String {
        format!(
            "입력 {} · 캐시 입력 {} · 출력 {}",
            number(self.input),
            number(self.cached),
            number(self.output)
        )
    }
}
type Models = BTreeMap<String, Tokens>;
struct Calendar {
    start: NaiveDate,
    today: NaiveDate,
    days: BTreeMap<NaiveDate, Models>,
    models: Models,
    totals: BTreeMap<NaiveDate, u64>,
    quartiles: [u64; 3],
}
impl Calendar {
    fn new(report: &Value, provider: &str) -> Self {
        let today = local_date(&report["scannedAt"])
            .unwrap_or_else(Local::now)
            .date_naive();
        let start = today - Days::new(u64::from(today.weekday().num_days_from_monday()) + 25 * 7);
        let mut days = BTreeMap::<_, Models>::new();
        let mut models = Models::new();
        let mut totals = BTreeMap::new();
        for d in list(&report["days"]) {
            if !text(d, "provider", "").eq_ignore_ascii_case(provider) {
                continue;
            }
            let Ok(date) = NaiveDate::parse_from_str(text(d, "day", ""), "%Y-%m-%d") else {
                continue;
            };
            if date < start || date > today {
                continue;
            }
            let model = text(d, "model", "");
            let c = &d["counts"];
            if MAC && total(c) == 0 {
                continue;
            }
            days.entry(date)
                .or_default()
                .entry(model.into())
                .or_default()
                .add(c);
            models.entry(model.into()).or_default().add(c);
            *totals.entry(date).or_insert(0) += total(c);
        }
        let mut values: Vec<_> = totals.values().copied().filter(|n| *n > 0).collect();
        values.sort_unstable();
        let quartiles = [1, 2, 3].map(|n| {
            if values.is_empty() {
                0
            } else {
                values[(values.len() - 1) * n / 4]
            }
        });
        Self {
            start,
            today,
            days,
            models,
            totals,
            quartiles,
        }
    }
}
impl Dashboard {
    pub fn activity(&mut self, ui: &mut Ui, p: Palette, viewport_top: f32) {
        let providers: Vec<_> = ["Claude", "Codex"]
            .into_iter()
            .filter(|pr| self.state.accounts.iter().any(|a| a.provider == *pr))
            .collect();
        if providers.is_empty() {
            return;
        }
        ui.add_space(10.);
        let anchor = ui
            .horizontal(|ui| {
                ui.vertical(|ui| {
                    crate::dashboard::section_title(
                        ui,
                        "사용 활동",
                        &format!(
                            "최근 26주 · 이 {}의 {} 로그",
                            if MAC { "Mac" } else { "PC" },
                            providers.join(" / ")
                        ),
                        p,
                    );
                });
                if self.state.raw["scanning"] == true {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if MAC {
                            ui.spinner();
                        } else {
                            caption(ui, "업데이트 중…", p);
                        }
                    });
                }
            })
            .response;
        if self
            .activity_scroll_frame
            .is_some_and(|frame| ui.ctx().cumulative_frame_nr() >= frame)
        {
            if MAC {
                anchor.scroll_to_me(Some(Align::Min));
            } else {
                // WPF scrolls to the section in the StackPanel's logical coordinates.
                // ScrollArea's widget alignment would use a pixel-rounded content origin.
                ui.scroll_with_delta(vec2(0., viewport_top - anchor.rect.top()));
                ui.ctx()
                    .request_discard("Paint the activity section at its new scroll offset");
            }
        }
        ui.add_space(if MAC { 24. } else { 16. });
        let status = if !self.state.flag("scanLocalLogs", "scanLocalLogs") {
            Some("로컬 통계가 꺼져 있습니다. 설정에서 켤 수 있습니다.")
        } else if self.state.report.is_null() || (MAC && self.state.report["scannedAt"].is_null()) {
            Some(if self.state.raw["scanning"] == true {
                "로컬 기록을 읽고 있습니다…"
            } else {
                "로컬 사용 기록을 확인하세요."
            })
        } else {
            None
        };
        if let Some(msg) = status {
            p.frame(20).show(ui, |ui| {
                ui.set_width(ui.available_width());
                #[cfg(target_os = "macos")]
                ui.horizontal(|ui| {
                    let symbol = if self.state.flag("scanLocalLogs", "scanLocalLogs") {
                        "clock"
                    } else {
                        "pause.circle"
                    };
                    crate::mac::intrinsic_symbol(ui, symbol, 13., false, p.muted);
                    system_label(ui, msg, 13., false, p.muted);
                });
                #[cfg(not(target_os = "macos"))]
                caption(ui, msg, p);
            });
            return;
        }
        let cols = columns(ui.available_width(), 300., 16.);
        for (row, chunk) in providers.chunks(cols).enumerate() {
            if row > 0 {
                ui.add_space(16.);
            }
            ui.horizontal_top(|ui| {
                let width = cell_width(ui.available_width(), cols, 16.);
                #[cfg(target_os = "macos")]
                let width = width.min(560.);
                ui.spacing_mut().item_spacing.x = 16.;
                let card = p.frame(16).corner_radius(if MAC { 18 } else { 16 });
                let content_width = width - card.total_margin().sum().x;
                for &pr in chunk {
                    card.show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(content_width);
                            self.activity_card(ui, pr, p);
                        })
                    });
                }
            });
        }
        ui.add_space(if MAC { 24. } else { 16. });
        for w in list(&self.state.report["warnings"]) {
            if let Some(msg) = w.as_str() {
                label(ui, msg, 11., false, p.warning);
            }
        }
        const NOTE: &str = "토큰은 계정별 할당이 아닌 기기 전체 기록입니다. 구독 요금이나 API 청구 금액으로 환산하지 않습니다.";
        #[cfg(target_os = "macos")]
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.;
            crate::mac::intrinsic_symbol(ui, "info.circle", 10., false, p.muted);
            system_label(ui, NOTE, 10., false, p.muted);
        });
        #[cfg(windows)]
        {
            caption(ui, NOTE, p);
            ui.add_space(6.);
            let at = local_date(&self.state.report["scannedAt"])
                .map(crate::platform::month_day_time)
                .unwrap_or_default();
            let files = crate::platform::grouped(count(&self.state.report, "scannedFiles"));
            label(
                ui,
                format!("{files}개 파일 · {at} 집계 · 총 토큰 = 입력 + 출력 (캐시 입력 포함)"),
                10.,
                false,
                p.muted,
            );
        }
    }
    fn activity_card(&self, ui: &mut Ui, provider: &str, p: Palette) {
        let tint = p.tint(provider);
        // WPF measures the header row from its 26-DIP logo. Setting the
        // child spacing after egui allocates the row would retain its 28-DIP minimum.
        let header_height = if MAC {
            ui.spacing().interact_size.y
        } else {
            26.
        };
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), header_height),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = if MAC { 6. } else { 10. };
                self.assets.logo(ui, provider, 26.);
                system_label(ui, provider, if MAC { 11. } else { 13. }, true, tint);
            },
        );
        let cal = Calendar::new(&self.state.report, provider);
        let active = cal.totals.values().filter(|n| **n > 0).count();
        if active == 0 {
            #[cfg(target_os = "macos")]
            {
                ui.add_space(32.);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.;
                    crate::mac::intrinsic_symbol(ui, "square.grid.3x3", 15., false, p.muted);
                    ui.vertical(|ui| {
                        system_label(ui, "아직 사용 기록이 없습니다", 11., true, p.ink);
                        ui.add_space(2.);
                        system_label(
                            ui,
                            "로컬 로그가 확인되면 일별 토큰이 표시됩니다.",
                            10.,
                            false,
                            p.muted,
                        );
                    });
                });
            }
            #[cfg(windows)]
            {
                ui.add_space(24.);
                label(ui, "아직 사용 기록이 없습니다", 13., true, p.ink);
                caption(ui, "로컬 로그가 확인되면 일별 토큰이 표시됩니다.", p);
            }
            ui.add_space(24.);
            return;
        }
        ui.add_space(8.);
        let width = ui.available_width();
        let available = (width - 28.).max(0.);
        let gap = if MAC && width < 340. { 2. } else { 3. };
        let weeks = if MAC {
            (((available + gap) / (7. + gap)) as u64).clamp(1, 26)
        } else {
            26
        };
        let pitch = (available + if MAC { gap } else { 0. }) / weeks as f32;
        let side = (pitch - gap).clamp(7., 16.);
        let col_pitch = if MAC { side + gap } else { pitch };
        // WPF sizes the columns from the available width and clamps only the row height.
        let cell_width = if MAC { side } else { (pitch - gap).max(0.) };
        let h = 18. + (side + gap) * 7. - if MAC { gap } else { 0. };
        let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
        let mut prev = 0;
        let mut last_label = f32::NEG_INFINITY;
        for col in 0..weeks {
            let monday = cal.start + Days::new((26 - weeks + col) * 7);
            let x = rect.min.x + 28. + col_pitch * col as f32;
            if monday.month() != prev {
                if !MAC || x - last_label >= 24. {
                    calendar_label(
                        ui,
                        pos2(x, rect.min.y + if MAC { 0. } else { 9. }),
                        if MAC {
                            Align2::LEFT_TOP
                        } else {
                            Align2::LEFT_CENTER
                        },
                        &format!("{}월", monday.month()),
                        9.,
                        p.muted,
                    );
                    last_label = x;
                }
                prev = monday.month();
            }
            for row in 0..7 {
                let date = monday + Days::new(row);
                let n = cal.totals.get(&date).copied().unwrap_or(0);
                let inset = if MAC { 0. } else { 1.5 };
                let y = rect.min.y + 18. + (side + gap) * row as f32 + inset;
                if col == 0
                    && let Some(s) = match row {
                        0 => Some("월"),
                        2 => Some("수"),
                        4 => Some("금"),
                        _ => None,
                    }
                {
                    calendar_label(
                        ui,
                        pos2(rect.min.x, y + side / 2.),
                        Align2::LEFT_CENTER,
                        s,
                        if MAC && side < 9. { 8. } else { 9. },
                        p.muted,
                    );
                }
                let cell = Rect::from_min_size(pos2(x + inset, y), vec2(cell_width, side));
                let accessible = !MAC || n > 0 || date == cal.today;
                let input = if !accessible || date > cal.today {
                    // SwiftUI hides empty/future dates from AX. Both native grids
                    // still accept pointer input on future days without a tab stop.
                    Sense::click().difference(Sense::focusable_noninteractive())
                } else {
                    Sense::click()
                };
                let response = ui.interact(cell, ui.id().with((provider, date)), input);
                let level = if n == 0 {
                    0
                } else {
                    1 + cal.quartiles.iter().filter(|q| n > **q).count()
                };
                let alpha: f32 = if p.dark {
                    [0., 0.44, 0.62, 0.80, 0.98][level]
                } else {
                    [0., 0.38, 0.56, 0.74, 0.92][level]
                };
                let track = if date > cal.today {
                    if MAC {
                        p.ink.gamma_multiply(0.025)
                    } else {
                        p.track.gamma_multiply(0.4)
                    }
                } else {
                    p.track
                };
                let border = if date == cal.today {
                    Stroke::new(if MAC { 1.2_f32 } else { 1. }, tint)
                } else {
                    Stroke::NONE
                };
                #[cfg(windows)]
                {
                    // WPF's window renderer applies the brush's floating-point opacity.
                    // RenderTargetBitmap first quantizes it to an 8-bit alpha.
                    let fill = (level > 0).then(|| {
                        if self.capture.is_some() {
                            tint.gamma_multiply_u8((alpha * 255.).round() as u8)
                        } else {
                            tint.gamma_multiply(alpha)
                        }
                    });
                    crate::windows_border::activity_cell(ui, cell, track, fill, border);
                }
                #[cfg(target_os = "macos")]
                {
                    let highlighted = response.hovered() || response.has_focus();
                    let zoom = if highlighted
                        && !objc2_app_kit::NSWorkspace::sharedWorkspace()
                            .accessibilityDisplayShouldReduceMotion()
                    {
                        1.16
                    } else {
                        1.
                    };
                    let radius = (side * 0.24).clamp(2., 4.) * zoom;
                    let bounds = Rect::from_center_size(cell.center(), cell.size() * zoom);
                    let mut outline = border;
                    if highlighted {
                        let shadow = epaint::Shadow {
                            offset: [0, 0],
                            blur: 6,
                            spread: 0,
                            color: tint.gamma_multiply(0.5),
                        };
                        ui.painter()
                            .add(shadow.as_shape(bounds, radius.round() as u8));
                        outline = Stroke::new(1.5 * zoom, p.ink.gamma_multiply(0.92));
                    }
                    // SwiftUI composites the tint over the track before the final sRGB rounding.
                    let fill = if level > 0 {
                        surface_fill(track, tint, alpha)
                    } else {
                        track
                    };
                    crate::mac::rounded_rect(
                        ui,
                        bounds,
                        radius,
                        fill,
                        outline,
                        crate::mac::Curve::Continuous,
                    );
                }
                if accessible {
                    response.widget_info(|| {
                        WidgetInfo::labeled(
                            WidgetType::Button,
                            true,
                            format!(
                                "{provider} {date}, {} tokens{}",
                                crate::platform::grouped(n),
                                if date == cal.today { ", 오늘" } else { "" }
                            ),
                        )
                    });
                }
                #[cfg(target_os = "macos")]
                ui.ctx().accesskit_node_builder(response.id, |node| {
                    if accessible {
                        node.set_description("날짜별 모델 사용량 보기");
                    } else {
                        node.set_hidden();
                    }
                });
                let show = |ui: &mut Ui| {
                    #[cfg(target_os = "macos")]
                    {
                        mac_tooltip(ui, date, &cal, n, tint, p);
                    }
                    #[cfg(windows)]
                    {
                        label(
                            ui,
                            format!("{provider} · {}", date.format("%Y.%m.%d")),
                            12.,
                            true,
                            p.ink,
                        );
                        label(
                            ui,
                            format!("{} tokens", crate::platform::grouped(n)),
                            18.,
                            true,
                            tint,
                        );
                        if let Some(models) = cal.days.get(&date) {
                            for (model, c) in sorted(models) {
                                ui.add_space(6.);
                                label(
                                    ui,
                                    format!("{model} · {}", number(c.total())),
                                    10.,
                                    false,
                                    p.muted,
                                );
                                label(
                                    ui,
                                    format!(
                                        "입력 {} · 캐시 {} · 출력 {}",
                                        number(c.input),
                                        number(c.cached),
                                        number(c.output)
                                    ),
                                    10.,
                                    false,
                                    p.muted,
                                );
                            }
                        }
                    }
                };
                #[cfg(target_os = "macos")]
                if response.hovered() || response.has_focus() || response.clicked() {
                    Tooltip::for_widget(&response)
                        .width(304.)
                        .gap(10.)
                        .show(show);
                }
                #[cfg(windows)]
                windows_activity_tooltip(&response, p, show);
            }
        }
        ui.add_space(10.);
        let summary = format!("총 {} tokens", number(cal.totals.values().sum()));
        if let Some((date, peak)) = cal.totals.iter().rev().max_by_key(|(_, n)| **n) {
            #[cfg(windows)]
            let day = date.format("%-m월 %-d일").to_string();
            #[cfg(target_os = "macos")]
            let day = crate::platform::activity_date(*date, "MMMd");
            let peak = format!("활동 {active}일 · 최고 {} ({day})", number(*peak));
            if MAC {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.;
                    system_label(ui, &summary, 10., true, p.ink);
                    label(ui, "·", 10., false, p.muted);
                    system_label(ui, &peak, 10., false, p.muted);
                });
                return;
            }
            label(ui, summary, 11., true, p.ink);
            label(ui, peak, 10., false, p.muted);
        }
        ui.add_space(12.);
        disclosure(
            ui,
            ("models", provider),
            "모델별 사용량",
            10.,
            p,
            |ui| {
                for (model, c) in sorted(&cal.models).into_iter().take(12) {
                    ui.add_space(8.);
                    ui.horizontal(|ui| {
                        label(ui, model, 11., false, tint);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            label(ui, number(c.total()), 11., true, p.ink)
                        });
                    });
                    caption(ui, c.detail(), p);
                }
            },
        );
    }
}

#[cfg(target_os = "macos")]
use crate::mac_text::calendar_label;
#[cfg(windows)]
fn calendar_label(ui: &Ui, at: Pos2, align: Align2, text: &str, size: f32, color: Color32) {
    ui.painter()
        .text(at, align, text, FontId::proportional(size), color);
}

#[cfg(windows)]
fn windows_activity_tooltip(response: &Response, p: Palette, content: impl FnOnce(&mut Ui)) {
    let ctx = &response.ctx;
    let id = response.id.with("activity tooltip");
    if !(response.hovered() || response.has_focus() || response.clicked()) {
        ctx.data_mut(|data| data.remove::<f64>(id));
        return;
    }
    let now = ctx.input(|i| i.time);
    let entered = ctx.data_mut(|data| {
        let start = data.get_temp_mut_or_insert_with(id, || now);
        if response.clicked() {
            *start = now;
        }
        *start
    });
    // The calendar overrides WPF's ordinary one-second tooltip delay. A tooltip
    // opens immediately on a click or while moving between visible tooltips.
    let delay = if response.clicked()
        || response.has_focus()
        || Tooltip::seconds_since_last_tooltip(ctx) < ctx.style().interaction.tooltip_grace_time
    {
        0.
    } else {
        0.080
    };
    let elapsed = now - entered;
    if elapsed < delay {
        ctx.request_repaint_after_secs((delay - elapsed) as f32);
    } else if elapsed < delay + 60. {
        ctx.request_repaint_after_secs((delay + 60. - elapsed) as f32);
        let mut tip = Tooltip::for_widget(response);
        if response.hovered() {
            tip = tip.at_pointer().gap(16.);
        }
        tip.popup = tip.popup.frame(
            Frame::new()
                .inner_margin(12)
                .fill(p.card)
                .stroke(Stroke::new(1_f32, p.line)),
        );
        tip.show(|ui| {
            ui.set_max_width(314.);
            content(ui);
        });
    }
}
#[cfg(target_os = "macos")]
fn mac_tooltip(
    ui: &mut Ui,
    date: NaiveDate,
    calendar: &Calendar,
    total: u64,
    tint: Color32,
    p: Palette,
) {
    ui.set_max_width(278.);
    ui.horizontal_top(|ui| {
        label(
            ui,
            crate::platform::activity_date(date, "yMMMEd"),
            12.,
            true,
            p.ink,
        );
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            if date > calendar.today {
                label(ui, "아직 도래하지 않은 날짜", 9., false, p.muted);
            } else {
                ui.vertical(|ui| {
                    label(ui, "Tokens", 8.5, false, p.muted);
                    label(ui, crate::platform::grouped(total), 13.5, true, p.ink);
                });
            }
        });
    });
    if let Some(models) = calendar.days.get(&date) {
        ui.add_space(7.);
        ui.separator();
        ui.add_space(7.);
        label(ui, format!("모델 {}", models.len()), 9., true, p.muted);
        for (model, counts) in sorted(models).into_iter().take(4) {
            ui.add_space(6.);
            ui.horizontal_top(|ui| {
                let (dot, _) = ui.allocate_exact_size(vec2(6., 12.), Sense::hover());
                ui.painter().circle_filled(dot.center(), 3., tint);
                ui.vertical(|ui| {
                    label(ui, model, 10.5, true, p.ink);
                    let detail = format!(
                        "입력 {} · 출력 {} · 캐시 {}",
                        number(counts.input),
                        number(counts.output),
                        number(counts.cached)
                    );
                    label(ui, detail, 8.5, false, p.muted);
                });
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    label(ui, number(counts.total()), 10., true, p.ink)
                });
            });
        }
        if models.len() > 4 {
            label(
                ui,
                format!("그 외 {}개 모델", models.len() - 4),
                8.5,
                false,
                p.muted,
            );
        }
    }
}

fn sorted(models: &Models) -> Vec<(&String, &Tokens)> {
    let mut rows: Vec<_> = models.iter().collect();
    rows.sort_by_key(|(_, c)| std::cmp::Reverse(c.total()));
    rows
}
