//! Exercise the same windows and IPC as a normal launch using synthetic accounts.
use super::{App, Result, display};
use crate::{notification_icon::CALLBACK_MESSAGE, tray_capture, tray_menu::Menu};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    fs,
    mem::size_of,
    path::Path,
    ptr::null_mut,
    rc::Rc,
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, RECT},
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow},
    System::{ProcessStatus::*, Threading::GetCurrentProcess},
    UI::{
        HiDpi::GetDpiForWindow, Input::KeyboardAndMouse::IsWindowEnabled, WindowsAndMessaging::*,
    },
};

fn check(ok: bool, description: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(description.to_owned())
    }
}
fn wait(app: &mut App, seconds: u64, operation: &str, until: impl Fn(&App) -> bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !until(app) {
        if !app.dispatcher.pump(false).map_err(|e| e.to_string())? {
            return Err(format!("Quit while {operation}"));
        }
        app.events();
        if Instant::now() > deadline {
            return Err(format!(
                "Timed out {operation} (pid {}, ready {}, route {})",
                app.dispatcher.dashboard.pid(),
                app.dispatcher.dashboard.ready(),
                app.route
            ));
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}
fn send(hwnd: HWND, msg: u32, wp: usize, lp: isize) {
    unsafe {
        SendMessageW(hwnd, msg, wp, lp);
    }
}
fn enabled(h: HWND) -> bool {
    unsafe { IsWindowEnabled(h) != 0 }
}
fn visible(h: HWND) -> bool {
    unsafe { IsWindowVisible(h) != 0 }
}
fn rect(h: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe { GetWindowRect(h, &mut r) };
    r
}
fn contains(outer: RECT, inner: RECT) -> bool {
    inner.left >= outer.left
        && inner.top >= outer.top
        && inner.right <= outer.right
        && inner.bottom <= outer.bottom
}
fn area(h: HWND) -> Result<RECT> {
    let mut m = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    check(
        unsafe { GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut m) } != 0,
        "No monitor work area",
    )?;
    Ok(m.rcWork)
}
fn children(h: HWND) -> Vec<(String, HWND)> {
    unsafe extern "system" fn visit(h: HWND, out: LPARAM) -> i32 {
        let mut label = [0u16; 2048];
        let size = unsafe { GetWindowTextW(h, label.as_mut_ptr(), label.len() as i32) };
        unsafe {
            (*(out as *mut Vec<(String, HWND)>))
                .push((String::from_utf16_lossy(&label[..size as usize]), h));
        }
        1
    }
    let mut out = Vec::new();
    unsafe { EnumChildWindows(h, Some(visit), (&mut out as *mut _) as isize) };
    out
}
fn button(h: HWND, name: &str) -> Result<HWND> {
    children(h)
        .into_iter()
        .find(|(s, _)| s == name)
        .map(|(_, h)| h)
        .ok_or_else(|| format!("Missing menu button {name}"))
}

fn tray(app: &mut App, dir: &Path) -> Result<Value> {
    let original = app.sample.clone();
    let accounts = original["settings"]["accounts"].as_array().unwrap();
    let id = |n: usize| accounts[n]["id"].as_str().unwrap();
    let mut captures = Vec::new();
    let work = area(app.icon.window())?;
    let point = (((work.bottom - 2) as u16 as u32) << 16) | (work.right - 2) as u16 as u32;
    let mouse = |app: &mut App, msg: u32| {
        send(
            app.icon.window(),
            CALLBACK_MESSAGE,
            point as usize,
            (0x10000 | msg) as isize,
        );
        app.events();
    };
    let open = |app: &mut App| -> Result<()> {
        for msg in [WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU] {
            mouse(app, msg);
        }
        check(
            app.menu.visible() && app.dispatcher.dashboard.pid() == 0,
            "Right click failed to open the standalone tray menu",
        )
    };
    let mut capture = |app: &App, name: &str| -> Result<()> {
        tray_capture::menu(&app.menu, &dir.join(format!("{name}.png")))
            .map_err(|e| e.to_string())?;
        captures.push(name.to_owned());
        Ok(())
    };
    for click in [WM_LBUTTONDOWN, WM_LBUTTONDBLCLK] {
        mouse(app, click);
        mouse(app, WM_LBUTTONUP);
        check(
            !app.menu.visible() && app.dispatcher.dashboard.pid() == 0,
            "Unexpected window for left click",
        )?;
    }
    for theme in ["light", "dark"] {
        app.sample = original.clone();
        app.sample["settings"]["appearance"] = json!(theme);
        open(app)?;
        check(
            children(app.menu.hwnd())
                .iter()
                .filter(|(s, _)| s.contains("Dashboard에서 보기"))
                .count()
                == 3,
            "Missing account cards",
        )?;
        check(
            contains(area(app.menu.hwnd())?, rect(app.menu.hwnd())),
            "Menu outside the work area",
        )?;
        capture(app, &format!("tray-menu-{theme}"))?;
        app.menu.close(false);
        for a in &accounts[1..] {
            let i = a["id"].as_str().unwrap();
            let period = app.sample["snapshots"][i]["windows"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()
                .clone();
            app.sample["snapshots"][i]["windows"] = json!([period]);
        }
        open(app)?;
        capture(app, &format!("tray-menu-single-{theme}"))?;
        app.menu.close(false);
        app.sample["settings"]["accounts"] = json!([]);
        app.sample["snapshots"] = json!({});
        open(app)?;
        capture(app, &format!("tray-menu-empty-{theme}"))?;
        app.menu.close(false);
    }
    app.sample = original.clone();
    app.sample["settings"]["appearance"] = json!("dark");
    app.sample["settings"]["accounts"][0]["name"] =
        json!("여러 프로젝트에서 사용하는 아주 긴 계정 이름 · 한글과 English 표시 확인");
    app.sample["settings"]["accounts"][2]["enabled"] = json!(false);
    app.sample["snapshots"]
        .as_object_mut()
        .unwrap()
        .remove(id(1));
    app.sample["errors"] = json!({(id(0)):"offline"});
    app.sample["loading"] = json!([id(1)]);
    open(app)?;
    capture(app, "tray-menu-states-dark")?;
    let menu = app.menu.hwnd();
    send(menu, WM_KEYDOWN, 0x23, 0);
    send(menu, WM_KEYDOWN, 0x26, 0);
    app.sample["snapshots"][id(0)]["windows"] =
        json!([{"title":"5시간","remaining":80.5,"resetAt":null}]);
    app.menu.update(&app.sample);
    check(
        app.menu.visible() && children(menu).iter().any(|(s, _)| s.contains("81%")),
        "Live menu did not update",
    )?;
    app.menu.close(false);
    app.sample = original.clone();
    app.sample["settings"]["appearance"] = json!("dark");
    app.sample["snapshots"] = json!({});
    let mut many = Vec::new();
    for n in 0..15 {
        let mut account = accounts[n % accounts.len()].clone();
        let key = Uuid::new_v4().to_string();
        account["id"] = json!(key);
        account["name"] = json!(format!("스크롤 확인 계정 {}", n + 1));
        app.sample["snapshots"][&key] = original["snapshots"][id(0)].clone();
        many.push(account);
    }
    app.sample["settings"]["accounts"] = json!(many);
    open(app)?;
    let up = button(menu, "위로 스크롤")?;
    let down = button(menu, "아래로 스크롤")?;
    check(
        visible(up) && !enabled(up) && enabled(down),
        "Initial overflow arrows incorrect",
    )?;
    let first = children(menu)
        .into_iter()
        .find(|(s, _)| s.contains("스크롤 확인 계정 1."))
        .ok_or("First account missing")?
        .1;
    let top = rect(first).top;
    send(down, BM_CLICK, 0, 0);
    check(
        rect(first).top < top && enabled(up),
        "Down arrow did not scroll",
    )?;
    send(up, BM_CLICK, 0, 0);
    check(
        rect(first).top == top && !enabled(up),
        "Up arrow did not return",
    )?;
    send(menu, WM_KEYDOWN, 0x23, 0);
    capture(app, "tray-menu-overflow-dark")?;
    check(
        contains(area(menu)?, rect(menu))
            && contains(rect(menu), rect(button(menu, "종료")?))
            && !enabled(down)
            && enabled(up),
        "End did not reveal the last row",
    )?;
    send(menu, WM_KEYDOWN, 0x24, 0);
    check(
        visible(first) && contains(rect(menu), rect(first)),
        "Home did not reveal the first row",
    )?;
    send(menu, WM_KEYDOWN, 0x1b, 0);
    app.events();
    check(!app.menu.visible(), "Escape did not dismiss menu")?;

    let actions = Rc::new(RefCell::new(Vec::new()));
    let observed = actions.clone();
    let tests = Menu::with_callback(move |action| observed.borrow_mut().push(action))
        .map_err(|e| e.to_string())?;
    for name in ["Dashboard 열기", "종료"] {
        tests.update(&original);
        tests.show(40, 40);
        let mut buttons: Vec<_> = children(tests.hwnd())
            .into_iter()
            .filter(|(s, h)| visible(*h) && !s.contains("Dashboard에서 보기"))
            .map(|(s, _)| s)
            .collect();
        buttons.sort();
        check(
            buttons == ["Dashboard 열기", "종료"],
            "Unexpected actions in menu",
        )?;
        send(button(tests.hwnd(), name)?, BM_CLICK, 0, 0);
        tests.close(false);
    }
    check(*actions.borrow() == [1, 2], "Menu action dispatch failed")?;
    app.sample = original;
    Ok(
        json!({"leftClickIgnored":true,"leftDoubleClickIgnored":true,"rightClickOpens":true,"escapeDismisses":true,"overflowReachable":true,
        "liveUpdate":true,"actionsDispatch":true,"dpi":unsafe{GetDpiForWindow(menu)},"captures":captures}),
    )
}

fn process_metrics() -> Result<(usize, usize, Vec<String>)> {
    unsafe {
        let process = GetCurrentProcess();
        let mut info = PROCESS_MEMORY_COUNTERS_EX::default();
        let bytes = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        check(
            K32GetProcessMemoryInfo(
                process,
                (&mut info as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
                bytes,
            ) != 0,
            "Process memory unavailable",
        )?;
        let mut handles = vec![null_mut(); 128];
        let mut size = 0;
        loop {
            let capacity =
                (handles.len() * size_of::<windows_sys::Win32::Foundation::HMODULE>()) as u32;
            check(
                K32EnumProcessModules(process, handles.as_mut_ptr(), capacity, &mut size) != 0,
                "Cannot inspect loaded modules",
            )?;
            let count = size as usize / size_of::<windows_sys::Win32::Foundation::HMODULE>();
            if size <= capacity {
                handles.truncate(count);
                break;
            }
            handles.resize(count, null_mut());
        }
        let mut names = Vec::new();
        for handle in handles {
            let mut name = [0u16; 260];
            let n = K32GetModuleBaseNameW(process, handle, name.as_mut_ptr(), name.len() as u32);
            check(n != 0, "Cannot read loaded module name")?;
            names.push(String::from_utf16_lossy(&name[..n as usize]).to_lowercase());
        }
        Ok((info.PrivateUsage, info.WorkingSetSize, names))
    }
}

fn capture(
    app: &mut App,
    dir: &Path,
    route: &str,
    name: &str,
    width: i32,
    height: i32,
    captures: &mut Vec<String>,
) -> Result<()> {
    app.publish();
    let path = dir
        .join(format!("{name}.png"))
        .to_string_lossy()
        .into_owned();
    app.captured = None;
    app.send(
        "capture",
        &json!({"route":route,"path":path,"width":width,"height":height}),
        &Value::Null,
    );
    wait(app, 10, &format!("capturing {name}"), |a| {
        a.captured.as_deref() == Some(path.as_str())
    })?;
    captures.push(name.into());
    Ok(())
}

pub(super) fn run(app: &mut App, path: &Path) -> Result<()> {
    let path = std::path::absolute(path).map_err(|e| e.to_string())?;
    let dir = path.parent().ok_or("Missing output directory")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    tray_capture::samples(dir).map_err(|e| e.to_string())?;
    let menu = tray(app, dir)?;
    let mut captures: Vec<String> = serde_json::from_value(menu["captures"].clone()).unwrap();
    let mut pids = vec![];
    let mut routes = vec![];
    let exe = app.folder.join("Dashboard/QuotaBar.Dashboard.exe");
    for (theme, requested) in [("light", "accounts"), ("dark", "settings")] {
        app.dispatcher
            .dashboard
            .show(&exe, None, "dashboard")
            .map_err(|e| e.to_string())?;
        let pid = app.dispatcher.dashboard.pid();
        app.dispatcher
            .dashboard
            .show(&exe, None, requested)
            .map_err(|e| e.to_string())?;
        wait(app, 10, "opening last route before ready", |a| {
            a.dispatcher.dashboard.ready() && a.route == requested
        })?;
        routes.push(app.route.clone());
        app.dispatcher
            .dashboard
            .show(&exe, None, "dashboard")
            .map_err(|e| e.to_string())?;
        wait(app, 10, "navigating the running Dashboard", |a| {
            a.route == "dashboard"
        })?;
        check(
            pid != 0 && app.dispatcher.dashboard.pid() == pid,
            "Navigation replaced the Dashboard",
        )?;
        pids.push(pid);
        app.sample["settings"]["appearance"] = json!(theme);
        for route in [
            "dashboard",
            "activity",
            "accounts",
            "settings",
            "connect",
            "connect-claude",
        ] {
            capture(
                app,
                dir,
                route,
                &format!("{route}-{theme}"),
                1120,
                810,
                &mut captures,
            )?;
        }
        capture(
            app,
            dir,
            "accounts",
            &format!("accounts-narrow-{theme}"),
            900,
            660,
            &mut captures,
        )?;
        capture(
            app,
            dir,
            "dashboard",
            &format!("dashboard-wide-{theme}"),
            1320,
            900,
            &mut captures,
        )?;
        if theme == "dark" {
            let original = app.sample.clone();
            app.sample["settings"]["accounts"] = json!([]);
            app.sample["snapshots"] = json!({});
            app.sample["errors"] = json!({});
            capture(
                app,
                dir,
                "dashboard",
                "empty-dark",
                1120,
                810,
                &mut captures,
            )?;
            app.sample = original.clone();
            app.sample["settings"]["accounts"][0]["name"] =
                json!("여러 프로젝트에서 사용하는 긴 계정 이름 · 디자인 검증용 샘플 계정");
            app.sample["settings"]["accounts"][2]["enabled"] = json!(false);
            let id = original["settings"]["accounts"][0]["id"].as_str().unwrap();
            app.sample["errors"][id] =
                json!("서비스에 연결하지 못했습니다. 마지막으로 확인한 사용량을 표시합니다.");
            capture(
                app,
                dir,
                "dashboard",
                "states-dark",
                900,
                660,
                &mut captures,
            )?;
            app.sample = original;
        }
        app.dispatcher.dashboard.close();
        wait(app, 5, "closing the helper", |a| {
            a.dispatcher.dashboard.pid() == 0
        })?;
    }
    app.publish();
    let summary = display::summary(&app.sample);
    check(
        app.icon.visible()
            && summary.text == "16"
            && summary.tooltip.contains("16%")
            && !summary.use_logo,
        "Numeric tray missing after Dashboard closed",
    )?;
    let (private, working, modules) = process_metrics()?;
    let managed = modules.iter().any(|s| {
        matches!(
            s.as_str(),
            "coreclr.dll" | "clr.dll" | "hostfxr.dll" | "clrjit.dll"
        )
    });
    check(!managed, "Managed runtime was loaded in the native monitor")?;
    let report = json!({"passed":true,"captures":captures,"trayMenu":menu,"dashboardPids":pids,"startupRoutes":routes,"dashboardRunning":false,
        "trayText":summary.text,"trayVisibleAfterDashboardClose":app.icon.visible(),"trayPixelSize":app.icon_size,
        "monitorPid":std::process::id(),"privateBytes":private,"workingSet":working,"clrLoadedInMonitor":managed,
        "wpfLoadedInMonitor":modules.iter().any(|s|s.starts_with("presentationframework")),
        "windowsFormsLoadedInMonitor":modules.iter().any(|s|s.starts_with("system.windows.forms"))});
    fs::write(&path, report.to_string()).map_err(|e| e.to_string())
}
