use super::prevent_window_drag;
use std::ptr::{null, null_mut};
use windows_sys::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        UI::{
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    },
    core::w,
};

#[test]
fn caption_drag_and_system_move_are_blocked_while_resize_is_available() {
    // An invisible, process-local window exercises real Win32 messages without
    // creating a monitor, taskbar button, or notification-area registration.
    unsafe extern "system" fn observe(
        window: HWND,
        message: u32,
        w: WPARAM,
        l: LPARAM,
        id: usize,
        _: usize,
    ) -> LRESULT {
        unsafe {
            if matches!(message, WM_NCLBUTTONDOWN | WM_SYSCOMMAND) {
                return 123;
            }
            if message == WM_NCDESTROY {
                RemoveWindowSubclass(window, Some(observe), id);
            }
            DefSubclassProc(window, message, w, l)
        }
    }
    struct Window(HWND);
    impl Drop for Window {
        fn drop(&mut self) {
            unsafe {
                DestroyWindow(self.0);
            }
        }
    }
    unsafe {
        let window = Window(CreateWindowExW(
            0,
            w!("STATIC"),
            w!("QuotaBar window policy"),
            WS_OVERLAPPEDWINDOW,
            80,
            90,
            450,
            300,
            null_mut(),
            null_mut(),
            null_mut(),
            null(),
        ));
        assert!(!window.0.is_null());
        assert_ne!(SetWindowSubclass(window.0, Some(observe), 99, 0), 0);
        assert!(prevent_window_drag(window.0));
        assert_eq!(
            SendMessageW(window.0, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0),
            0
        );
        assert_eq!(
            SendMessageW(window.0, WM_SYSCOMMAND, SC_MOVE as usize | 2, 0),
            0
        );
        assert_eq!(
            SendMessageW(window.0, WM_NCLBUTTONDOWN, HTBOTTOMRIGHT as usize, 0),
            123
        );
        assert_eq!(
            SendMessageW(window.0, WM_SYSCOMMAND, SC_SIZE as usize, 0),
            123
        );
        let mut before = RECT::default();
        assert_ne!(GetWindowRect(window.0, &mut before), 0);
        assert_ne!(
            SetWindowPos(
                window.0,
                null_mut(),
                0,
                0,
                before.right - before.left + 50,
                before.bottom - before.top + 30,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE
            ),
            0
        );
        let mut after = RECT::default();
        assert_ne!(GetWindowRect(window.0, &mut after), 0);
        assert_eq!((after.left, after.top), (before.left, before.top));
        assert_eq!(
            (after.right - after.left, after.bottom - after.top),
            (
                before.right - before.left + 50,
                before.bottom - before.top + 30
            )
        );
        assert_eq!(IsWindowVisible(window.0), 0);
    }
}
