use std::{env, process::ExitCode};

fn main() -> ExitCode {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.len() == 2 && arguments[0] == "--apply-update" {
        #[cfg(target_os = "macos")]
        use quotabar::updates::macos::apply;
        #[cfg(windows)]
        use quotabar::updates::windows::apply;
        return if apply(std::path::Path::new(&arguments[1])).is_ok() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    match quotabar::claude::run(&arguments) {
        Ok(status) => std::process::exit(status),
        Err(_) => ExitCode::FAILURE,
    }
}
