//! The notification popup owns a Win32 window and accessible owner-drawn button
//! children. The monitor only publishes display state and receives chosen actions.
mod paint;
use chrono::{DateTime, Local};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    io,
    ptr::{null, null_mut},
    sync::OnceLock,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, GetDoubleClickTime, ReleaseCapture, SetCapture,
};
use windows_sys::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::*,
            HiDpi::{GetDpiForSystem, GetDpiForWindow},
            Shell::{DefSubclassProc, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    },
    core::w,
};
const CLASS: *const u16 = w!("QuotaBar.Native.UsageMenu");
const SCROLL_TIMER: usize = 1;
const SCROLL_ICON_WIDTHS: [u32; 7] = [16, 24, 32, 40, 48, 56, 64];
static REGISTER: OnceLock<Result<u16, i32>> = OnceLock::new();
pub type Callback = extern "C" fn(i32);

pub fn show_message(text: &str, error: bool) {
    let message: Vec<_> = text.encode_utf16().chain(Some(0)).collect();
    let title: Vec<_> = "QuotaBar".encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | if error { MB_ICONERROR } else { 0 },
        );
    }
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
#[derive(Clone)]
enum Item {
    Header(bool),
    Caption(usize),
    Empty(bool),
    Account {
        provider: String,
        name: String,
        periods: Vec<Period>,
        uncertain: bool,
        loading: bool,
    },
    Divider,
    Dashboard,
    Quit,
    Padding,
}
#[derive(Clone)]
struct Period {
    title: String,
    percent: f64,
    reset: String,
}
impl Period {
    fn percent_label(&self) -> String {
        if self.percent.is_finite() {
            format!("{}%", self.percent.clamp(0., 100.).round())
        } else {
            "—".into()
        }
    }
}
struct Row {
    id: String,
    label: String,
    item: Item,
    top: i32,
    height: i32,
    button: HWND,
}
impl Row {
    fn action(&self) -> bool {
        matches!(
            self.item,
            Item::Account { .. } | Item::Empty(_) | Item::Dashboard | Item::Quit
        )
    }
}
struct Layout {
    rows: Vec<Row>,
    appearance: String,
}
impl Layout {
    fn new(v: &Value) -> Self {
        let settings = &v["settings"];
        let all = settings["accounts"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let accounts: Vec<_> = all
            .iter()
            .filter(|a| a["enabled"] != false && a["showInTray"] != false)
            .collect();
        let mut result = Self {
            rows: vec![],
            appearance: settings["appearance"].as_str().unwrap_or("system").into(),
        };
        let mut add = |id: String, label: String, height, item| {
            result.rows.push(Row {
                id,
                label,
                item,
                top: 0,
                height,
                button: null_mut(),
            });
        };
        add(
            "header".into(),
            "QuotaBar".into(),
            62,
            Item::Header(v["demo"] == true),
        );
        add(
            "caption".into(),
            "남은 사용량".into(),
            32,
            Item::Caption(accounts.len()),
        );
        if accounts.is_empty() {
            add(
                "empty".into(),
                if all.is_empty() {
                    "계정을 연결해 주세요"
                } else {
                    "트레이에 표시할 계정이 없습니다"
                }
                .into(),
                94,
                Item::Empty(all.is_empty()),
            );
        }
        for a in accounts {
            let id = a["id"].as_str().unwrap_or("");
            let name = a["name"].as_str().unwrap_or("").to_owned();
            let provider = match a["provider"].as_str() {
                Some("claude") => "Claude",
                Some("gemini") => "Gemini",
                _ => "Codex",
            }
            .to_owned();
            let snapshot = &v["snapshots"][id];
            let uncertain = !v["errors"][id].is_null()
                || matches!(
                    snapshot["state"].as_str(),
                    Some("stale" | "reset" | "disconnected" | "api-routed")
                );
            let loading = v["loading"].as_array().is_some_and(|rows| {
                rows.iter()
                    .any(|x| x.as_str().is_some_and(|s| s.eq_ignore_ascii_case(id)))
            });
            let periods: Vec<_> = snapshot["windows"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|q| {
                    let reset = q["resetAt"]
                        .as_str()
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&Local));
                    Period {
                        title: q["title"].as_str().unwrap_or("").into(),
                        percent: q["remaining"].as_f64().unwrap_or(f64::NAN),
                        reset: match reset {
                            None => "리셋 시간 미제공".into(),
                            Some(d) if d <= Local::now() => "리셋 경과 · 갱신 필요".into(),
                            Some(d) => format!("{} 리셋", crate::windows_format::month_day_time(d)),
                        },
                    }
                })
                .collect();
            let detail = if periods.is_empty() {
                "데이터 대기 중".into()
            } else {
                periods
                    .iter()
                    .map(|w| format!("{}: {} 남음, {}", w.title, w.percent_label(), w.reset))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let label = format!(
                "{provider} · {name}. {detail}{}. Dashboard에서 보기",
                if uncertain {
                    ". 연결 확인 필요. 이전 값 포함"
                } else {
                    ""
                }
            );
            let h =
                66 + periods.len().div_ceil(2).max(1) as i32 * 52 + if uncertain { 22 } else { 0 };
            add(
                format!("account-{id}"),
                label,
                h,
                Item::Account {
                    provider,
                    name,
                    periods,
                    uncertain,
                    loading,
                },
            );
        }
        add("accounts-divider".into(), String::new(), 12, Item::Divider);
        add(
            "dashboard".into(),
            "Dashboard 열기".into(),
            38,
            Item::Dashboard,
        );
        add("quit".into(), "종료".into(), 32, Item::Quit);
        add("bottom-padding".into(), String::new(), 8, Item::Padding);
        result
    }
    fn measure(&mut self, scale: f32) -> i32 {
        // The original menu rounds each item's preferred height separately.
        let mut y = 2;
        for row in &mut self.rows {
            row.top = y;
            y += (row.height as f32 * scale).ceil() as i32;
        }
        y + 2
    }
}
pub struct Menu {
    window: HWND,
    callback: Box<dyn Fn(i32)>,
    layout: RefCell<Layout>,
    selected: Cell<Option<usize>>,
    first_row: Cell<usize>,
    arrow_height: Cell<i32>,
    scroll_buttons: [HWND; 2],
    pressed_arrow: Cell<i32>,
    wheel_remainder: Cell<i32>,
    anchor: Cell<POINT>,
    scale: Cell<f32>,
    height: Cell<i32>,
}
impl Menu {
    pub fn new(callback: Callback) -> io::Result<Box<Self>> {
        Self::with_callback(move |action| callback(action))
    }
    pub fn with_callback(callback: impl Fn(i32) + 'static) -> io::Result<Box<Self>> {
        crate::tray_icon::start()?;
        unsafe {
            let module = GetModuleHandleW(null());
            let atom = REGISTER.get_or_init(|| {
                let a = RegisterClassW(&WNDCLASSW {
                    style: CS_DROPSHADOW,
                    lpfnWndProc: Some(window_proc),
                    hInstance: module,
                    hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                    lpszClassName: CLASS,
                    ..Default::default()
                });
                if a == 0 {
                    Err(io::Error::last_os_error().raw_os_error().unwrap_or(1))
                } else {
                    Ok(a)
                }
            });
            if let Err(code) = atom {
                return Err(io::Error::from_raw_os_error(*code));
            }
            let window = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                CLASS,
                w!("QuotaBar 사용량 메뉴"),
                WS_POPUP | WS_CLIPCHILDREN,
                0,
                0,
                1,
                1,
                null_mut(),
                null_mut(),
                module,
                null(),
            );
            if window.is_null() {
                return Err(io::Error::last_os_error());
            }
            let scroll_buttons =
                [(w!("위로 스크롤"), 1), (w!("아래로 스크롤"), 2)].map(|(label, id)| {
                    let button = CreateWindowExW(
                        0,
                        w!("BUTTON"),
                        label,
                        WS_CHILD | BS_OWNERDRAW as u32,
                        0,
                        0,
                        0,
                        0,
                        window,
                        id as HMENU,
                        module,
                        null(),
                    );
                    if !button.is_null() {
                        SetWindowSubclass(button, Some(button_proc), 1, window as usize);
                    }
                    button
                });
            if scroll_buttons.iter().any(|hwnd| hwnd.is_null()) {
                let error = io::Error::last_os_error();
                DestroyWindow(window);
                return Err(error);
            }
            let menu = Box::new(Self {
                window,
                callback: Box::new(callback),
                layout: RefCell::new(Layout::new(&Value::Null)),
                selected: Cell::new(None),
                first_row: Cell::new(0),
                arrow_height: Cell::new(0),
                scroll_buttons,
                pressed_arrow: Cell::new(0),
                wheel_remainder: Cell::new(0),
                anchor: Cell::new(POINT::default()),
                scale: Cell::new(1.),
                height: Cell::new(0),
            });
            SetWindowLongPtrW(window, GWLP_USERDATA, (&*menu as *const Menu) as isize);
            Ok(menu)
        }
    }
    pub fn hwnd(&self) -> HWND {
        self.window
    }
    pub fn visible(&self) -> bool {
        unsafe { IsWindowVisible(self.window) != 0 }
    }
    pub fn update(&self, value: &Value) {
        let mut next = Layout::new(value);
        let old = self.layout.replace(Layout::new(&Value::Null));
        let selected = self
            .selected
            .get()
            .and_then(|i| old.rows.get(i))
            .map(|r| r.id.clone());
        // Preserve accessible elements and pointer tracking on frequent quota updates.
        let mut previous: std::collections::HashMap<_, _> = old
            .rows
            .into_iter()
            .filter(|r| !r.button.is_null())
            .map(|r| (r.id, r.button))
            .collect();
        for (i, r) in next.rows.iter_mut().enumerate() {
            if r.action() {
                unsafe {
                    let name = wide(&r.label);
                    let button = previous.remove(&r.id).unwrap_or_else(|| {
                        CreateWindowExW(
                            0,
                            w!("BUTTON"),
                            name.as_ptr(),
                            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW as u32,
                            0,
                            0,
                            1,
                            1,
                            self.window,
                            (100 + i) as HMENU,
                            GetModuleHandleW(null()),
                            null(),
                        )
                    });
                    if !button.is_null() {
                        SetWindowTextW(button, name.as_ptr());
                        SetWindowLongPtrW(button, GWLP_ID, (100 + i) as isize);
                        SetWindowSubclass(button, Some(button_proc), 1, self.window as usize);
                    }
                    r.button = button;
                }
            }
        }
        for hwnd in previous.into_values() {
            unsafe {
                DestroyWindow(hwnd);
            }
        }
        self.selected
            .set(selected.and_then(|id| next.rows.iter().position(|r| r.id == id)));
        self.layout.replace(next);
        if self.visible() {
            self.position(false);
        }
    }
    pub fn show(&self, x: i32, y: i32) {
        self.anchor.set(POINT { x, y });
        self.first_row.set(0);
        self.wheel_remainder.set(0);
        self.selected.set(None);
        // Moving first gives GetDpiForWindow the monitor under the shell anchor.
        unsafe {
            SetWindowPos(
                self.window,
                null_mut(),
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
        self.position(true);
        unsafe {
            SetForegroundWindow(self.window);
            windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(self.window);
        }
    }
    pub fn close(&self, keyboard: bool) {
        self.end_arrow_press();
        unsafe {
            ShowWindow(self.window, SW_HIDE);
        }
        if keyboard {
            (self.callback)(0);
        }
    }
    fn position(&self, show: bool) {
        unsafe {
            let pt = self.anchor.get();
            let mut monitor = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut monitor) == 0 {
                return;
            }
            let work = monitor.rcWork;
            let scale = (GetDpiForWindow(self.window) as f32 / 96.).max(1.);
            self.scale.set(scale);
            let width = (360. * scale).ceil() as i32 + 2;
            let full = self.layout.borrow_mut().measure(scale);
            let height = full.min((work.bottom - work.top - 12).max(80));
            self.height.set(height);
            self.arrow_height.set(if full > height {
                // The original scroll label uses an unscaled frame from a multi-size
                // icon selected at system DPI, even when the popup is on another monitor.
                let requested = (GetDpiForSystem() as f32 / 6.).round() as u32;
                let icon = SCROLL_ICON_WIDTHS
                    .iter()
                    .rev()
                    .copied()
                    .min_by_key(|width| width.abs_diff(requested))
                    .unwrap();
                icon as i32 + 4
            } else {
                0
            });
            let x = pt.x.clamp(work.left, (work.right - width).max(work.left));
            let y = pt.y.clamp(work.top, (work.bottom - height).max(work.top));
            SetWindowPos(
                self.window,
                HWND_TOPMOST,
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | if show { SWP_SHOWWINDOW } else { 0 },
            );
            if let Ok(region) = paint::window_region(width, height, 12. * scale) {
                if !region.is_null() && SetWindowRgn(self.window, region, 1) == 0 {
                    DeleteObject(region);
                }
            }
            self.scroll_to(self.first_row.get());
            RedrawWindow(
                self.window,
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_ALLCHILDREN,
            );
        }
    }
    fn viewport_height(&self) -> i32 {
        self.height.get() - 2 * self.arrow_height.get().max(2)
    }
    fn row_y(&self, layout: &Layout, row: &Row) -> i32 {
        self.arrow_height.get().max(2) + row.top - layout.rows[self.first_row.get()].top
    }
    fn last_start(&self, layout: &Layout, bottom: i32) -> usize {
        layout
            .rows
            .partition_point(|r| r.top < bottom - self.viewport_height())
            .min(layout.rows.len() - 1)
    }
    fn scroll_to(&self, first: usize) {
        let l = self.layout.borrow();
        let s = self.scale.get();
        let arrow = self.arrow_height.get();
        let last = l.rows.last().unwrap();
        let max = if arrow == 0 {
            0
        } else {
            self.last_start(&l, last.top + (last.height as f32 * s).ceil() as i32)
        };
        let first = first.min(max);
        self.first_row.set(first);
        let width = (360. * s).ceil() as i32;
        for (i, button) in self.scroll_buttons.iter().copied().enumerate() {
            unsafe {
                SetWindowPos(
                    button,
                    null_mut(),
                    1,
                    if i == 0 { 0 } else { self.height.get() - arrow },
                    width,
                    arrow,
                    SWP_NOZORDER
                        | SWP_NOACTIVATE
                        | if arrow == 0 {
                            SWP_HIDEWINDOW
                        } else {
                            SWP_SHOWWINDOW
                        },
                );
                windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow(
                    button,
                    (if i == 0 { first > 0 } else { first < max }).into(),
                );
            }
        }
        for r in &l.rows {
            if !r.button.is_null() {
                let y = self.row_y(&l, r);
                let h = (r.height as f32 * s).ceil() as i32;
                let w = (336. * s).ceil() as i32;
                let (top, bottom) = (y.max(arrow), (y + h).min(self.height.get() - arrow));
                unsafe {
                    // The portion under a scroll button must neither paint nor receive clicks.
                    let region = if top < bottom && (top > y || bottom < y + h) {
                        CreateRectRgn(0, top - y, w, bottom - y)
                    } else {
                        null_mut()
                    };
                    if SetWindowRgn(r.button, region, 0) == 0 && !region.is_null() {
                        DeleteObject(region);
                    }
                    SetWindowPos(
                        r.button,
                        null_mut(),
                        (12. * s).round() as i32,
                        y,
                        w,
                        h,
                        SWP_NOZORDER
                            | SWP_NOACTIVATE
                            | if top < bottom {
                                SWP_SHOWWINDOW
                            } else {
                                SWP_HIDEWINDOW
                            },
                    );
                }
            }
        }
        unsafe {
            RedrawWindow(
                self.window,
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_ALLCHILDREN,
            );
        }
    }
    fn step(&self, direction: i32) {
        if self.arrow_height.get() > 0 && direction != 0 {
            self.scroll_to(
                self.first_row
                    .get()
                    .saturating_add_signed(direction as isize),
            );
        }
    }
    fn end_arrow_press(&self) {
        self.pressed_arrow.set(0);
        unsafe {
            KillTimer(self.window, SCROLL_TIMER);
            if GetCapture() == self.window {
                ReleaseCapture();
            }
        }
    }
    fn press_arrow(&self, direction: i32) {
        self.pressed_arrow.set(direction);
        self.step(direction);
        unsafe {
            SetCapture(self.window);
            SetTimer(self.window, SCROLL_TIMER, GetDoubleClickTime(), None);
        }
    }
    fn select(&self, i: usize, reveal: bool) {
        {
            let l = self.layout.borrow();
            if !l.rows.get(i).is_some_and(Row::action) {
                return;
            }
        }
        if self.selected.replace(Some(i)) != Some(i) {
            unsafe {
                RedrawWindow(
                    self.window,
                    null(),
                    null_mut(),
                    RDW_INVALIDATE | RDW_ALLCHILDREN,
                );
            }
        }
        if reveal && self.arrow_height.get() > 0 {
            let l = self.layout.borrow();
            let row = &l.rows[i];
            let s = self.scale.get();
            let bottom = row.top + (row.height as f32 * s).ceil() as i32;
            let first = self.first_row.get();
            let next = if i < first {
                i
            } else if bottom > l.rows[first].top + self.viewport_height() {
                self.last_start(&l, bottom).min(i)
            } else {
                first
            };
            drop(l);
            if next != first {
                self.scroll_to(next);
            }
        }
    }
    fn key(&self, k: usize) {
        let actions: Vec<_> = self
            .layout
            .borrow()
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.action())
            .map(|(i, _)| i)
            .collect();
        if actions.is_empty() {
            return;
        }
        let pos = self
            .selected
            .get()
            .and_then(|i| actions.iter().position(|r| *r == i));
        match k {
            27 => self.close(true),
            13 | 32 => {
                if let Some(i) = self.selected.get() {
                    self.invoke(i);
                }
            }
            36 => self.select(actions[0], true),
            35 => self.select(*actions.last().unwrap(), true),
            38 => self.select(
                actions[pos.map_or(actions.len() - 1, |i| {
                    (i + actions.len() - 1) % actions.len()
                })],
                true,
            ),
            9 | 40 => self.select(actions[pos.map_or(0, |i| (i + 1) % actions.len())], true),
            33 | 34 if self.arrow_height.get() > 0 => {
                let l = self.layout.borrow();
                let y = l.rows[self.first_row.get()].top
                    + if k == 33 {
                        -self.viewport_height()
                    } else {
                        self.viewport_height()
                    };
                let next = l.rows.partition_point(|r| r.top < y);
                drop(l);
                self.scroll_to(next);
            }
            _ => {}
        }
    }
    fn invoke(&self, i: usize) {
        let action = self
            .layout
            .borrow()
            .rows
            .get(i)
            .filter(|r| r.action())
            .map(|r| if r.id == "quit" { 2 } else { 1 });
        if let Some(action) = action {
            self.close(false);
            (self.callback)(action);
        }
    }
}
impl Drop for Menu {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.window, GWLP_USERDATA, 0);
            DestroyWindow(self.window);
        }
    }
}
unsafe extern "system" fn button_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _: usize,
    parent: usize,
) -> LRESULT {
    unsafe {
        let root = parent as HWND;
        let menu = (GetWindowLongPtrW(root, GWLP_USERDATA) as *const Menu).as_ref();
        if let Some(menu) = menu {
            let id = GetDlgCtrlID(hwnd);
            let direction = if id == 1 { -1 } else { 1 };
            match msg {
                BM_CLICK if id < 100 => {
                    menu.step(direction);
                    return 0;
                }
                WM_LBUTTONDOWN if id < 100 => {
                    menu.press_arrow(direction);
                    return 0;
                }
                WM_MOUSEMOVE if id >= 100 => menu.select((id - 100) as usize, false),
                WM_KEYDOWN | WM_MOUSEWHEEL => return SendMessageW(root, msg, wp, lp),
                WM_GETDLGCODE => return DLGC_WANTALLKEYS as isize,
                _ => {}
            }
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
}
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if let Some(menu) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Menu).as_ref() {
            match msg {
                WM_PAINT => {
                    let mut ps = PAINTSTRUCT::default();
                    let dc = BeginPaint(hwnd, &mut ps);
                    let mut bounds = RECT::default();
                    GetClientRect(hwnd, &mut bounds);
                    paint::buffered(dc, bounds.right, bounds.bottom, |buffer| {
                        paint::background(menu, buffer);
                    });
                    EndPaint(hwnd, &ps);
                    return 0;
                }
                WM_PRINTCLIENT => {
                    paint::background(menu, wp as HDC);
                    return 0;
                }
                WM_DRAWITEM => {
                    if let Some(s) = (lp as *const DRAWITEMSTRUCT).as_ref() {
                        paint::buffered(s.hDC, s.rcItem.right, s.rcItem.bottom, |buffer| {
                            paint::child(menu, s, buffer);
                        });
                    }
                    return 1;
                }
                WM_ERASEBKGND => return 1,
                WM_MOUSEWHEEL => {
                    let delta = menu.wheel_remainder.get() + ((wp >> 16) as i16 as i32);
                    menu.wheel_remainder.set(delta % 120);
                    menu.step(-(delta / 120));
                    return 0;
                }
                WM_MOUSEMOVE if menu.pressed_arrow.get() != 0 => {
                    let mut rect = RECT::default();
                    GetClientRect(hwnd, &mut rect);
                    let x = lp as i16 as i32;
                    let y = (lp >> 16) as i16 as i32;
                    let in_band = if menu.pressed_arrow.get() < 0 {
                        y < menu.arrow_height.get()
                    } else {
                        y >= rect.bottom - menu.arrow_height.get()
                    };
                    if x < 1 || x >= rect.right - 1 || y < 0 || y >= rect.bottom || !in_band {
                        menu.end_arrow_press();
                    }
                    return 0;
                }
                WM_LBUTTONUP | WM_CAPTURECHANGED | WM_CANCELMODE
                    if menu.pressed_arrow.get() != 0 =>
                {
                    menu.end_arrow_press();
                    return 0;
                }
                WM_TIMER if wp == SCROLL_TIMER => {
                    let direction = menu.pressed_arrow.get();
                    if direction != 0 {
                        menu.step(direction);
                        SetTimer(hwnd, SCROLL_TIMER, 50, None);
                    }
                    return 0;
                }
                WM_KEYDOWN => {
                    menu.key(wp);
                    return 0;
                }
                WM_COMMAND if (wp >> 16) == BN_CLICKED as usize => {
                    match wp & 0xffff {
                        1 => menu.step(-1),
                        2 => menu.step(1),
                        id if id >= 100 => menu.invoke(id - 100),
                        _ => {}
                    }
                    return 0;
                }
                WM_ACTIVATE if wp & 0xffff == WA_INACTIVE as usize => {
                    if menu.visible() {
                        menu.close(false);
                    }
                    return 0;
                }
                WM_CLOSE => {
                    menu.close(false);
                    return 0;
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}
