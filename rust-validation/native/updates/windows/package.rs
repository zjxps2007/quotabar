//! Prepare a portable overlay without modifying the running installation.
use super::{absolute, parent, same};
use crate::{
    files::{invalid, reject_links},
    windows_format::ordinal_uppercase,
};
use chrono::{Local, TimeZone};
use rawzip::{CompressionMethod, ZipArchive};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;

pub(super) fn check_cancel(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(io::ErrorKind::Interrupted.into())
    } else {
        Ok(())
    }
}

pub fn copy_tree(source: &Path, destination: &Path, cancel: &AtomicBool) -> io::Result<()> {
    check_cancel(cancel)?;
    let source = absolute(source)?;
    let destination = absolute(destination)?;
    if destination.ancestors().any(|p| same(p, &source)) {
        return Err(invalid("Copy destination is inside source"));
    }
    fs::create_dir(&destination)?;
    let result = copy_contents(&source, &destination, cancel);
    if result.is_err() {
        let _ = fs::remove_dir_all(&destination);
    }
    result
}

/// Both prepare and rollback first reserve their own destination directory.
pub(super) fn copy_contents(
    source: &Path,
    destination: &Path,
    cancel: &AtomicBool,
) -> io::Result<()> {
    reject_links(source)?;
    for entry in fs::read_dir(source)? {
        check_cancel(cancel)?;
        let entry = entry?;
        let path = entry.path();
        let target = destination.join(entry.file_name());
        reject_links(&path)?;
        reject_links(&target)?;
        if entry.file_type()?.is_dir() {
            fs::create_dir(&target)?;
            copy_contents(&path, &target, cancel)?;
        } else {
            fs::copy(&path, &target)?;
        }
    }
    check_cancel(cancel)
}

fn relative_path(name: &str) -> io::Result<PathBuf> {
    let relative = name
        .strip_prefix("QuotaBar-Windows-x64/")
        .ok_or_else(|| invalid("Entry outside Windows package"))?;
    if relative.starts_with('/') || relative.contains([':', '\0']) {
        return Err(invalid("Absolute or invalid update path"));
    }
    let mut path = PathBuf::new();
    for component in relative.split('/').filter(|s| !s.is_empty()) {
        if component.ends_with(['.', ' ']) || device_name(component) {
            return Err(invalid("Unsafe Windows update path"));
        }
        path.push(component);
    }
    Ok(path)
}

fn device_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|n| {
            matches!(
                n,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

pub fn overlay_zip(source: &Path, destination: &Path, cancel: &AtomicBool) -> io::Result<()> {
    const MAX_ENTRIES: u64 = 10_000;
    const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
    check_cancel(cancel)?;
    reject_links(source)?;
    let destination = absolute(destination)?;
    let mut directory_buffer = vec![0; rawzip::RECOMMENDED_BUFFER_SIZE];
    let archive = ZipArchive::from_file(File::open(source)?, &mut directory_buffer)
        .map_err(io::Error::other)?;
    let count = archive.entries_hint();
    if count > MAX_ENTRIES {
        return Err(invalid("Too many update entries"));
    }
    let mut seen = HashSet::new();
    let mut total = 0u64;
    let mut processed = 0;
    let mut buffer = vec![0; 64 * 1024];
    let mut entries = archive.entries(&mut directory_buffer);
    while let Some(entry) = entries.next_entry().map_err(io::Error::other)? {
        check_cancel(cancel)?;
        processed += 1;
        if processed > count
            || entry.external_attributes() >> 16 & 0xf000 == 0xa000
            || entry.flags().is_encrypted()
        {
            return Err(invalid("Unsafe update archive entry"));
        }
        let name = String::from_utf8_lossy(entry.file_path().as_bytes()).replace('\\', "/");
        let path = relative_path(&name)?;
        if path.as_os_str().is_empty() {
            continue;
        }
        if !seen.insert(ordinal_uppercase(&path.to_string_lossy())) {
            return Err(invalid("Duplicate update archive entry"));
        }
        total = total
            .checked_add(entry.uncompressed_size_hint())
            .filter(|v| *v <= MAX_BYTES)
            .ok_or_else(|| invalid("Update archive is too large"))?;
        let target = destination.join(path);
        reject_links(&target)?;
        if name.ends_with('/') {
            fs::create_dir_all(&target)?;
            continue;
        }
        let local = archive
            .get_entry(entry.wayfinder())
            .map_err(io::Error::other)?;
        let stream = local.reader();
        let decoder: Box<dyn Read + '_> = match entry.compression_method() {
            CompressionMethod::STORE => Box::new(stream),
            CompressionMethod::DEFLATE => Box::new(flate2::read::DeflateDecoder::new(stream)),
            CompressionMethod::DEFLATE64 => Box::new(deflate64::Deflate64Decoder::new(stream)),
            _ => return Err(invalid("Unsupported update compression")),
        };
        let mut input = local.verifying_reader(decoder);
        let folder = parent(&target)?;
        fs::create_dir_all(folder)?;
        // Replacing a directory entry also preserves any other hard links to the old file.
        let temporary = folder.join(format!(".{}.tmp", Uuid::new_v4()));
        let mut output = File::create_new(&temporary)?;
        let write = (|| {
            loop {
                check_cancel(cancel)?;
                let size = input.read(&mut buffer)?;
                if size == 0 {
                    break;
                }
                output.write_all(&buffer[..size])?;
            }
            let time = entry.last_modified_dos();
            if let Some(date) = Local
                .with_ymd_and_hms(
                    time.year().into(),
                    time.month().into(),
                    time.day().into(),
                    time.hour().into(),
                    time.minute().into(),
                    time.second().into(),
                )
                .latest()
            {
                output.set_modified(date.to_utc().into())?;
            }
            drop(output);
            fs::rename(&temporary, &target)
        })();
        if write.is_err() {
            let _ = fs::remove_file(temporary);
        }
        write?;
    }
    if processed != count {
        return Err(invalid("Incomplete update central directory"));
    }
    check_cancel(cancel)
}
