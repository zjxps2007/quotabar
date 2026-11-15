//! Per-monitor-DPI shell icon and its hidden message window. All methods and the
//! callback run on the UI thread owning the Win32 message pump.
use crate::tray_icon::{ShellIcon, shell_icon};
use std::{
    cell::RefCell,
    ffi::c_void,
    io,
    ptr::{null, null_mut},
    sync::OnceLock,
};
use windows_sys::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        System::{
            LibraryLoader::GetModuleHandleW,
            Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
        },
        UI::{
            HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
            Shell::*,
            WindowsAndMessaging::*,
        },
    },
    core::w,
};

pub const CALLBACK_MESSAGE: u32 = WM_APP + 24;
const CLASS: *const u16 = w!("QuotaBar.Native.NotificationWindow");
static WINDOW_CLASS: OnceLock<Result<u16, i32>> = OnceLock::new();
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();
pub type MenuCallback = extern "C" fn(i32, i32);

pub struct NotificationIcon {
    window: HWND,
    callback: Box<dyn Fn(i32, i32)>,
    display: RefCell<Display>,
}

#[derive(Default)]
struct Display {
    icon: Option<ShellIcon>,
    key: Option<(String, bool, bool, i32, bool)>,
    tooltip: String,
    added: bool,
    version_four: bool,
}

fn icon_data(window: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: 1,
        ..Default::default()
    }
}
impl Display {
    fn sync(&mut self, window: HWND) {
        let Some(icon) = &self.icon else { return };
        let mut data = icon_data(window);
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = CALLBACK_MESSAGE;
        data.hIcon = icon.0;
        for (out, c) in data
            .szTip
            .iter_mut()
            .take(127)
            .zip(self.tooltip.encode_utf16())
        {
            *out = c;
        }
        unsafe {
            if self.added && Shell_NotifyIconW(NIM_MODIFY, &data) != 0 {
                return;
            }
            self.added = Shell_NotifyIconW(NIM_ADD, &data) != 0;
            if self.added {
                data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                self.version_four = Shell_NotifyIconW(NIM_SETVERSION, &data) != 0;
            }
        }
    }
}

impl NotificationIcon {
    pub fn create(callback: MenuCallback) -> io::Result<Box<Self>> {
        Self::with_callback(move |x, y| callback(x, y))
    }
    pub fn with_callback(callback: impl Fn(i32, i32) + 'static) -> io::Result<Box<Self>> {
        unsafe {
            let instance = GetModuleHandleW(null());
            let class = WINDOW_CLASS.get_or_init(|| {
                let cls = WNDCLASSW {
                    hInstance: instance,
                    lpszClassName: CLASS,
                    lpfnWndProc: Some(window_proc),
                    ..Default::default()
                };
                let atom = RegisterClassW(&cls);
                if atom == 0 {
                    Err(io::Error::last_os_error().raw_os_error().unwrap_or(1))
                } else {
                    Ok(atom)
                }
            });
            if let Err(code) = class {
                return Err(io::Error::from_raw_os_error(*code));
            }
            TASKBAR_CREATED.get_or_init(|| RegisterWindowMessageW(w!("TaskbarCreated")));
            // Message-only windows cannot receive the broadcast when Explorer restarts.
            let window = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                CLASS,
                w!("QuotaBar"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            if window.is_null() {
                return Err(io::Error::last_os_error());
            }
            let host = Box::new(Self {
                window,
                callback: Box::new(callback),
                display: RefCell::new(Display::default()),
            });
            SetWindowLongPtrW(window, GWLP_USERDATA, (&*host as *const Self) as isize);
            Ok(host)
        }
    }
    pub fn window(&self) -> HWND {
        self.window
    }
    pub fn visible(&self) -> bool {
        self.display.borrow().added
    }
    pub fn update(&self, text: &str, logo: bool, warning: bool, tooltip: &str) -> io::Result<i32> {
        let (size, light) = appearance();
        let mut display = self.display.borrow_mut();
        let key = (text.to_owned(), logo, warning, size, light);
        let changed = display.key.as_ref() != Some(&key);
        if changed {
            display.icon = Some(shell_icon(text, logo, warning, size, light)?);
            display.key = Some(key);
        }
        if changed || display.tooltip != tooltip || !display.added {
            display.tooltip = tooltip.to_owned();
            display.sync(self.window);
        }
        Ok(size)
    }
    pub fn return_focus(&self) {
        if self.visible() {
            unsafe {
                Shell_NotifyIconW(NIM_SETFOCUS, &icon_data(self.window));
            }
        }
    }
}
impl Drop for NotificationIcon {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.window, GWLP_USERDATA, 0);
            if self.display.get_mut().added {
                Shell_NotifyIconW(NIM_DELETE, &icon_data(self.window));
            }
            DestroyWindow(self.window);
        }
    }
}

fn appearance() -> (i32, bool) {
    unsafe {
        let dpi = GetDpiForWindow(FindWindowW(w!("Shell_TrayWnd"), null()));
        let size = if dpi == 0 {
            GetSystemMetrics(SM_CXSMICON)
        } else {
            GetSystemMetricsForDpi(SM_CXSMICON, dpi)
        };
        let mut mode = 0_u32;
        let mut bytes = 4;
        let result = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut mode as *mut u32).cast::<c_void>(),
            &mut bytes,
        );
        (size.clamp(16, 64), result == 0 && mode == 1)
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        let state = (GetWindowLongPtrW(window, GWLP_USERDATA) as *const NotificationIcon).as_ref();
        if let Some(state) = state {
            if msg == CALLBACK_MESSAGE {
                let Ok(display) = state.display.try_borrow() else {
                    return 0;
                };
                let modern = display.version_four;
                drop(display);
                let event = (lparam as u32) & 0xffff;
                if (modern && event == WM_CONTEXTMENU) || (!modern && event == WM_RBUTTONUP) {
                    let mut point = POINT {
                        x: wparam as i16 as i32,
                        y: (wparam >> 16) as i16 as i32,
                    };
                    if !modern || (point.x == -1 && point.y == -1) {
                        GetCursorPos(&mut point);
                    }
                    SetForegroundWindow(window);
                    (state.callback)(point.x, point.y);
                    PostMessageW(window, WM_NULL, 0, 0);
                }
                return 0;
            }
            if msg != 0 && Some(&msg) == TASKBAR_CREATED.get() {
                if let Ok(mut d) = state.display.try_borrow_mut() {
                    d.added = false;
                    d.sync(window);
                }
                return 0;
            }
        }
        DefWindowProcW(window, msg, wparam, lparam)
    }
}
