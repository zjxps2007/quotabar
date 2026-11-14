//! GDI+ glyph outlines on a transparent, straight-alpha BGRA surface, as used by
//! the existing notification-area icon and the logo in its pop-up.
use std::{
    io,
    ptr::{null, null_mut},
    sync::OnceLock,
};
use windows_sys::Win32::Graphics::GdiPlus::{
    FillModeAlternate, FontStyleBold, GdipAddPathArc, GdipAddPathString, GdipClosePathFigure,
    GdipCreateBitmapFromScan0, GdipCreateFontFamilyFromName, GdipCreatePath, GdipCreateSolidFill,
    GdipDeleteBrush, GdipDeleteFontFamily, GdipDeleteGraphics, GdipDeletePath,
    GdipDeleteStringFormat, GdipDisposeImage, GdipFillPath, GdipGetImageGraphicsContext,
    GdipGetPathWorldBounds, GdipScaleWorldTransform, GdipSetSmoothingMode,
    GdipStringFormatGetGenericTypographic, GdipTranslateWorldTransform, GdiplusStartup,
    GdiplusStartupInput, GpBitmap, GpSolidFill, MatrixOrderPrepend, RectF, SmoothingModeAntiAlias,
    Status,
};

const ARGB_32: i32 = 0x26200a; // PixelFormat32bppARGB in GdiplusPixelFormats.h
const LOGO: &str = include_str!("../shared/assets/AppMark.svg");
static STARTUP: OnceLock<Result<usize, i32>> = OnceLock::new();
static BARS: OnceLock<Vec<[f32; 5]>> = OnceLock::new();

fn checked(status: Status) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!("GDI+ status {status}")))
    }
}
fn start() -> io::Result<()> {
    // The token lives for the whole UI process, including overlapping paints.
    match STARTUP.get_or_init(|| {
        let mut token = 0;
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        let status = unsafe { GdiplusStartup(&mut token, &input, null_mut()) };
        if status == 0 { Ok(token) } else { Err(status) }
    }) {
        Ok(_) => Ok(()),
        Err(status) => checked(*status),
    }
}

struct Owned<T> {
    raw: *mut T,
    release: unsafe fn(*mut T),
}
impl<T> Owned<T> {
    fn new(
        create: impl FnOnce(*mut *mut T) -> Status,
        release: unsafe fn(*mut T),
    ) -> io::Result<Self> {
        let mut raw = null_mut();
        checked(create(&mut raw))?;
        Ok(Self { raw, release })
    }
}
impl<T> Drop for Owned<T> {
    fn drop(&mut self) {
        unsafe {
            (self.release)(self.raw);
        }
    }
}

fn bars() -> &'static [[f32; 5]] {
    BARS.get_or_init(|| {
        LOGO.split("<rect ")
            .skip(1)
            .map(|element| {
                ["x", "y", "width", "height", "rx"].map(|field| {
                    let (_, rest) = element
                        .split_once(&format!("{field}=\""))
                        .expect("Application mark rectangle");
                    rest.split('"')
                        .next()
                        .unwrap()
                        .parse()
                        .expect("Application mark dimension")
                })
            })
            .collect()
    })
}

/// Returns the same positive-stride BGRA pixels that Format32bppArgb exposes.
pub fn render(
    text: &str,
    use_logo: bool,
    warning: bool,
    size: i32,
    light: bool,
) -> io::Result<Vec<u8>> {
    if !(1..=256).contains(&size) {
        return Err(crate::files::invalid("Invalid icon size"));
    }
    start()?;
    let mut pixels = vec![0u8; size as usize * size as usize * 4];
    unsafe {
        let bitmap = Owned::new(
            |out| {
                GdipCreateBitmapFromScan0(size, size, size * 4, ARGB_32, pixels.as_mut_ptr(), out)
            },
            |p: *mut GpBitmap| {
                GdipDisposeImage(p.cast());
            },
        )?;
        let graphics = Owned::new(
            |out| GdipGetImageGraphicsContext(bitmap.raw.cast(), out),
            |p| {
                GdipDeleteGraphics(p);
            },
        )?;
        checked(GdipSetSmoothingMode(graphics.raw, SmoothingModeAntiAlias))?;
        let color = match (warning, light) {
            (true, true) => 0xffa65500,
            (true, false) => 0xffffb74d,
            (false, true) => 0xff000000,
            (false, false) => 0xffffffff,
        };
        let brush = Owned::new(
            |out| GdipCreateSolidFill(color, out),
            |p: *mut GpSolidFill| {
                GdipDeleteBrush(p.cast());
            },
        )?;
        let path = || {
            Owned::new(
                |out| GdipCreatePath(FillModeAlternate, out),
                |p| {
                    GdipDeletePath(p);
                },
            )
        };
        if use_logo {
            let scale = (size as f32 - 2.) / 282.;
            let left = (size as f32 - 266. * scale) / 2.;
            for &[sx, sy, sw, sh, r] in bars() {
                let p = path()?;
                let (x, y, w, h, d) = (
                    left + sx * scale,
                    1. + sy * scale,
                    sw * scale,
                    sh * scale,
                    r * scale * 2.,
                );
                for (px, py, angle) in [
                    (x, y, 180.),
                    (x + w - d, y, 270.),
                    (x + w - d, y + h - d, 0.),
                    (x, y + h - d, 90.),
                ] {
                    checked(GdipAddPathArc(p.raw, px, py, d, d, angle, 90.))?;
                }
                checked(GdipClosePathFigure(p.raw))?;
                checked(GdipFillPath(graphics.raw, brush.raw.cast(), p.raw))?;
            }
        } else {
            let name = "Segoe UI\0".encode_utf16().collect::<Vec<_>>();
            let family = Owned::new(
                |out| GdipCreateFontFamilyFromName(name.as_ptr(), null_mut(), out),
                |p| {
                    GdipDeleteFontFamily(p);
                },
            )?;
            let format = Owned::new(
                |out| GdipStringFormatGetGenericTypographic(out),
                |p| {
                    GdipDeleteStringFormat(p);
                },
            )?;
            let p = path()?;
            let label = text.encode_utf16().collect::<Vec<_>>();
            checked(GdipAddPathString(
                p.raw,
                label.as_ptr(),
                label.len() as i32,
                family.raw,
                FontStyleBold,
                32.,
                &RectF::default(),
                format.raw,
            ))?;
            let mut bounds = RectF::default();
            checked(GdipGetPathWorldBounds(p.raw, &mut bounds, null(), null()))?;
            if bounds.Width > 0. && bounds.Height > 0. {
                let size = size as f32;
                let scale = ((size - 1.) / bounds.Width)
                    .min(size * 0.84 / bounds.Height)
                    .min(size / 24.);
                checked(GdipTranslateWorldTransform(
                    graphics.raw,
                    (size - bounds.Width * scale) / 2.,
                    (size - bounds.Height * scale) / 2.,
                    MatrixOrderPrepend,
                ))?;
                checked(GdipScaleWorldTransform(
                    graphics.raw,
                    scale,
                    scale,
                    MatrixOrderPrepend,
                ))?;
                checked(GdipTranslateWorldTransform(
                    graphics.raw,
                    -bounds.X,
                    -bounds.Y,
                    MatrixOrderPrepend,
                ))?;
                checked(GdipFillPath(graphics.raw, brush.raw.cast(), p.raw))?;
            }
        }
    }
    Ok(pixels)
}
