use super::*;
use crate::tray_icon::{ARGB_32, Owned};
use std::result::Result::Ok;
use windows_sys::Win32::{
    Graphics::GdiPlus::*,
    System::Registry::*,
    UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
};
type Color = u32;
#[derive(Clone, Copy)]
struct Palette {
    bg: Color,
    surface: Color,
    ink: Color,
    muted: Color,
    border: Color,
    track: Color,
    hover: Color,
    warn: Color,
    dark: bool,
    hc: bool,
}
impl Palette {
    fn new(mode: &str) -> Self {
        unsafe {
            let mut light = 1u32;
            let mut n = 4;
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
                w!("AppsUseLightTheme"),
                RRF_RT_REG_DWORD,
                null_mut(),
                (&mut light as *mut u32).cast(),
                &mut n,
            );
            let dark = mode == "dark" || mode == "system" && light == 0;
            let c = |l: u32, d: u32| 0xff000000u32 | if dark { d } else { l };
            let mut info = HIGHCONTRASTW {
                cbSize: size_of::<HIGHCONTRASTW>() as u32,
                ..Default::default()
            };
            let hc = SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                info.cbSize,
                (&mut info as *mut HIGHCONTRASTW).cast(),
                0,
            ) != 0
                && info.dwFlags & HCF_HIGHCONTRASTON != 0;
            if hc {
                return Self {
                    bg: system(COLOR_WINDOW),
                    surface: system(COLOR_WINDOW),
                    ink: system(COLOR_WINDOWTEXT),
                    muted: system(COLOR_WINDOWTEXT),
                    border: system(COLOR_WINDOWTEXT),
                    track: system(COLOR_GRAYTEXT),
                    hover: system(COLOR_HIGHLIGHT),
                    warn: system(COLOR_WINDOWTEXT),
                    dark,
                    hc,
                };
            }
            Self {
                bg: c(0xffffff, 0x131313),
                surface: c(0xf7f7f7, 0x1c1c1c),
                ink: c(0x171717, 0xf2f2f2),
                muted: c(0x757575, 0xa3a3a3),
                border: c(0xeaeaea, 0x2d2d2d),
                track: c(0xe5e5e5, 0x353535),
                hover: c(0xeeeeee, 0x292929),
                warn: c(0xb86708, 0xf5a34f),
                dark,
                hc,
            }
        }
    }
    fn provider(self, name: &str) -> Color {
        if self.hc {
            return system(COLOR_HIGHLIGHT);
        }
        0xff000000
            | match (name, self.dark) {
                ("Claude", false) => 0xa84f26,
                ("Claude", true) => 0xe6824a,
                ("Gemini", false) => 0x1a73e8,
                ("Gemini", true) => 0x8ab4f8,
                (_, false) => 0x4f5cc8,
                (_, true) => 0x8995ff,
            }
    }
}
fn system(index: SYS_COLOR_INDEX) -> Color {
    let color = unsafe { GetSysColor(index) };
    0xff000000 | (color & 0xff) << 16 | (color & 0xff00) | (color >> 16) & 0xff
}
fn rounded_path([x, y, w, h]: [f32; 4], radius: f32) -> io::Result<Owned<GpPath>> {
    let d = (radius * 2.).min(w).min(h);
    unsafe {
        let path = Owned::new(
            |p| GdipCreatePath(FillModeAlternate, p),
            |p| {
                GdipDeletePath(p);
            },
        )?;
        for (cx, cy, angle) in [
            (x, y, 180.),
            (x + w - d, y, 270.),
            (x + w - d, y + h - d, 0.),
            (x, y + h - d, 90.),
        ] {
            crate::tray_icon::checked(GdipAddPathArc(path.raw, cx, cy, d, d, angle, 90.))?;
        }
        crate::tray_icon::checked(GdipClosePathFigure(path.raw))?;
        Ok(path)
    }
}
pub(super) fn window_region(width: i32, height: i32, radius: f32) -> io::Result<HRGN> {
    let path = rounded_path([0., 0., width as f32, height as f32], radius)?;
    unsafe {
        let region = Owned::new(
            |p| GdipCreateRegionPath(path.raw, p),
            |p| {
                GdipDeleteRegion(p);
            },
        )?;
        let mut handle = null_mut();
        crate::tray_icon::checked(GdipGetRegionHRgn(region.raw, null_mut(), &mut handle))?;
        Ok(handle)
    }
}
struct Graphics {
    g: Owned<GpGraphics>,
    family: Owned<GpFontFamily>,
    palette: Palette,
}
impl Graphics {
    fn new(dc: HDC, palette: Palette) -> Option<Self> {
        unsafe {
            let g = Owned::new(
                |p| GdipCreateFromHDC(dc, p),
                |p| {
                    GdipDeleteGraphics(p);
                },
            )
            .ok()?;
            GdipSetPageUnit(g.raw, UnitPixel);
            GdipSetSmoothingMode(g.raw, SmoothingModeAntiAlias);
            GdipSetTextRenderingHint(g.raw, TextRenderingHintClearTypeGridFit);
            GdipSetInterpolationMode(g.raw, InterpolationModeHighQualityBicubic);
            let family = Owned::new(
                |p| GdipCreateFontFamilyFromName(w!("Segoe UI"), null_mut(), p),
                |p| {
                    GdipDeleteFontFamily(p);
                },
            )
            .ok()?;
            Some(Self { g, family, palette })
        }
    }
    fn transform(&self, x: f32, y: f32, s: f32) {
        unsafe {
            GdipResetWorldTransform(self.g.raw);
            GdipTranslateWorldTransform(self.g.raw, x, y, MatrixOrderPrepend);
            GdipScaleWorldTransform(self.g.raw, s, s, MatrixOrderPrepend);
        }
    }
    fn action_glyph(&self, primary: bool, ink: Color) {
        unsafe {
            let Ok(pen) = Owned::new(
                |p| GdipCreatePen1(ink, 1.5, UnitWorld, p),
                |p| {
                    GdipDeletePen(p);
                },
            ) else {
                return;
            };
            let mut saved = 0;
            if GdipSaveGraphics(self.g.raw, &mut saved) != 0 {
                return;
            }
            GdipTranslateWorldTransform(
                self.g.raw,
                12.,
                if primary { 10. } else { 8. },
                MatrixOrderPrepend,
            );
            GdipScaleWorldTransform(self.g.raw, 0.8, 0.8, MatrixOrderPrepend);
            GdipSetPenStartCap(pen.raw, LineCapRound);
            GdipSetPenEndCap(pen.raw, LineCapRound);
            GdipSetPenLineJoin(pen.raw, LineJoinRound);
            if primary {
                for x in [2, 12] {
                    for y in [2, 12] {
                        GdipDrawRectangleI(self.g.raw, pen.raw, x, y, 6, 6);
                    }
                }
            } else {
                GdipDrawArcI(self.g.raw, pen.raw, 3, 3, 14, 14, -50., 280.);
                GdipDrawLineI(self.g.raw, pen.raw, 10, 1, 10, 10);
            }
            GdipRestoreGraphics(self.g.raw, saved);
        }
    }
    fn shape(&self, b: [f32; 4], rad: f32, color: Color, stroke: bool) {
        let Ok(path) = rounded_path(b, rad) else {
            return;
        };
        unsafe {
            if stroke {
                if let Ok(pen) = Owned::new(
                    |p| GdipCreatePen1(color, 1., UnitWorld, p),
                    |p| {
                        GdipDeletePen(p);
                    },
                ) {
                    GdipDrawPath(self.g.raw, pen.raw, path.raw);
                }
            } else if let Ok(brush) = Owned::new(
                |p| GdipCreateSolidFill(color, p),
                |p: *mut GpSolidFill| {
                    GdipDeleteBrush(p.cast());
                },
            ) {
                GdipFillPath(self.g.raw, brush.raw.cast(), path.raw);
            }
        }
    }
    fn text(&self, s: &str, size: f32, b: [f32; 4], color: Color, bold: bool, right: bool) {
        unsafe {
            let Ok(font) = Owned::new(
                |p| {
                    GdipCreateFont(
                        self.family.raw,
                        size,
                        if bold {
                            FontStyleBold
                        } else {
                            FontStyleRegular
                        },
                        UnitPixel,
                        p,
                    )
                },
                |p| {
                    GdipDeleteFont(p);
                },
            ) else {
                return;
            };
            let Ok(format) = Owned::new(
                |p| GdipCreateStringFormat(StringFormatFlagsNoWrap, 0, p),
                |p| {
                    GdipDeleteStringFormat(p);
                },
            ) else {
                return;
            };
            GdipSetStringFormatAlign(
                format.raw,
                if right {
                    StringAlignmentFar
                } else {
                    StringAlignmentNear
                },
            );
            GdipSetStringFormatLineAlign(format.raw, StringAlignmentCenter);
            GdipSetStringFormatTrimming(format.raw, StringTrimmingEllipsisCharacter);
            let Ok(brush) = Owned::new(
                |p| GdipCreateSolidFill(color, p),
                |p: *mut GpSolidFill| {
                    GdipDeleteBrush(p.cast());
                },
            ) else {
                return;
            };
            let chars: Vec<_> = s.encode_utf16().collect();
            let bounds = RectF {
                X: b[0],
                Y: b[1],
                Width: b[2],
                Height: b[3],
            };
            GdipDrawString(
                self.g.raw,
                chars.as_ptr(),
                chars.len() as i32,
                font.raw,
                &bounds,
                format.raw,
                brush.raw.cast(),
            );
        }
    }
    fn pixels(&self, mut data: Vec<u8>, w: i32, h: i32, at: [f32; 4]) {
        unsafe {
            if let Ok(bitmap) = Owned::new(
                |p| GdipCreateBitmapFromScan0(w, h, w * 4, ARGB_32, data.as_mut_ptr(), p),
                |p: *mut GpBitmap| {
                    GdipDisposeImage(p.cast());
                },
            ) {
                GdipDrawImageRect(self.g.raw, bitmap.raw.cast(), at[0], at[1], at[2], at[3]);
            }
        }
    }
    fn row(&self, r: &Row, selected: bool) {
        let p = self.palette;
        match &r.item {
            Item::Header(demo) => {
                self.shape([4., 16., 32., 32.], 9., p.ink, false);
                if let Ok(data) = crate::tray_icon::render("", true, false, 22, p.dark) {
                    self.pixels(data, 22, 22, [9., 21., 22., 22.]);
                }
                self.text("QuotaBar", 16., [46., 13., 180., 23.], p.ink, true, false);
                self.text(
                    "사용량 한눈에 보기",
                    11.,
                    [47., 37., 200., 17.],
                    p.muted,
                    false,
                    false,
                );
                self.text(
                    if *demo {
                        "미리보기"
                    } else {
                        concat!("v", env!("QUOTABAR_VERSION"))
                    },
                    10.,
                    [244., 22., 86., 22.],
                    p.muted,
                    false,
                    true,
                );
            }
            Item::Caption(n) => {
                self.text(
                    "남은 사용량",
                    11.,
                    [4., 5., 120., 20.],
                    p.muted,
                    true,
                    false,
                );
                self.text(
                    &format!("{n}개 계정 · 기간별 잔여율"),
                    10.,
                    [130., 5., 202., 20.],
                    p.muted,
                    false,
                    true,
                );
            }
            Item::Empty(none) => {
                self.shape([0., 0., 336., 86.], 12., p.surface, false);
                self.text(
                    if *none {
                        "계정을 연결해 주세요"
                    } else {
                        "표시할 계정을 선택해 주세요"
                    },
                    13.,
                    [14., 13., 308., 24.],
                    p.ink,
                    true,
                    false,
                );
                self.text(
                    if *none {
                        "계정을 연결하면 남은 사용량을 여기서 볼 수 있어요."
                    } else {
                        "Dashboard에서 모니터링과 트레이 표시를 켜세요."
                    },
                    11.,
                    [14., 41., 308., 30.],
                    p.muted,
                    false,
                    false,
                );
            }
            Item::Account {
                provider,
                name,
                periods,
                uncertain,
                loading,
            } => {
                let height = r.height as f32 - 8.;
                let ink = if selected && p.hc {
                    system(COLOR_HIGHLIGHTTEXT)
                } else {
                    p.ink
                };
                self.shape(
                    [0., 0., 336., height],
                    12.,
                    if selected { p.hover } else { p.surface },
                    false,
                );
                self.shape(
                    [0.5, 0.5, 335., height - 1.],
                    12.,
                    if selected { p.muted } else { p.border },
                    true,
                );
                self.shape([12., 13., 30., 30.], 8., 0xffffffff, false);
                let idx = match provider.as_str() {
                    "Claude" => 1,
                    "Gemini" => 2,
                    _ => 0,
                };
                let inset = if idx == 1 { 4 } else { 2 };
                let size = 30 - 2 * inset;
                self.provider_logo(idx, 12 + inset, 13 + inset, size);
                self.text(provider, 12., [52., 9., 150., 20.], ink, true, false);
                self.text(name, 10., [52., 30., 263., 17.], p.muted, false, false);
                self.text(
                    if *loading {
                        "확인 중…"
                    } else if *uncertain {
                        "확인 필요"
                    } else {
                        "연결됨"
                    },
                    10.,
                    [212., 10., 110., 20.],
                    if *uncertain { p.warn } else { p.muted },
                    false,
                    true,
                );
                if periods.is_empty() {
                    self.text(
                        if *uncertain {
                            "사용량을 확인할 수 없습니다"
                        } else {
                            "첫 사용량을 기다리고 있어요"
                        },
                        12.,
                        [14., 58., 308., 20.],
                        if *uncertain { p.warn } else { p.muted },
                        false,
                        false,
                    );
                    self.text(
                        "계정 카드를 눌러 자세히 보기",
                        10.,
                        [14., 80., 308., 16.],
                        p.muted,
                        false,
                        false,
                    );
                }
                for (i, period) in periods.iter().enumerate() {
                    let x = 14. + (i % 2) as f32 * 162.;
                    let y = 56. + (i / 2) as f32 * 52.;
                    let w = if i % 2 == 0 && i == periods.len() - 1 {
                        308.
                    } else {
                        146.
                    };
                    let n = period.percent.clamp(0., 100.);
                    self.text(
                        &period.title,
                        10.,
                        [x, y, w - 64., 18.],
                        p.muted,
                        false,
                        false,
                    );
                    self.text(
                        &period.percent_label(),
                        12.,
                        [x + w - 63., y - 1., 63., 20.],
                        ink,
                        true,
                        true,
                    );
                    self.shape([x, y + 23., w, 5.], 2.5, p.track, false);
                    if n > 0. {
                        self.shape(
                            [x, y + 23., (n as f32 * w / 100.).max(3.), 5.],
                            2.5,
                            if n <= 20. {
                                p.warn
                            } else {
                                p.provider(provider)
                            },
                            false,
                        );
                    }
                    self.text(
                        &period.reset,
                        9.,
                        [x, y + 32., w + 2., 15.],
                        p.muted,
                        false,
                        false,
                    );
                }
                if *uncertain {
                    self.text(
                        if periods.is_empty() {
                            "Dashboard에서 연결 상태를 확인하세요"
                        } else {
                            "이전 값 포함 · Dashboard에서 연결 확인"
                        },
                        10.,
                        [14., height - 24., 308., 18.],
                        p.warn,
                        false,
                        false,
                    );
                }
            }
            Item::Divider => unsafe {
                if let Ok(pen) = Owned::new(
                    |out| GdipCreatePen1(p.border, 1., UnitWorld, out),
                    |p| {
                        GdipDeletePen(p);
                    },
                ) {
                    GdipDrawLine(self.g.raw, pen.raw, 4., 6., 332., 6.);
                }
            },
            Item::Dashboard | Item::Quit => {
                let primary = matches!(r.item, Item::Dashboard);
                let ink = if primary {
                    p.bg
                } else if selected && p.hc {
                    system(COLOR_HIGHLIGHTTEXT)
                } else {
                    p.ink
                };
                if primary || selected {
                    self.shape(
                        [0., 1., 336., if primary { 34. } else { 30. }],
                        8.,
                        if primary { p.ink } else { p.hover },
                        false,
                    );
                }
                self.action_glyph(primary, ink);
                self.text(
                    &r.label,
                    12.,
                    [39., if primary { 7. } else { 4. }, 264., 24.],
                    ink,
                    primary,
                    false,
                );
                if primary {
                    self.text("↗", 15., [304., 6., 20., 24.], ink, false, true);
                }
            }
            Item::Padding => {}
        }
    }
}
thread_local! {
    static LOGOS: [Option<Owned<GpBitmap>>; 3] = [
        include_bytes!("../../shared/assets/CodexLogo.png").as_slice(),
        include_bytes!("../../windows/assets/ClaudeLogo.png").as_slice(),
        include_bytes!("../../shared/assets/GeminiLogo.png").as_slice(),
    ].map(|bytes| load_logo(bytes).ok());
}
impl Graphics {
    fn provider_logo(&self, index: usize, x: i32, y: i32, size: i32) {
        LOGOS.with(|logos| {
            if let Some(bitmap) = &logos[index] {
                unsafe {
                    GdipDrawImageRect(
                        self.g.raw,
                        bitmap.raw.cast(),
                        x as f32,
                        y as f32,
                        size as f32,
                        size as f32,
                    );
                }
            }
        });
    }
}
fn load_logo(bytes: &[u8]) -> io::Result<Owned<GpBitmap>> {
    // Draw into an ARGB bitmap once, just like the original menu's Bitmap(Image).
    // Keeping the decoded source alive until this copy completes preserves PNG alpha.
    unsafe {
        let stream = Owned::new(
            |out| {
                *out = windows_sys::Win32::UI::Shell::SHCreateMemStream(
                    bytes.as_ptr(),
                    bytes.len() as u32,
                );
                if (*out).is_null() { OutOfMemory } else { 0 }
            },
            |ptr| {
                let vtable = *ptr.cast::<*const windows_sys::core::IUnknown_Vtbl>();
                ((*vtable).Release)(ptr);
            },
        )?;
        let source = Owned::new(
            |out| GdipLoadImageFromStream(stream.raw, out),
            |p| {
                GdipDisposeImage(p);
            },
        )?;
        crate::tray_icon::checked(GdipImageForceValidation(source.raw))?;
        let (mut width, mut height) = (0, 0);
        crate::tray_icon::checked(GdipGetImageWidth(source.raw, &mut width))?;
        crate::tray_icon::checked(GdipGetImageHeight(source.raw, &mut height))?;
        let (w, h) = (width as i32, height as i32);
        let copy = Owned::new(
            |out| GdipCreateBitmapFromScan0(w, h, 0, ARGB_32, null_mut(), out),
            |p: *mut GpBitmap| {
                GdipDisposeImage(p.cast());
            },
        )?;
        let dc = Owned::new(
            |out| GdipGetImageGraphicsContext(copy.raw.cast(), out),
            |p| {
                GdipDeleteGraphics(p);
            },
        )?;
        crate::tray_icon::checked(GdipGraphicsClear(dc.raw, 0x00ffffff))?;
        crate::tray_icon::checked(GdipDrawImageRect(
            dc.raw, source.raw, 0., 0., w as f32, h as f32,
        ))?;
        Ok(copy)
    }
}
/// WinForms draws its visible ToolStrip into an opaque color buffer. GDI+
/// produces monochrome glyphs on some redirected popup and child window DCs.
pub(super) fn buffered(dc: HDC, width: i32, height: i32, draw: impl Fn(HDC)) {
    if width <= 0 || height <= 0 {
        return;
    }
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let buffer = CreateCompatibleDC(dc);
        let mut bits = null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        let copied = if buffer.is_null() || bitmap.is_null() {
            false
        } else {
            let previous = SelectObject(buffer, bitmap);
            if previous.is_null() {
                false
            } else {
                draw(buffer);
                let copied = BitBlt(dc, 0, 0, width, height, buffer, 0, 0, SRCCOPY) != 0;
                SelectObject(buffer, previous);
                copied
            }
        };
        if !bitmap.is_null() {
            DeleteObject(bitmap);
        }
        if !buffer.is_null() {
            DeleteDC(buffer);
        }
        if !copied {
            draw(dc);
        }
    }
}

pub(super) fn background(menu: &Menu, dc: HDC) {
    let Ok(l) = menu.layout.try_borrow() else {
        return;
    };
    let p = Palette::new(&l.appearance);
    let Some(g) = Graphics::new(dc, p) else {
        return;
    };
    let s = menu.scale.get();
    unsafe {
        GdipGraphicsClear(g.g.raw, p.bg);
    }
    let mut rect = RECT::default();
    unsafe {
        GetClientRect(menu.window, &mut rect);
    }
    g.shape(
        [0.5, 0.5, rect.right as f32 - 1., rect.bottom as f32 - 1.],
        12. * s,
        p.border,
        true,
    );
    let arrow = menu.arrow_height.get();
    if arrow > 0 {
        unsafe {
            GdipSetClipRectI(
                g.g.raw,
                0,
                arrow,
                rect.right,
                rect.bottom - 2 * arrow,
                CombineModeIntersect,
            );
        }
    }
    for (i, row) in l.rows.iter().enumerate() {
        let y = menu.row_y(&l, row) as f32;
        if y < (rect.bottom - arrow) as f32 && y + row.height as f32 * s > arrow as f32 {
            if matches!(row.item, Item::Account { .. })
                && paint_account(
                    dc,
                    row,
                    menu.selected.get() == Some(i),
                    p,
                    s,
                    y as i32,
                    arrow..rect.bottom - arrow,
                )
            {
                continue;
            }
            g.transform((12. * s).round(), y, s);
            g.row(row, menu.selected.get() == Some(i));
        }
    }
    if arrow > 0 {
        g.transform(0., 0., 1.);
        unsafe {
            GdipResetClip(g.g.raw);
        }
        for (i, button) in menu.scroll_buttons.iter().enumerate() {
            let y = if i == 0 { 0 } else { rect.bottom - arrow };
            let enabled = unsafe {
                windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(*button)
            } != 0;
            scroll_arrow(&g, 1, y, rect.right - 2, arrow, i, enabled);
        }
    }
}
fn paint_account(
    dc: HDC,
    row: &Row,
    selected: bool,
    p: Palette,
    scale: f32,
    y: i32,
    clip: std::ops::Range<i32>,
) -> bool {
    // ToolStrip paints each card at its own origin. Translating the GDI+ image
    // interpolation over the whole menu changes its samples on lower rows.
    // The antialiased right stroke also touches the pixel just outside the card.
    let width = (336. * scale).ceil() as i32 + 1;
    let height = (row.height as f32 * scale).ceil() as i32;
    unsafe {
        let buffer = CreateCompatibleDC(dc);
        if buffer.is_null() {
            return false;
        }
        let bitmap = CreateCompatibleBitmap(dc, width, height);
        if bitmap.is_null() {
            DeleteDC(buffer);
            return false;
        }
        let previous = SelectObject(buffer, bitmap);
        let rendered = Graphics::new(buffer, p).is_some_and(|g| {
            GdipGraphicsClear(g.g.raw, p.bg);
            g.transform(0., 0., scale);
            g.row(row, selected);
            true
        });
        let first = clip.start.max(y);
        let last = clip.end.min(y + height);
        let copied = rendered
            && first < last
            && BitBlt(
                dc,
                (12. * scale).round() as i32,
                first,
                width,
                last - first,
                buffer,
                0,
                first - y,
                SRCCOPY,
            ) != 0;
        SelectObject(buffer, previous);
        DeleteObject(bitmap);
        DeleteDC(buffer);
        copied
    }
}
fn scroll_arrow(
    g: &Graphics,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    direction: usize,
    enabled: bool,
) {
    // Scroll controls cover the popup border along their top and bottom edges.
    unsafe {
        if let Ok(brush) = Owned::new(
            |p| GdipCreateSolidFill(g.palette.bg, p),
            |p: *mut GpSolidFill| {
                GdipDeleteBrush(p.cast());
            },
        ) {
            GdipSetSmoothingMode(g.g.raw, SmoothingModeNone);
            GdipFillRectangleI(g.g.raw, brush.raw.cast(), x, y, width, height);
            GdipSetSmoothingMode(g.g.raw, SmoothingModeAntiAlias);
        }
    }
    static ICONS: OnceLock<image::RgbaImage> = OnceLock::new();
    let icons = ICONS.get_or_init(|| {
        image::load_from_memory(include_bytes!("../../windows/assets/MenuScrollArrows.png"))
            .unwrap()
            .into_rgba8()
    });
    let size = height - 4;
    let index = SCROLL_ICON_WIDTHS
        .iter()
        .position(|&width| width as i32 == size);
    let Some(index) = index else { return };
    let w = SCROLL_ICON_WIDTHS[index];
    let mut pixels = image::imageops::crop_imm(
        icons,
        SCROLL_ICON_WIDTHS[..index].iter().sum(),
        direction as u32 * 64,
        w,
        w,
    )
    .to_image();
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
        if !enabled {
            // Windows Forms' disabled-label image transform, applied to a greyscale glyph.
            let value = (pixel[0] as f32 * 0.5063 + 96.9).round() as u8;
            pixel.0[..3].fill(value);
        }
    }
    g.pixels(
        pixels.into_raw(),
        w as i32,
        w as i32,
        [
            (x + (width - size) / 2) as f32,
            (y + 2) as f32,
            size as f32,
            size as f32,
        ],
    );
}
pub(super) fn child(menu: &Menu, s: &DRAWITEMSTRUCT, dc: HDC) {
    let Ok(l) = menu.layout.try_borrow() else {
        return;
    };
    let p = Palette::new(&l.appearance);
    let Some(g) = Graphics::new(dc, p) else {
        return;
    };
    unsafe {
        GdipGraphicsClear(g.g.raw, p.bg);
    }
    if let 1 | 2 = s.CtlID {
        scroll_arrow(
            &g,
            0,
            0,
            s.rcItem.right,
            s.rcItem.bottom,
            (s.CtlID - 1) as usize,
            s.itemState & ODS_DISABLED == 0,
        );
        return;
    }
    let i = s.CtlID.saturating_sub(100) as usize;
    let Some(row) = l.rows.get(i) else { return };
    g.transform(0., 0., menu.scale.get());
    g.row(row, menu.selected.get() == Some(i));
}
