//! Render the packaged monitor's diagnostic snapshots with the same GDI+ surfaces as the tray.
use crate::{
    files::wide,
    tray_icon::{self, ARGB_32, Owned, checked},
    tray_menu::Menu,
};
use std::{
    io,
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::{
    Win32::{
        Foundation::RECT,
        Graphics::{
            Gdi::{BITMAP, DeleteObject, GetObjectW},
            GdiPlus::{
                FontStyleRegular, GdipCreateBitmapFromScan0, GdipCreateFont,
                GdipCreateFontFamilyFromName, GdipCreateSolidFill, GdipDeleteBrush, GdipDeleteFont,
                GdipDeleteFontFamily, GdipDeleteGraphics, GdipDisposeImage, GdipDrawImageRectI,
                GdipDrawString, GdipGetDC, GdipGetImageGraphicsContext, GdipGraphicsClear,
                GdipReleaseDC, GdipSaveImageToFile, GpBitmap, GpGraphics, GpSolidFill, RectF,
                UnitPixel,
            },
        },
        UI::WindowsAndMessaging::{
            GetIconInfo, GetWindowRect, ICONINFO, SendMessageW, WM_PRINTCLIENT,
        },
    },
    core::{GUID, w},
};

const PNG: GUID = GUID::from_u128(0x557cf406_1a04_11d3_9a73_0000f81ef32e);

struct Canvas {
    graphics: Owned<GpGraphics>,
    bitmap: Owned<GpBitmap>,
}

impl Canvas {
    fn new(width: i32, height: i32) -> io::Result<Self> {
        tray_icon::start()?;
        unsafe {
            let bitmap = Owned::new(
                |p| GdipCreateBitmapFromScan0(width, height, 0, ARGB_32, null_mut(), p),
                |p: *mut GpBitmap| {
                    GdipDisposeImage(p.cast());
                },
            )?;
            let graphics = Owned::new(
                |p| GdipGetImageGraphicsContext(bitmap.raw.cast(), p),
                |p| {
                    GdipDeleteGraphics(p);
                },
            )?;
            Ok(Self { graphics, bitmap })
        }
    }
    fn save(self, path: &Path) -> io::Result<()> {
        let path = wide(path)?;
        drop(self.graphics);
        unsafe {
            checked(GdipSaveImageToFile(
                self.bitmap.raw.cast(),
                path.as_ptr(),
                &PNG,
                null(),
            ))
        }
    }
}

pub fn menu(menu: &Menu, path: &Path) -> io::Result<()> {
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(menu.hwnd(), &mut rect) == 0 {
            return Err(io::Error::last_os_error());
        }
        let canvas = Canvas::new(rect.right - rect.left, rect.bottom - rect.top)?;
        let mut dc = null_mut();
        checked(GdipGetDC(canvas.graphics.raw, &mut dc))?;
        SendMessageW(menu.hwnd(), WM_PRINTCLIENT, dc as usize, 0);
        checked(GdipReleaseDC(canvas.graphics.raw, dc))?;
        canvas.save(path)
    }
}

fn check_icon(icon: &tray_icon::ShellIcon, size: i32) -> io::Result<()> {
    unsafe {
        let mut info = ICONINFO::default();
        if GetIconInfo(icon.0, &mut info) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut bitmap = BITMAP::default();
        let read = GetObjectW(
            info.hbmColor,
            size_of::<BITMAP>() as i32,
            (&mut bitmap as *mut BITMAP).cast(),
        );
        DeleteObject(info.hbmColor);
        DeleteObject(info.hbmMask);
        if read == 0 || (bitmap.bmWidth, bitmap.bmHeight) != (size, size) {
            return Err(io::Error::other("Unexpected shell icon dimensions"));
        }
        Ok(())
    }
}

pub fn samples(directory: &Path) -> io::Result<()> {
    const LABELS: [&str; 9] = ["0", "7", "16", "38", "99", "100", "—", "!", "logo"];
    for light in [true, false] {
        let canvas = Canvas::new(800, 630)?;
        unsafe {
            let g = canvas.graphics.raw;
            checked(GdipGraphicsClear(
                g,
                if light { 0xfff5f5f5 } else { 0xff191919 },
            ))?;
            let family = Owned::new(
                |p| GdipCreateFontFamilyFromName(w!("Segoe UI"), null_mut(), p),
                |p| {
                    GdipDeleteFontFamily(p);
                },
            )?;
            let font = Owned::new(
                |p| GdipCreateFont(family.raw, 11., FontStyleRegular, UnitPixel, p),
                |p| {
                    GdipDeleteFont(p);
                },
            )?;
            let brush = Owned::new(
                |p| GdipCreateSolidFill(if light { 0xff696969 } else { 0xffd3d3d3 }, p),
                |p: *mut GpSolidFill| {
                    GdipDeleteBrush(p.cast());
                },
            )?;
            let text = |label: &str, x: i32, y: i32| {
                let label: Vec<u16> = label.encode_utf16().collect();
                let origin = RectF {
                    X: x as f32,
                    Y: y as f32,
                    ..Default::default()
                };
                checked(GdipDrawString(
                    g,
                    label.as_ptr(),
                    label.len() as i32,
                    font.raw,
                    &origin,
                    null(),
                    brush.raw.cast(),
                ))
            };
            for (column, label) in LABELS.iter().enumerate() {
                text(label, 90 + column as i32 * 78, 10)?;
            }
            for (row, size) in [16, 20, 24, 32, 40, 48, 64].into_iter().enumerate() {
                let y = 42 + row as i32 * 82;
                text(&format!("{size}px"), 10, y + 20)?;
                for (column, label) in LABELS.into_iter().enumerate() {
                    let warning = matches!(label, "0" | "7" | "16" | "!");
                    let logo = label == "logo";
                    let mut pixels = tray_icon::render(label, logo, warning, size, light)?;
                    let bitmap = Owned::new(
                        |p| {
                            GdipCreateBitmapFromScan0(
                                size,
                                size,
                                size * 4,
                                ARGB_32,
                                pixels.as_mut_ptr(),
                                p,
                            )
                        },
                        |p: *mut GpBitmap| {
                            GdipDisposeImage(p.cast());
                        },
                    )?;
                    let margin = (64 - size) / 2;
                    checked(GdipDrawImageRectI(
                        g,
                        bitmap.raw.cast(),
                        78 + column as i32 * 78 + margin,
                        y + margin,
                        size,
                        size,
                    ))?;
                    check_icon(
                        &tray_icon::shell_icon(label, logo, warning, size, light)?,
                        size,
                    )?;
                }
            }
        }
        canvas.save(&directory.join(if light {
            "tray-light.png"
        } else {
            "tray-dark.png"
        }))?;
    }
    Ok(())
}
