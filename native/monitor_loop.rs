//! Windows monitor dispatch, system broadcasts and one-shot refresh/update timers.
use crate::dashboard::windows::host::Host;
use std::{
    cell::Cell,
    io,
    ptr::{null, null_mut},
    sync::OnceLock,
};
use windows_sys::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::*,
    },
    core::w,
};

const CLASS: *const u16 = w!("QuotaBar.Native.MonitorLoop");
const WAKE: u32 = WM_APP + 37;
const DASHBOARD: u32 = WM_APP + 38;
const UPDATE_WORK: u32 = WM_APP + 39;
static REGISTERED: OnceLock<Result<u16, i32>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Event {
    Work,
    Refresh,
    Update,
    Suspend,
    Resume,
    PowerStatus,
    Display,
    Dashboard,
    UpdateWork,
}

pub struct MonitorLoop {
    window: HWND,
    pub dashboard: Host,
    callback: Box<dyn Fn(Event)>,
    timers: [Cell<usize>; 2],
    next_timer: Cell<usize>,
}

impl MonitorLoop {
    pub fn new(callback: impl Fn(Event) + 'static) -> io::Result<Box<Self>> {
        unsafe {
            let instance = GetModuleHandleW(null());
            if let Err(e) = REGISTERED.get_or_init(|| {
                let class = WNDCLASSW {
                    hInstance: instance,
                    lpfnWndProc: Some(window_proc),
                    lpszClassName: CLASS,
                    ..Default::default()
                };
                let atom = RegisterClassW(&class);
                if atom != 0 {
                    Ok(atom)
                } else {
                    Err(io::Error::last_os_error().raw_os_error().unwrap_or(1))
                }
            }) {
                return Err(io::Error::from_raw_os_error(*e));
            }
            // A top-level, invisible window receives the power, display and theme broadcasts.
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
            let state = Box::new(Self {
                window,
                dashboard: Host::new(window as isize),
                callback: Box::new(callback),
                timers: Default::default(),
                next_timer: Cell::new(1),
            });
            SetWindowLongPtrW(window, GWLP_USERDATA, (&*state as *const Self) as isize);
            Ok(state)
        }
    }

    pub fn window(&self) -> HWND {
        self.window
    }

    pub fn run(&self) -> io::Result<()> {
        unsafe {
            let mut message = MSG::default();
            loop {
                match GetMessageW(&mut message, null_mut(), 0, 0) {
                    -1 => return Err(io::Error::last_os_error()),
                    0 => return Ok(()),
                    _ => {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
            }
        }
    }

    /// Post from any thread while the owner keeps the HWND alive.
    pub fn wake(window: isize) -> bool {
        window != 0 && unsafe { PostMessageW(window as HWND, WAKE, 0, 0) != 0 }
    }
    pub(crate) fn dashboard_input(window: isize) -> bool {
        window != 0 && unsafe { PostMessageW(window as HWND, DASHBOARD, 0, 0) != 0 }
    }
    pub fn update_input(window: isize) -> bool {
        window != 0 && unsafe { PostMessageW(window as HWND, UPDATE_WORK, 0, 0) != 0 }
    }
    pub fn quit(&self) {
        unsafe {
            PostQuitMessage(0);
        }
    }

    pub fn timer(&self, index: usize, delay_ms: u32) -> io::Result<()> {
        let slot = self.timers.get(index).ok_or(io::ErrorKind::InvalidInput)?;
        let previous = slot.replace(0);
        if previous != 0 {
            unsafe {
                KillTimer(self.window, previous);
            }
        }
        if delay_ms != 0 {
            // KillTimer can leave an already posted WM_TIMER in the queue. Give each deadline its own ID.
            let id = self.next_timer.get();
            self.next_timer.set(id.wrapping_add(1).max(1));
            let actual = unsafe { SetTimer(self.window, id, delay_ms, None) };
            if actual == 0 {
                return Err(io::Error::last_os_error());
            }
            slot.set(id);
        }
        Ok(())
    }
}

impl Drop for MonitorLoop {
    fn drop(&mut self) {
        self.dashboard.shutdown();
        unsafe {
            SetWindowLongPtrW(self.window, GWLP_USERDATA, 0);
            DestroyWindow(self.window);
        }
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    unsafe {
        if let Some(state) =
            (GetWindowLongPtrW(window, GWLP_USERDATA) as *const MonitorLoop).as_ref()
        {
            let event = match message {
                WAKE => Some(Event::Work),
                DASHBOARD => Some(Event::Dashboard),
                UPDATE_WORK => Some(Event::UpdateWork),
                WM_TIMER => {
                    if let Some(index) = state
                        .timers
                        .iter()
                        .position(|slot| slot.get() == wp && wp != 0)
                    {
                        KillTimer(window, wp);
                        state.timers[index].set(0);
                        Some([Event::Refresh, Event::Update][index])
                    } else {
                        None
                    }
                }
                WM_POWERBROADCAST => match wp as u32 {
                    PBT_APMSUSPEND | PBT_APMSTANDBY => Some(Event::Suspend),
                    PBT_APMRESUMESUSPEND | PBT_APMRESUMECRITICAL | PBT_APMRESUMESTANDBY => {
                        Some(Event::Resume)
                    }
                    PBT_APMBATTERYLOW | PBT_APMOEMEVENT | PBT_APMPOWERSTATUSCHANGE => {
                        Some(Event::PowerStatus)
                    }
                    _ => None,
                },
                WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED | WM_DISPLAYCHANGE => {
                    Some(Event::Display)
                }
                _ => None,
            };
            if let Some(event) = event {
                (state.callback)(event);
            }
            if message == WAKE || message == WM_TIMER {
                return 0;
            }
        }
        DefWindowProcW(window, message, wp, lp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    unsafe extern "system" fn finish_test(_: HWND, _: u32, _: usize, _: u32) {
        unsafe {
            PostQuitMessage(0);
        }
    }

    #[test]
    fn owner_delivers_work_power_and_only_current_deadlines() {
        let received = Rc::new(RefCell::new(Vec::new()));
        let events = received.clone();
        let monitor = MonitorLoop::new(move |event| events.borrow_mut().push(event)).unwrap();
        let window = monitor.window();
        monitor.timer(0, 15).unwrap();
        monitor.timer(0, 0).unwrap();
        monitor.timer(1, 15).unwrap();
        assert!(MonitorLoop::wake(window as isize));
        unsafe {
            // The canceled timer's old message must not fire the replacement deadline.
            PostMessageW(window, WM_TIMER, 1, 0);
            for value in [
                PBT_APMSUSPEND,
                PBT_APMRESUMEAUTOMATIC,
                PBT_APMRESUMESUSPEND,
                PBT_APMPOWERSTATUSCHANGE,
            ] {
                PostMessageW(window, WM_POWERBROADCAST, value as usize, 0);
            }
            PostMessageW(window, WM_DISPLAYCHANGE, 0, 0);
            assert_ne!(SetTimer(window, 900, 150, Some(finish_test)), 0);
        }
        monitor.run().unwrap();
        assert_eq!(
            received.borrow().as_slice(),
            [
                Event::Work,
                Event::Suspend,
                Event::Resume,
                Event::PowerStatus,
                Event::Display,
                Event::Update
            ]
        );
    }
}
