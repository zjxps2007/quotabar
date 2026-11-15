//! AppKit's process lifetime, legacy helper entry points and application delegate.
mod presentation;
use crate::instance::Instance;
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::autoreleasepool,
    runtime::ProtocolObject,
};
use objc2_app_kit::{NSApplication, NSApplicationDelegate};
use objc2_foundation::{NSBundle, NSHomeDirectory, NSNotification, NSObject, NSObjectProtocol};
pub use presentation::{Callback, Presentation};
use std::{
    cell::OnceCell,
    env,
    ffi::{OsStr, OsString, c_void},
    os::unix::process::CommandExt,
    path::PathBuf,
    process::Command,
    rc::Rc,
};

struct Services {
    context: *mut c_void,
    callback: Callback,
    arguments: Vec<OsString>,
    presentation: OnceCell<Rc<Presentation>>,
}

define_class!(
    #[unsafe(super=NSObject)]
    #[thread_kind=MainThreadOnly]
    #[ivars=Services]
    #[name = "QuotaBarRustAppDelegate"]
    struct Delegate;
    unsafe impl NSObjectProtocol for Delegate {}
    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn launched(&self, _: &NSNotification) {
            let services = self.ivars();
            if services.presentation.get().is_some() {
                return;
            }
            let presentation = Presentation::new(
                self.mtm(),
                services.context,
                services.callback,
                &services.arguments,
            );
            let _ = services.presentation.set(presentation.clone());
            presentation.launch();
            crate::updates::macos::acknowledge_launch();
        }
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn close_window(&self, _: &NSApplication) -> bool {
            false
        }
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn reopen(&self, _: &NSApplication, visible: bool) -> bool {
            if !visible && let Some(presentation) = self.ivars().presentation.get() {
                presentation.reopen();
            }
            true
        }
        #[unsafe(method(applicationWillTerminate:))]
        fn terminating(&self, _: &NSNotification) {
            if let Some(presentation) = self.ivars().presentation.get() {
                presentation.terminate();
            }
        }
    }
);

fn helper(arguments: &[OsString]) -> Option<&'static str> {
    let first = arguments.first().map(OsString::as_os_str);
    if first == Some(OsStr::new("--claude-statusline"))
        || (arguments.len() == 2 && first == Some(OsStr::new("--apply-update")))
    {
        Some("Contents/MacOS/quotabar-statusline")
    } else if arguments.iter().any(|a| a == "--dashboard-helper") {
        Some("Contents/Helpers/QuotaBar Dashboard.app/Contents/MacOS/QuotaBar")
    } else {
        None
    }
}

pub fn run(context: *mut c_void, callback: Callback) -> i32 {
    let Some(main) = MainThreadMarker::new() else {
        return 1;
    };
    autoreleasepool(|_| {
        let mut argv = env::args_os();
        let executable = argv.next().unwrap_or_default();
        let arguments: Vec<_> = argv.collect();
        if let Some(helper) = helper(&arguments) {
            let path = PathBuf::from(NSBundle::mainBundle().bundlePath().to_string()).join(helper);
            let _ = Command::new(path).arg0(executable).args(arguments).exec();
            return 1;
        }
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        }
        let _instance = if arguments.iter().any(|a| a == "--demo") {
            None
        } else {
            let root = PathBuf::from(NSHomeDirectory().to_string())
                .join("Library/Application Support/AIUsage Lite");
            match Instance::acquire(&root) {
                Ok(instance) if instance.is_owner() => Some(instance),
                Ok(instance) => {
                    let _ = instance.activate();
                    return 0;
                }
                Err(_) => {
                    eprintln!(
                        "QuotaBar 실행 상태를 확인하지 못했습니다. 앱 저장 폴더의 권한을 확인하세요."
                    );
                    return 1;
                }
            }
        };
        let application = NSApplication::sharedApplication(main);
        let delegate: objc2::rc::Retained<Delegate> = unsafe {
            msg_send![
                super(Delegate::alloc(main).set_ivars(Services {
                    context,
                    callback,
                    arguments,
                    presentation: OnceCell::new()
                })),
                init
            ]
        };
        application.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        application.run();
        application.setDelegate(None);
        0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packaged_helper_switches_do_not_open_the_monitor() {
        let args = |items: &[&str]| items.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            helper(&args(&["--apply-update", "/a path/job"])),
            Some("Contents/MacOS/quotabar-statusline")
        );
        assert_eq!(helper(&args(&["--apply-update"])), None);
        assert_eq!(
            helper(&args(&["--claude-statusline", "--dashboard-helper"])),
            Some("Contents/MacOS/quotabar-statusline")
        );
        assert_eq!(
            helper(&args(&["--demo", "--dashboard-helper"])),
            Some("Contents/Helpers/QuotaBar Dashboard.app/Contents/MacOS/QuotaBar")
        );
        assert_eq!(helper(&args(&["--menu-bar-only", "--demo"])), None);
    }
}
