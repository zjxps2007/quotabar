#[cfg(windows)]
fn prevent_window_drag(hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        UI::{
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    };
    unsafe extern "system" fn fixed_position(
        hwnd: HWND,
        message: u32,
        w: WPARAM,
        l: LPARAM,
        id: usize,
        _: usize,
    ) -> LRESULT {
        unsafe {
            if (message == WM_NCLBUTTONDOWN && w == HTCAPTION as usize)
                || (message == WM_SYSCOMMAND && w & 0xfff0 == SC_MOVE as usize)
            {
                return 0;
            }
            if message == WM_NCDESTROY {
                RemoveWindowSubclass(hwnd, Some(fixed_position), id);
            }
            DefSubclassProc(hwnd, message, w, l)
        }
    }
    unsafe { SetWindowSubclass(hwnd, Some(fixed_position), 0x51424d56, 0) != 0 }
}

#[cfg(windows)]
pub fn constrain_dashboard_window(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        UI::{
            HiDpi::GetDpiForWindow,
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    };

    unsafe extern "system" fn sizing(
        hwnd: HWND,
        message: u32,
        w: WPARAM,
        l: LPARAM,
        id: usize,
        _: usize,
    ) -> LRESULT {
        unsafe {
            if message == WM_NCDESTROY {
                RemoveWindowSubclass(hwnd, Some(sizing), id);
            }
            let result = DefSubclassProc(hwnd, message, w, l);
            if matches!(message, WM_GETMINMAXINFO | WM_WINDOWPOSCHANGING) && l != 0 {
                // The original Window.MinWidth/MinHeight are outer dimensions
                // in DIPs. SetWindowPos can otherwise bypass winit's drag limit.
                let dpi = GetDpiForWindow(hwnd) as i32;
                if dpi > 0 {
                    let width = ((900 * dpi + 48) / 96).min(GetSystemMetrics(SM_CXMAXTRACK));
                    let height = ((660 * dpi + 48) / 96).min(GetSystemMetrics(SM_CYMAXTRACK));
                    if message == WM_GETMINMAXINFO {
                        let info = &mut *(l as *mut MINMAXINFO);
                        info.ptMinTrackSize.x = width;
                        info.ptMinTrackSize.y = height;
                    } else {
                        let position = &mut *(l as *mut WINDOWPOS);
                        if position.flags & SWP_NOSIZE == 0
                            && IsIconic(hwnd) == 0
                            && IsZoomed(hwnd) == 0
                        {
                            position.cx = position.cx.max(width);
                            position.cy = position.cy.max(height);
                        }
                    }
                }
            }
            result
        }
    }
    if let Ok(handle) = cc.window_handle()
        && let RawWindowHandle::Win32(window) = handle.as_raw()
    {
        unsafe { SetWindowSubclass(window.hwnd.get() as HWND, Some(sizing), 1, 0) };
        prevent_window_drag(window.hwnd.get() as HWND);
    }
}

#[cfg(windows)]
pub fn choose_path(frame: &eframe::Frame, provider: &str, _current: &str) -> Option<String> {
    use raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, Win32WindowHandle,
        WindowHandle,
    };
    use std::num::NonZeroIsize;
    struct Parent(NonZeroIsize);
    impl HasDisplayHandle for Parent {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            Ok(DisplayHandle::windows())
        }
    }
    impl HasWindowHandle for Parent {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            // The connection dialog owns this HWND on this thread until the modal picker returns.
            Ok(unsafe { WindowHandle::borrow_raw(Win32WindowHandle::new(self.0).into()) })
        }
    }
    let owner = NonZeroIsize::new(dialog_hwnd(frame, "QuotaBar · 계정 연결")? as isize)?;
    let parent = Parent(owner);
    let picker = rfd::FileDialog::new().set_parent(&parent);
    let path = if provider == "Claude" {
        picker.set_title("Claude 설정 폴더").pick_folder()
    } else {
        picker.add_filter("JSON 로그인 파일", &["json"]).pick_file()
    }?;
    Some(path.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
pub fn choose_path(_frame: &eframe::Frame, provider: &str, current: &str) -> Option<String> {
    use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
    use objc2_foundation::{NSString, NSURL};
    let panel = NSOpenPanel::openPanel(objc2::MainThreadMarker::new()?);
    panel.setCanChooseDirectories(provider == "Claude");
    panel.setCanChooseFiles(provider != "Claude");
    panel.setAllowsMultipleSelection(false);
    panel.setShowsHiddenFiles(true);
    let path = NSString::from_str(current).stringByExpandingTildeInPath();
    let directory = NSURL::fileURLWithPath(&path).URLByDeletingLastPathComponent();
    panel.setDirectoryURL(directory.as_deref());
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    Some(panel.URL()?.path()?.to_string())
}

#[cfg(windows)]
pub use quotabar::windows_format::{grouped, month_day_time};

#[cfg(target_os = "macos")]
pub fn grouped(value: u64) -> String {
    use objc2_foundation::{NSNumber, NSNumberFormatter, NSNumberFormatterStyle};
    NSNumberFormatter::localizedStringFromNumber_numberStyle(
        &NSNumber::numberWithUnsignedLongLong(value),
        NSNumberFormatterStyle::DecimalStyle,
    )
    .to_string()
}

#[cfg(target_os = "macos")]
pub fn activity_date(date: chrono::NaiveDate, template: &str) -> String {
    use chrono::{Local, TimeZone};
    use objc2_foundation::{NSDate, NSDateFormatter, NSString};
    let noon = date.and_hms_opt(12, 0, 0).unwrap();
    let Some(local) = Local.from_local_datetime(&noon).earliest() else {
        return date.to_string();
    };
    let formatter = NSDateFormatter::new();
    formatter.setLocalizedDateFormatFromTemplate(&NSString::from_str(template));
    formatter
        .stringFromDate(&NSDate::dateWithTimeIntervalSince1970(
            local.timestamp() as f64
        ))
        .to_string()
}

#[cfg(target_os = "macos")]
pub fn quota_time(date: chrono::DateTime<chrono::Local>, with_date: bool) -> String {
    use objc2_foundation::{NSDate, NSDateFormatter, NSDateFormatterStyle};
    let date_style = if with_date {
        NSDateFormatterStyle::MediumStyle
    } else {
        NSDateFormatterStyle::NoStyle
    };
    let date = NSDate::dateWithTimeIntervalSince1970(date.timestamp() as f64);
    NSDateFormatter::localizedStringFromDate_dateStyle_timeStyle(
        &date,
        date_style,
        NSDateFormatterStyle::ShortStyle,
    )
    .to_string()
}

#[cfg(windows)]
pub fn confirm_remove(
    frame: &eframe::Frame,
    account: &crate::model::Account,
    finish: impl Fn(bool) + 'static,
) {
    use windows_sys::{
        Win32::UI::WindowsAndMessaging::{IDOK, MB_OKCANCEL, MessageBoxW},
        core::w,
    };
    let Some(owner) = crate::hwnd(frame) else {
        finish(false);
        return;
    };
    let mut text = format!("{} 연결을 해제할까요?", account.name);
    if account.provider == "Claude" {
        text.push_str("\nQuotaBar가 설치한 status line은 기존 설정으로 복원합니다.");
    }
    let message: Vec<_> = text.encode_utf16().chain(Some(0)).collect();
    finish(unsafe { MessageBoxW(owner, message.as_ptr(), w!("QuotaBar"), MB_OKCANCEL) } == IDOK);
}

#[cfg(target_os = "macos")]
pub fn confirm_remove(
    frame: &eframe::Frame,
    account: &crate::model::Account,
    finish: impl Fn(bool) + 'static,
) {
    use objc2_app_kit::{NSAlert, NSAlertSecondButtonReturn, NSView};
    use objc2_foundation::NSString;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let owner = frame
        .window_handle()
        .ok()
        .and_then(|handle| match handle.as_raw() {
            RawWindowHandle::AppKit(h) => unsafe { h.ns_view.cast::<NSView>().as_ref() }.window(),
            _ => None,
        });
    let (Some(owner), Some(main)) = (owner, objc2::MainThreadMarker::new()) else {
        finish(false);
        return;
    };
    let alert = NSAlert::new(main);
    alert.setMessageText(&NSString::from_str(&format!(
        "{} 계정을 삭제할까요?",
        account.name
    )));
    alert.setInformativeText(&NSString::from_str(if account.provider == "Claude" {
        "이 앱의 프로필과 스냅샷을 지우고 연결한 statusLine을 복구합니다. CLI 로그인은 유지됩니다."
    } else {
        "이 앱에 저장한 로그인과 모니터링 정보를 삭제합니다. 원본 CLI 로그인 파일은 유지됩니다."
    }));
    let cancel = alert.addButtonWithTitle(&NSString::from_str("취소"));
    cancel.setKeyEquivalent(&NSString::from_str("\u{1b}"));
    let remove = alert.addButtonWithTitle(&NSString::from_str("삭제"));
    remove.setHasDestructiveAction(true);
    let done = block2::RcBlock::new(move |response| finish(response == NSAlertSecondButtonReturn));
    alert.beginSheetModalForWindow_completionHandler(&owner, Some(&done));
}

#[cfg(windows)]
pub fn activate_dashboard(frame: &eframe::Frame) {
    use windows_sys::Win32::UI::{
        Input::KeyboardAndMouse::IsWindowEnabled, WindowsAndMessaging::*,
    };
    let Some(owner) = crate::hwnd(frame) else {
        return;
    };
    unsafe {
        if IsIconic(owner) != 0 {
            ShowWindow(owner, SW_RESTORE);
        } else if IsWindowVisible(owner) == 0 {
            ShowWindow(owner, SW_SHOW);
        }
        let popup = GetLastActivePopup(owner);
        let target =
            if IsWindowEnabled(owner) == 0 && !popup.is_null() && IsWindowVisible(popup) != 0 {
                popup
            } else {
                owner
            };
        SetForegroundWindow(target);
    }
}

#[cfg(windows)]
pub fn dialog_hwnd(
    frame: &eframe::Frame,
    title: &str,
) -> Option<windows_sys::Win32::Foundation::HWND> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::{System::Threading::GetCurrentThreadId, UI::WindowsAndMessaging::*};
    let name: Vec<_> = title.encode_utf16().chain(Some(0)).collect();
    let owner = crate::hwnd(frame)?;
    let mut handle = null_mut();
    loop {
        handle = unsafe { FindWindowExW(null_mut(), handle, null(), name.as_ptr()) };
        if handle.is_null() {
            return None;
        }
        if unsafe { GetWindowThreadProcessId(handle, null_mut()) == GetCurrentThreadId() }
            && handle != owner
        {
            return Some(handle);
        }
    }
}
#[cfg(windows)]
pub fn dialog_owner(frame: &eframe::Frame, title: &str, open: bool) {
    use windows_sys::Win32::UI::{
        Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled},
        WindowsAndMessaging::*,
    };
    let Some(owner) = crate::hwnd(frame) else {
        return;
    };
    if title.is_empty() {
        return;
    }
    unsafe {
        if !open {
            if IsWindowEnabled(owner) == 0 {
                EnableWindow(owner, 1);
                SetForegroundWindow(owner);
            }
            return;
        }
        let Some(window) = dialog_hwnd(frame, title) else {
            return;
        };
        prevent_window_drag(window);
        if GetWindowLongPtrW(window, GWLP_HWNDPARENT) != owner as isize {
            SetWindowLongPtrW(window, GWLP_HWNDPARENT, owner as isize);
        }
        if IsWindowEnabled(owner) != 0 {
            EnableWindow(owner, 0);
            SetForegroundWindow(window);
        }
    }
}
#[cfg(target_os = "macos")]
pub fn dialog_owner(frame: &eframe::Frame, title: &str, open: bool) {
    use objc2_app_kit::{NSApplication, NSView};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if title.is_empty() {
        return;
    }
    let Ok(handle) = frame.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };
    let view = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    let Some(owner) = view.window() else { return };
    let Some(m) = objc2::MainThreadMarker::new() else {
        return;
    };
    if open {
        for child in NSApplication::sharedApplication(m).windows().iter() {
            if child != owner && child.title().to_string() == title {
                child.setMovable(false);
                child.setMovableByWindowBackground(false);
                if owner.attachedSheet().is_none() {
                    owner.beginSheet_completionHandler(&child, None);
                }
                break;
            }
        }
    } else if let Some(sheet) = owner.attachedSheet() {
        owner.endSheet(&sheet);
    }
}
#[cfg(target_os = "macos")]
pub fn capture_backing_size(
    frame: &eframe::Frame,
    requested_height: f32,
) -> Option<([usize; 2], bool)> {
    use objc2_app_kit::NSView;
    use objc2_foundation::NSSize;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let RawWindowHandle::AppKit(handle) = frame.window_handle().ok()?.as_raw() else {
        return None;
    };
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let window = view.window()?;
    let available = window
        .contentRectForFrameRect(window.screen()?.visibleFrame())
        .size
        .height;
    let requested = f64::from(requested_height);
    let height = requested.min(available).max(window.contentMinSize().height);
    let size = view.bounds().size;
    if size.height > height + 0.5 / window.backingScaleFactor() {
        window.setContentSize(NSSize::new(size.width, height));
    }
    // WindowServer can still expose the old view bounds after setContentSize.
    let mut bounds = view.bounds();
    bounds.size.height = bounds.size.height.min(height);
    let pixels = view.convertRectToBacking(bounds).size;
    Some((
        [
            pixels.width.round() as usize,
            pixels.height.round() as usize,
        ],
        height < requested,
    ))
}

#[cfg(target_os = "macos")]
pub fn dashboard_titlebar_height(frame: &eframe::Frame) -> i8 {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Some(RawWindowHandle::AppKit(handle)) = frame.window_handle().ok().map(|h| h.as_raw())
    else {
        return 0;
    };
    let view = unsafe { handle.ns_view.cast::<objc2_app_kit::NSView>().as_ref() };
    let Some(window) = view.window() else {
        return 0;
    };
    let layout = window.contentLayoutRect();
    (view.bounds().size.height - (layout.origin.y + layout.size.height))
        .round()
        .clamp(0., 80.) as i8
}

#[cfg(target_os = "macos")]
pub fn restore_window(cc: &eframe::CreationContext<'_>) {
    use objc2_app_kit::{NSUserInterfaceItemIdentification, NSView};
    use objc2_foundation::NSString;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = cc.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };
    let view = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    if let Some(window) = view.window() {
        window.setMovable(false);
        window.setMovableByWindowBackground(false);
        let name = NSString::from_str("DashboardWindow");
        window.setIdentifier(Some(&NSString::from_str("dashboard")));
        if !window.setFrameUsingName(&name) {
            window.center();
        }
        window.setFrameAutosaveName(&name);
    }
}

#[cfg(all(test, windows))]
#[path = "platform/tests.rs"]
mod tests;
