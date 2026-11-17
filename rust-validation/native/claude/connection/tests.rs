use super::*;
use std::{ffi::OsString, os::windows::ffi::OsStringExt};
use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

#[test]
fn dos_alias_reconnects_to_the_same_profile_and_preserves_existing_callbacks() -> io::Result<()> {
    let root = std::env::temp_dir().join(format!("QuotaBar alias test {}", Uuid::new_v4()));
    let dir = root.join("Claude settings");
    fs::create_dir_all(&dir)?;
    let directory = canonical(&dir)?;
    let input = crate::files::windows::wide_file_path(&dir)?;
    let mut buf = vec![0; 32_768];
    let count = unsafe { GetShortPathNameW(input.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    assert!(count > 0 && (count as usize) < buf.len());
    let alias = PathBuf::from(OsString::from_wide(&buf[..count as usize]));
    if alias == Path::new(&directory) {
        // Some NTFS volumes disable creation of 8.3 names.
        return fs::remove_dir_all(root);
    }
    assert_eq!(canonical(&alias)?, directory);
    assert_eq!(
        canonical(&alias.join("not created"))?,
        Path::new(&directory).join("not created").to_str().unwrap()
    );

    let helper = std::env::current_exe()?;
    let data = root.join("data");
    let previous_command = json!({"type":"command","command":"echo prior"});
    let config = dir.join("settings.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({"statusLine":previous_command}))?,
    )?;
    let fresh = install(&data, &alias, &helper, Some("powershell"))?;
    assert_eq!(fresh.id, profile_id(&directory));
    assert_eq!(
        install(&data, &dir, &helper, Some("powershell"))?.id,
        fresh.id
    );
    assert!(uninstall(&data, &fresh.id)?);

    // Reproduce an already-installed Rust callback that stored the short path and its hash.
    let old_root = root.join("early build");
    let spelled = alias.to_str().unwrap();
    let old_id = profile_id(spelled);
    let generation = Uuid::new_v4().to_string();
    let command = wrapper(&helper, &old_root, &old_id, &generation, "powershell")?;
    let old_folder = folder(&old_root, &old_id)?;
    ensure_private_directory(&old_folder)?;
    write_private(
        &old_folder.join(ACTIVE),
        &serde_json::to_vec(&json!({
            "id":old_id,"directory":spelled,"generation":generation,"command":command,
            "original":previous_command,"shell":"powershell"
        }))?,
    )?;
    fs::write(
        &config,
        serde_json::to_vec(&json!({"statusLine":{"type":"command","command":command}}))?,
    )?;
    assert_eq!(load(&old_root, &old_id)?.generation, generation);
    for path in [&dir, &alias] {
        assert_eq!(fetch(&old_root, path)?.state, "waiting");
        assert_eq!(
            install(&old_root, path, &helper, Some("powershell"))?.id,
            old_id
        );
    }
    assert!(uninstall(&old_root, &old_id)?);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(config)?)?["statusLine"],
        previous_command
    );
    fs::remove_dir_all(root)
}
