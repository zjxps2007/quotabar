use std::{fs, process::Command};

fn main() {
    let dashboard = std::env::var("CARGO_PKG_NAME").as_deref() == Ok("quotabar-ui");
    if dashboard {
        std::env::set_current_dir("../..").expect("Locate the shared application resources");
    }
    let root = std::env::current_dir().expect("Locate the repository");
    for watched in [
        "shared/version.json",
        "windows/assets/QuotaBar.ico",
        "windows/src/QuotaBar.Monitor/app.manifest",
    ] {
        println!("cargo:rerun-if-changed={}", root.join(watched).display());
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read("shared/version.json").unwrap()).unwrap();
    let version = metadata["version"]
        .as_str()
        .expect("Missing application version");
    println!("cargo:rustc-env=QUOTABAR_VERSION={version}");
    for args in [
        ["rev-parse", "--git-path", "HEAD"],
        ["rev-parse", "--git-path", "packed-refs"],
    ] {
        if let Some(path) = git(&args)
            && std::path::Path::new(&path).exists()
        {
            println!(
                "cargo:rerun-if-changed={}",
                std::path::absolute(path).unwrap().display()
            );
        }
    }
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git(&["rev-parse", "--git-path", &branch])
    {
        println!(
            "cargo:rerun-if-changed={}",
            std::path::absolute(path).unwrap().display()
        );
    }
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        use winresource::{VersionInfo, WindowsResource};
        let mut fields = version
            .split('.')
            .map(|s| s.parse::<u16>().expect("Invalid Windows resource version"));
        let mut number = 0u64;
        for shift in [48, 32, 16] {
            number |= u64::from(fields.next().expect("Missing version field")) << shift;
        }
        assert!(fields.next().is_none(), "Too many version fields");
        let revision = git(&["rev-parse", "HEAD"])
            .map(|hash| format!("{version}+{hash}"))
            .unwrap_or_else(|| version.to_owned());
        let mut resource = WindowsResource::new();
        resource
            .set_icon("windows/assets/QuotaBar.ico")
            .set("FileDescription", "QuotaBar")
            .set("ProductName", "QuotaBar")
            .set("FileVersion", version)
            .set("ProductVersion", &revision)
            .set_version_info(VersionInfo::FILEVERSION, number)
            .set_version_info(VersionInfo::PRODUCTVERSION, number);
        if dashboard {
            resource
                .set("OriginalFilename", "QuotaBar.Dashboard.exe")
                .compile_for("quotabar-dashboard")
                .unwrap();
        } else {
            resource
                .set("OriginalFilename", "QuotaBar.StatusLine.exe")
                .compile_for("quotabar-statusline")
                .unwrap();
            resource
                .set("OriginalFilename", "QuotaBar.Monitor.exe")
                .set_manifest_file("windows/src/QuotaBar.Monitor/app.manifest")
                .compile_for("quotabar-monitor")
                .unwrap();
        }
    }
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}
