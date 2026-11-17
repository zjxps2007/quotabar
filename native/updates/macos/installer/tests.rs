use super::*;
use crate::updates::{Asset, Release, Verified, Version};
use objc2_foundation::{NSString, NSURL};
use p256::{
    ecdsa::{Signature, SigningKey, signature::Signer},
    elliptic_curve::rand_core::OsRng,
    pkcs8::{EncodePublicKey, LineEnding},
};
use serde_json::json;
use sha2::{Digest, Sha256};

fn file_url(path: &Path) -> String {
    NSURL::fileURLWithPath_isDirectory(&NSString::from_str(path.to_str().unwrap()), true)
        .absoluteString()
        .unwrap()
        .to_string()
}

fn application(path: &Path, version: &str, key: &str, executable: &Path) {
    let contents = path.join("Contents");
    let resources = contents.join("Resources/QuotaBar_QuotaBar.bundle/Contents");
    fs::create_dir_all(contents.join("MacOS")).unwrap();
    fs::create_dir_all(resources.join("Resources")).unwrap();
    fs::write(
        resources.join("Info.plist"),
        r#"<plist version="1.0"><dict>
        <key>CFBundleIdentifier</key><string>com.aiusage.lite.resources</string>
        <key>CFBundlePackageType</key><string>BNDL</string></dict></plist>"#,
    )
    .unwrap();
    fs::write(resources.join("Resources/UpdatePublicKey.pem"), key).unwrap();
    fs::write(
        contents.join("Info.plist"),
        format!(
            r#"<plist version="1.0"><dict>
        <key>CFBundleExecutable</key><string>QuotaBar</string>
        <key>CFBundleIdentifier</key><string>com.aiusage.lite</string>
        <key>CFBundlePackageType</key><string>APPL</string>
        <key>CFBundleShortVersionString</key><string>{version}</string>
        <key>CFBundleVersion</key><string>12</string></dict></plist>"#
        ),
    )
    .unwrap();
    fs::copy(executable, contents.join("MacOS/QuotaBar")).unwrap();
    run(Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(path))
    .unwrap();
}

#[test]
fn signed_stage_helper_and_startup_preserve_the_original_until_handoff() {
    let root = fs::canonicalize(env::temp_dir())
        .unwrap()
        .join(Uuid::new_v4().to_string());
    let transaction = Uuid::new_v4();
    let updates = root.join("Updates");
    let stage = updates.join(transaction.to_string().to_uppercase());
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&stage)
        .unwrap();
    let key = SigningKey::random(&mut OsRng);
    let pem = key
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .unwrap();
    let program = root.join("test-app");
    let source = root.join("main.c");
    fs::write(
        &source,
        "#include <unistd.h>\nint main(void) { sleep(30); return 0; }",
    )
    .unwrap();
    run(Command::new("/usr/bin/cc")
        .arg(&source)
        .arg("-o")
        .arg(&program))
    .unwrap();
    let target = root.join("QuotaBar 한글.app");
    let published = root.join("release/QuotaBar.app");
    application(&target, "1.0.0", &pem, &program);
    application(&published, "1.1.0", &pem, &program);
    let name = platform().asset_name(Version(1, 1, 0));
    let archive = stage.join(&name);
    run(Command::new("/usr/bin/ditto")
        .args(["-c", "-k", "--keepParent"])
        .arg(&published)
        .arg(&archive))
    .unwrap();
    let payload = fs::read(&archive).unwrap();
    let repo = "https://github.com/zjxps2007/quotabar";
    let asset = Asset {
        name: name.clone(),
        url: format!("{repo}/releases/download/v1.1.0/{name}"),
        bytes: payload.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&payload)),
    };
    let release = Release {
        tag: "v1.1.0".into(),
        version: "1.1.0".into(),
        page: format!("{repo}/releases/tag/v1.1.0"),
        asset: Some(asset.clone()),
    };
    let manifest = serde_json::to_vec(&json!({"schemaVersion":1,"version":"1.1.0","build":12,
        "assets":[{"name":name,"bytes":asset.bytes,"sha256":asset.sha256}]}))
    .unwrap();
    let signature: Signature = key.sign(&manifest);
    fs::write(stage.join("update-manifest.json"), &manifest).unwrap();
    fs::write(
        stage.join("update-manifest.sig"),
        signature.to_der().as_bytes(),
    )
    .unwrap();
    let update = Verified {
        release,
        build: 12,
        asset,
    };
    let plan = serde_json::to_vec(
        &json!({"staged":{"update":update,"directory":file_url(&stage)},
        "target":file_url(&target),"currentVersion":"1.0.0"}),
    )
    .unwrap();
    let paths = Replacement::new(target.clone(), &transaction.to_string().to_uppercase()).unwrap();
    assert!(prepare_in(&updates, &plan, PUBLIC_KEY).is_err());
    let job = stage.join("install.json");
    let installer = stage.join("Installer.app");
    fs::create_dir(&installer).unwrap();
    fs::write(installer.join("marker"), b"existing").unwrap();
    assert!(prepare_in(&updates, &plan, &pem).is_err());
    assert!(installer.join("marker").is_file() && !paths.incoming.exists() && !job.exists());
    fs::remove_dir_all(&installer).unwrap();
    // The ordinary installer also accepts an intact stage without a signing key.
    fs::remove_file(stage.join("update-manifest.sig")).unwrap();
    prepare_in(&updates, &plan, &pem).unwrap();
    assert_eq!(fs::read(&job).unwrap(), plan);
    assert_eq!(
        bundle_field(&paths.incoming, "CFBundleShortVersionString").unwrap(),
        "1.1.0"
    );
    assert_eq!(
        bundle_field(&installer, "CFBundleShortVersionString").unwrap(),
        "1.0.0"
    );
    assert!(prepare_in(&updates, &plan, &pem).is_err());

    let ready = stage.join("installer-ready.json");
    let receipt = |version: &str| {
        serde_json::to_vec(&json!({"version":version,"success":true,"message":"ready"})).unwrap()
    };
    fs::write(&ready, receipt("1.1.0")).unwrap();
    let mut helper = Installer::launch_in(&job, &updates).unwrap();
    assert!(!helper.ready().unwrap());
    fs::write(&ready, receipt("2.0.0")).unwrap();
    assert!(!helper.ready().unwrap());
    fs::write(&ready, receipt("1.1.0")).unwrap();
    assert!(helper.ready().unwrap());
    helper.finish(true);
    assert!(acknowledge(&updates, &target, transaction).is_err());
    assert!(acknowledge(&updates, &published, transaction).is_err());
    assert!(!stage.join("started.json").exists());
    paths.commit().unwrap();
    acknowledge(&updates, &target, transaction).unwrap();
    let result: InstallResult =
        serde_json::from_slice(&fs::read(stage.join("started.json")).unwrap()).unwrap();
    assert!(result.success && result.version == "1.1.0");
    paths.rollback().unwrap();
    assert_eq!(
        bundle_field(&target, "CFBundleShortVersionString").unwrap(),
        "1.0.0"
    );
    fs::remove_dir_all(root).unwrap();
}
