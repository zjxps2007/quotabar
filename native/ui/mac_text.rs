use core_foundation::{
    attributed_string::CFMutableAttributedString,
    base::{CFRange, TCFType},
    boolean::CFBoolean,
    number::CFNumber,
    string::CFString,
};
use core_graphics::{
    base::kCGImageAlphaNoneSkipLast,
    color_space::CGColorSpace,
    context::CGContext,
    geometry::{CGPoint, CGRect, CGSize},
    path::CGPath,
};
use core_text::{
    font::{CTFont, CTFontRef},
    framesetter::CTFramesetter,
    line::CTLine,
    string_attributes::{
        kCTFontAttributeName, kCTForegroundColorFromContextAttributeName, kCTKernAttributeName,
        kCTUnderlineStyleAttributeName,
    },
};
use eframe::egui::{self, epaint::TextShape, *};
use objc2::rc::Retained;
use objc2_app_kit::{
    NSFont, NSFontWeightBold, NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold,
};
use std::collections::HashMap;

#[derive(Clone, Copy, Hash, Eq, PartialEq)]
pub enum PreferredFont {
    Body,
    Headline,
    Callout,
    CalloutBold,
    Title2Bold,
    Title3Bold,
    SubheadlineSemibold,
    Caption,
    CaptionSemibold,
    Caption2,
}

impl PreferredFont {
    fn base(self) -> Retained<NSFont> {
        use objc2_app_kit::{
            NSFontTextStyleBody, NSFontTextStyleCallout, NSFontTextStyleCaption1,
            NSFontTextStyleCaption2, NSFontTextStyleHeadline, NSFontTextStyleSubheadline,
            NSFontTextStyleTitle2, NSFontTextStyleTitle3,
        };
        unsafe {
            let style = match self {
                Self::Body => NSFontTextStyleBody,
                Self::Headline => NSFontTextStyleHeadline,
                Self::Callout | Self::CalloutBold => NSFontTextStyleCallout,
                Self::Title2Bold => NSFontTextStyleTitle2,
                Self::Title3Bold => NSFontTextStyleTitle3,
                Self::SubheadlineSemibold => NSFontTextStyleSubheadline,
                Self::Caption | Self::CaptionSemibold => NSFontTextStyleCaption1,
                Self::Caption2 => NSFontTextStyleCaption2,
            };
            NSFont::preferredFontForTextStyle_options(style, &objc2_foundation::NSDictionary::new())
        }
    }

    fn resolved(self) -> Retained<NSFont> {
        let font = self.base();
        if self == Self::CalloutBold {
            // SwiftUI applies the bold trait to the preferred callout face.
            // The resulting Latin and Korean faces differ from absolute system bold.
            let descriptor = font.fontDescriptor().fontDescriptorWithSymbolicTraits(
                objc2_app_kit::NSFontDescriptorSymbolicTraits::TraitBold,
            );
            return NSFont::fontWithDescriptor_size(&descriptor, 0.).unwrap_or(font);
        }
        let weight = unsafe {
            match self {
                Self::Title2Bold => NSFontWeightBold,
                Self::Title3Bold | Self::SubheadlineSemibold | Self::CaptionSemibold => {
                    NSFontWeightSemibold
                }
                _ => return font,
            }
        };
        NSFont::systemFontOfSize_weight(font.pointSize(), weight)
    }

    fn metrics(font: &NSFont, ui: &Ui) -> LineMetrics {
        let scale = f64::from(ui.pixels_per_point());
        let height = font.ascender() - font.descender() + font.leading();
        LineMetrics {
            height: ((height * scale).round() / scale) as f32,
            ascent: ((font.ascender() * scale).ceil() / scale) as f32,
        }
    }

    pub fn line_height(self, ui: &Ui) -> f32 {
        Self::metrics(&self.base(), ui).height
    }

    /// Paint inside a control that already owns its accessibility label.
    pub fn paint_at(self, ui: &Ui, top_left: Pos2, text: &str, color: Color32) {
        let font = self.base();
        let size = font.pointSize() as f32;
        if let Some(renderer) = ui.ctx().plugin_opt::<Renderer>()
            && let Some(run) =
                renderer
                    .lock()
                    .run(ui.ctx(), text, size, Face::Preferred(self), color)
        {
            let ascent = Self::metrics(&font, ui).ascent;
            ui.painter()
                .add(run.preferred_shape(top_left, ascent, ui.pixels_per_point(), color));
        } else {
            ui.painter().text(
                top_left,
                Align2::LEFT_TOP,
                text,
                FontId::proportional(size),
                color,
            );
        }
    }

    pub fn text_width(self, ui: &Ui, text: &str) -> f32 {
        let size = self.base().pointSize() as f32;
        let ink = ui.visuals().text_color();
        let native = ui.ctx().plugin_opt::<Renderer>().and_then(|renderer| {
            Some(
                renderer
                    .lock()
                    .run(ui.ctx(), text, size, Face::Preferred(self), ink)?
                    .size
                    .x,
            )
        });
        let width = native.unwrap_or_else(|| {
            ui.painter()
                .layout_no_wrap(text.to_owned(), FontId::proportional(size), ink)
                .size()
                .x
        });
        (width * ui.pixels_per_point()).ceil() / ui.pixels_per_point()
    }
}

#[derive(Clone, Copy, Hash, Eq, PartialEq)]
pub enum DigitWeight {
    Regular,
    Medium,
    Semibold,
}

#[derive(Clone, Copy, Hash, Eq, PartialEq)]
enum Face {
    Regular,
    Medium,
    Monospace,
    Underlined,
    Emphasized,
    Bold,
    TabularDigits(DigitWeight),
    RoundedDigits,
    Preferred(PreferredFont),
}

impl Face {
    fn label(heavy: bool) -> Self {
        if heavy {
            Self::Emphasized
        } else {
            Self::Regular
        }
    }
}

#[derive(Hash, Eq, PartialEq)]
struct Key {
    text: String,
    size: u32,
    scale: u32,
    face: Face,
    tracking: u32,
    truncation: Truncation,
    light_ink: bool,
    ink_alpha: u8,
}

#[derive(Clone, Copy, Default, Hash, Eq, PartialEq)]
enum Truncation {
    #[default]
    None,
    End(u32),
    Middle(u32),
}

fn attributed(value: &str, size: f64, face: Face, tracking: f32) -> CFMutableAttributedString {
    // NSFont and CTFont are toll-free bridged. Use the AppKit font at its
    // logical point size so Retina drawing preserves its optical size.
    let weight = unsafe {
        match face {
            Face::Regular
            | Face::Monospace
            | Face::Underlined
            | Face::Preferred(_)
            | Face::TabularDigits(DigitWeight::Regular) => NSFontWeightRegular,
            Face::Medium | Face::TabularDigits(DigitWeight::Medium) => NSFontWeightMedium,
            Face::Bold => NSFontWeightBold,
            Face::Emphasized if size >= 17. => NSFontWeightBold,
            Face::Emphasized | Face::TabularDigits(DigitWeight::Semibold) | Face::RoundedDigits => {
                NSFontWeightSemibold
            }
        }
    };
    let nsfont = if let Face::Preferred(style) = face {
        style.resolved()
    } else if face == Face::RoundedDigits {
        let digits = NSFont::monospacedDigitSystemFontOfSize_weight(size, weight);
        digits
            .fontDescriptor()
            .fontDescriptorWithDesign(unsafe { objc2_app_kit::NSFontDescriptorSystemDesignRounded })
            .and_then(|descriptor| NSFont::fontWithDescriptor_size(&descriptor, size))
            .unwrap_or(digits)
    } else if face == Face::Monospace {
        NSFont::monospacedSystemFontOfSize_weight(size, weight)
    } else if matches!(face, Face::TabularDigits(_)) {
        NSFont::monospacedDigitSystemFontOfSize_weight(size, weight)
    } else {
        NSFont::systemFontOfSize_weight(size, weight)
    };
    let font = unsafe { CTFont::wrap_under_get_rule(Retained::as_ptr(&nsfont) as CTFontRef) };
    let mut text = CFMutableAttributedString::new();
    text.replace_str(&CFString::new(value), CFRange::init(0, 0));
    let range = CFRange::init(0, text.char_len());
    text.set_attribute(range, unsafe { kCTFontAttributeName }, &font);
    if tracking != 0. {
        text.set_attribute(
            range,
            unsafe { kCTKernAttributeName },
            &CFNumber::from(f64::from(tracking)),
        );
    }
    if face == Face::Underlined {
        text.set_attribute(
            range,
            unsafe { kCTUnderlineStyleAttributeName },
            &CFNumber::from(1_i32),
        );
    }
    text.set_attribute(
        range,
        unsafe { kCTForegroundColorFromContextAttributeName },
        &CFBoolean::true_value(),
    );
    text
}

#[derive(Clone, Copy)]
struct LineMetrics {
    height: f32,
    ascent: f32,
}

fn fitted_line(
    text: &mut CFMutableAttributedString,
    mode: objc2_app_kit::NSLineBreakMode,
    width: f64,
    height: f64,
    scale: f64,
) -> Option<CTLine> {
    use objc2_app_kit::{NSMutableParagraphStyle, NSParagraphStyleAttributeName};
    use objc2_foundation::{NSMutableAttributedString, NSRange};

    if width <= 0. {
        return None;
    }
    let style = NSMutableParagraphStyle::new();
    style.setLineBreakMode(mode);
    // NSMutableAttributedString is toll-free bridged with CFMutableAttributedString.
    unsafe {
        let native = &*text
            .as_concrete_TypeRef()
            .cast::<NSMutableAttributedString>();
        native.addAttribute_value_range(
            NSParagraphStyleAttributeName,
            &style,
            NSRange::new(0, native.length()),
        );
    }
    let setter = CTFramesetter::new_with_attributed_string(text.as_concrete_TypeRef());
    let range = CFRange::init(0, 0);
    // SwiftUI measures the label in the available space, then draws it in its
    // allocated pixel-aligned bounds. Core Text may choose a different suffix
    // on the second layout even when both results fit the original proposal.
    let (fit, _) = setter.suggest_frame_size_with_constraints(
        range,
        std::ptr::null(),
        CGSize::new(width, height),
    );
    let fitted = ((fit.width * scale).ceil() / scale).min(width);
    if fitted <= 0. {
        return None;
    }
    let path = CGPath::from_rect(
        CGRect::new(&CGPoint::new(0., 0.), &CGSize::new(fitted, height)),
        None,
    );
    setter
        .create_frame(range, &path)
        .get_lines()
        .into_iter()
        .next()
}

fn raster(key: &Key, limit: usize) -> Option<(ColorImage, Vec2, LineMetrics)> {
    let size = f64::from(f32::from_bits(key.size));
    let scale = f64::from(f32::from_bits(key.scale));
    let make_line = |face| {
        use objc2_app_kit::NSLineBreakMode;
        let mut text = attributed(&key.text, size, face, f32::from_bits(key.tracking));
        let line = CTLine::new_with_attributed_string(text.as_concrete_TypeRef());
        let (width, kind) = match key.truncation {
            Truncation::None => return line,
            Truncation::End(bits) => (
                f64::from(f32::from_bits(bits)),
                NSLineBreakMode::ByTruncatingTail,
            ),
            Truncation::Middle(bits) => (
                f64::from(f32::from_bits(bits)),
                NSLineBreakMode::ByTruncatingMiddle,
            ),
        };
        let bounds = line.get_typographic_bounds();
        if bounds.width <= width {
            return line;
        }
        let height = (bounds.ascent + bounds.descent + bounds.leading).ceil();
        fitted_line(&mut text, kind, width, height, scale).unwrap_or_else(|| {
            CTLine::new_with_attributed_string(
                CFMutableAttributedString::new().as_concrete_TypeRef(),
            )
        })
    };
    let line = make_line(key.face);
    let bounds = line.get_typographic_bounds();
    let ascent = (bounds.ascent * scale).ceil();
    let height = (ascent + ((bounds.descent + bounds.leading) * scale).ceil()).max(1.) as usize;
    let width = (bounds.width * scale).ceil().max(1.) as usize;
    if width > limit || height > limit || width.checked_mul(height)? > 8_000_000 {
        return None;
    }
    let mask = |line: &CTLine, light_ink: bool, alpha: u8| -> Vec<Color32> {
        let mut canvas = CGContext::create_bitmap_context(
            None,
            width,
            height,
            8,
            width * 4,
            &CGColorSpace::create_device_rgb(),
            kCGImageAlphaNoneSkipLast,
        );
        // Quartz smooths light ink on a dark surface differently from dark ink on
        // a light surface. A transparent alpha-only mask loses that distinction.
        let ink = if light_ink { 1. } else { 0. };
        let paper = 1. - ink;
        canvas.set_rgb_fill_color(paper, paper, paper, 1.);
        canvas.fill_rect(CGRect::new(
            &CGPoint::new(0., 0.),
            &CGSize::new(width as f64, height as f64),
        ));
        canvas.scale(scale, scale);
        // Let Quartz composite the semantic label opacity with its glyph coverage
        // in one pass. Multiplying an 8-bit mask by the label opacity later loses
        // a level of edge coverage on both light and dark backgrounds.
        canvas.set_rgb_fill_color(ink, ink, ink, f64::from(alpha) / 255.);
        canvas.set_should_antialias(true);
        canvas.set_allows_font_smoothing(true);
        canvas.set_should_smooth_fonts(true);
        canvas.set_text_position(0., (height as f64 - ascent) / scale);
        line.draw(&canvas);
        canvas
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|rgb| {
                let shade =
                    ((u16::from(rgb[0]) + u16::from(rgb[1]) + u16::from(rgb[2]) + 1) / 3) as u8;
                Color32::from_white_alpha(if light_ink { shade } else { 255 - shade })
            })
            .collect()
    };
    let pixels = if key.face == Face::Underlined && key.light_ink {
        // Core Text can draw an underline in black even when the context draws
        // white glyphs. Isolate its coverage using two black-on-white runs.
        // This retains the fallback fonts' underline positions at each scale.
        let plain = make_line(Face::Regular);
        let mut pixels = mask(&plain, true, key.ink_alpha);
        let glyphs = mask(&plain, false, 255);
        let decorated = mask(&line, false, 255);
        for ((pixel, glyph), mark) in pixels.iter_mut().zip(glyphs).zip(decorated) {
            let added = mark.a().saturating_sub(glyph.a());
            if added > 0 {
                // Ink covers this fraction of the area left clear by the glyph.
                let clear = u32::from(255 - glyph.a());
                let gain = u32::from(key.ink_alpha.saturating_sub(pixel.a())) * u32::from(added);
                *pixel = Color32::from_white_alpha(pixel.a() + ((gain + clear / 2) / clear) as u8);
            }
        }
        pixels
    } else {
        mask(&line, key.light_ink, key.ink_alpha)
    };
    let line_pixels = (bounds.ascent + bounds.descent + bounds.leading) * scale;
    Some((
        ColorImage::new([width, height], pixels),
        // The texture needs room on both sides of the baseline, while AppKit
        // rounds the whole logical line box once when stacking controls.
        vec2(bounds.width as f32, (line_pixels.round() / scale) as f32),
        LineMetrics {
            height: (line_pixels.ceil() / scale) as f32,
            ascent: (ascent / scale) as f32,
        },
    ))
}

#[derive(Clone)]
struct Run {
    texture: TextureHandle,
    size: Vec2,
    metrics: LineMetrics,
    last_pass: u64,
}

#[derive(Default)]
pub struct Renderer {
    runs: HashMap<Key, Run>,
    pass: u64,
}

impl Renderer {
    fn run(
        &mut self,
        ctx: &Context,
        text: &str,
        size: f32,
        face: Face,
        color: Color32,
    ) -> Option<&Run> {
        self.tracked_run(ctx, text, size, face, color, 0., Truncation::None)
    }

    fn tracked_run(
        &mut self,
        ctx: &Context,
        text: &str,
        size: f32,
        face: Face,
        color: Color32,
        tracking: f32,
        truncation: Truncation,
    ) -> Option<&Run> {
        let [r, g, b, _] = color.to_srgba_unmultiplied();
        let ink = Color32::from_rgb(r, g, b).intensity();
        let key = Key {
            text: text.to_owned(),
            size: size.to_bits(),
            scale: ctx.pixels_per_point().to_bits(),
            face,
            tracking: tracking.to_bits(),
            truncation,
            light_ink: ink > ctx.style().visuals.panel_fill.intensity(),
            ink_alpha: if color.is_additive() { 255 } else { color.a() },
        };
        let cached = match self.runs.entry(key) {
            std::collections::hash_map::Entry::Occupied(slot) => slot.into_mut(),
            std::collections::hash_map::Entry::Vacant(slot) => {
                let (image, size, metrics) = raster(slot.key(), ctx.input(|i| i.max_texture_side))?;
                slot.insert(Run {
                    texture: ctx.load_texture("Core Text", image, TextureOptions::NEAREST),
                    size,
                    metrics,
                    last_pass: self.pass,
                })
            }
        };
        cached.last_pass = self.pass;
        Some(cached)
    }

    fn text_shape(&mut self, ctx: &Context, text: &TextShape) -> Option<Shape> {
        let [section] = text.galley.job.sections.as_slice() else {
            return None;
        };
        let format = &section.format;
        let face = match &format.font_id.family {
            FontFamily::Proportional => Face::Regular,
            FontFamily::Monospace => Face::Monospace,
            FontFamily::Name(name) if name.as_ref() == "system-semibold" => Face::Emphasized,
            _ => return None,
        };
        if text.angle != 0.
            || format.italics
            || !text.underline.is_empty()
            || text
                .galley
                .rows
                .iter()
                .any(|row| row.visuals.glyph_vertex_range != (0..row.visuals.mesh.vertices.len()))
        {
            return None;
        }
        let mut color = text.override_text_color.unwrap_or(format.color);
        if color == Color32::PLACEHOLDER {
            color = text.fallback_color;
        }
        color = color.gamma_multiply(text.opacity_factor);
        let mut shapes = Vec::new();
        for row in &text.galley.rows {
            if row.glyphs.is_empty() {
                continue;
            }
            let string: String = row.glyphs.iter().map(|g| g.chr).collect();
            let run = self.tracked_run(
                ctx,
                &string,
                format.font_id.size,
                face,
                color,
                format.extra_letter_spacing,
                Truncation::None,
            )?;
            let origin = text.pos + row.pos.to_vec2();
            let scale = ctx.pixels_per_point();
            let shape = if format.line_height.is_some() {
                run.centered_shape(origin + vec2(0., row.size.y / 2.), scale, color)
            } else {
                run.shape(
                    origin + vec2(0., (row.size.y - run.size.y) * 0.5),
                    scale,
                    color,
                )
            };
            shapes.push(shape);
        }
        Some(Shape::Vec(shapes))
    }
}

impl Run {
    fn preferred_shape(&self, origin: Pos2, ascent: f32, scale: f32, color: Color32) -> Shape {
        self.shape(
            origin + vec2(0., (ascent - self.metrics.ascent).max(0.)),
            scale,
            color,
        )
    }

    fn centered_shape(&self, left_center: Pos2, scale: f32, color: Color32) -> Shape {
        // Center the Core Text bitmap, including its separately rounded ascent
        // and descent. AppKit puts an odd leftover backing pixel below the run.
        let top = (left_center.y * scale - self.texture.size_vec2().y / 2.).floor() / scale;
        self.shape(pos2(left_center.x, top), scale, color)
    }

    fn shape(&self, pos: Pos2, scale: f32, color: Color32) -> Shape {
        let pos = pos2(
            (pos.x * scale).round() / scale,
            (pos.y * scale).round() / scale,
        );
        let tint = if color.is_opaque() || color.is_additive() {
            color
        } else {
            let [r, g, b, _] = color.to_srgba_unmultiplied();
            Color32::from_rgb(r, g, b)
        };
        Shape::image(
            self.texture.id(),
            Rect::from_min_size(pos, self.texture.size_vec2() / scale),
            Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
            tint,
        )
    }
}

pub fn natural_size(ui: &Ui, text: &str, size: f32, heavy: bool) -> Option<Vec2> {
    let handle = ui.ctx().plugin_opt::<Renderer>()?;
    Some(
        handle
            .lock()
            .run(
                ui.ctx(),
                text,
                size,
                Face::label(heavy),
                ui.visuals().text_color(),
            )?
            .size,
    )
}

pub fn calendar_label(ui: &Ui, at: Pos2, align: Align2, text: &str, size: f32, color: Color32) {
    if let Some(renderer) = ui.ctx().plugin_opt::<Renderer>()
        && let Some(run) = renderer
            .lock()
            .run(ui.ctx(), text, size, Face::Medium, color)
    {
        let rect = align.anchor_size(at, run.size);
        ui.painter()
            .add(run.shape(rect.min, ui.ctx().pixels_per_point(), color));
    } else {
        ui.painter()
            .text(at, align, text, FontId::proportional(size), color);
    }
}

pub fn navigation_title(ui: &Ui, start: Pos2, text: &str, selected: bool, color: Color32) {
    let scale = ui.ctx().pixels_per_point();
    let advance = system_line_height(ui, 12.);
    let ascent = (NSFont::systemFontOfSize(12.).ascender() * f64::from(scale)).ceil() as f32 / scale;
    let renderer = ui.ctx().plugin_opt::<Renderer>();
    for (index, line) in text.lines().enumerate() {
        let top = start + vec2(0., index as f32 * advance);
        if let Some(renderer) = &renderer
            && let Some(run) =
                renderer
                    .lock()
                    .run(ui.ctx(), line, 12., Face::label(selected), color)
        {
            // A fallback face shares the system font's line box in SwiftUI.
            ui.painter()
                .add(run.preferred_shape(top, ascent, scale, color));
        } else {
            ui.painter().text(
                top,
                Align2::LEFT_TOP,
                line,
                FontId::proportional(12.),
                color,
            );
        }
    }
}

pub fn system_line_height(ui: &Ui, size: f64) -> f32 {
    // The control reserves its system font's line box even if every glyph
    // comes from a shorter fallback face, e.g. an all-Hangul button title.
    let scale = f64::from(ui.ctx().pixels_per_point());
    ((font_line_height(size) * scale).ceil() / scale) as f32
}

pub fn title_stack_height(ui: &Ui, title: f64, subtitle: f64, spacing: f64) -> f32 {
    let scale = f64::from(ui.ctx().pixels_per_point());
    let total = font_line_height(title) + spacing + font_line_height(subtitle);
    ((total * scale).ceil() / scale) as f32
}

fn font_line_height(size: f64) -> f64 {
    let font = NSFont::systemFontOfSize(size);
    font.ascender() - font.descender() + font.leading()
}

pub fn label(ui: &mut Ui, text: &str, size: f32, heavy: bool, color: Color32) -> Option<Response> {
    draw_label(ui, text, size, Face::label(heavy), color, LineBox::Glyphs)
}

pub fn centered_label(
    ui: &mut Ui,
    text: &str,
    size: f32,
    heavy: bool,
    color: Color32,
) -> Option<Response> {
    draw_label(ui, text, size, Face::label(heavy), color, LineBox::Centered)
}

pub fn picker_label(ui: &mut Ui, text: &str, color: Color32) -> Option<Response> {
    draw_label(ui, text, 13., Face::Regular, color, LineBox::Control)
}

pub fn system_label(
    ui: &mut Ui,
    text: &str,
    size: f32,
    heavy: bool,
    color: Color32,
) -> Option<Response> {
    // A text line rounds its total extent once. A button reserves the next
    // full device pixel for its alignment box in system_line_height.
    draw_label(ui, text, size, Face::label(heavy), color, LineBox::System)
}

fn covers_text(font: &NSFont, text: &str) -> bool {
    let covered = font.coveredCharacterSet();
    text.chars().all(|ch| covered.longCharacterIsMember(ch as u32))
}

pub fn dashboard_heading(ui: &mut Ui, text: &str, color: Color32) -> Option<Response> {
    // SwiftUI centers a system-face title on backing pixels. A substituted
    // face sits in the logical system line box, which preserves its baseline.
    let line = if covers_text(&NSFont::boldSystemFontOfSize(25.), text) {
        LineBox::CenteredSystem
    } else {
        LineBox::System
    };
    draw_label(ui, text, 25., Face::Bold, color, line)
}

pub fn stat_title(ui: &mut Ui, text: &str, color: Color32) -> Option<Response> {
    // Keep the system baseline when Core Text adds fallback-font leading to
    // the bitmap. Centering its rounded logical height can move the title.
    let metrics = PreferredFont::metrics(&NSFont::systemFontOfSize(11.), ui);
    draw_label(
        ui,
        text,
        11.,
        Face::Regular,
        color,
        LineBox::Preferred {
            height: metrics.height,
            ascent: metrics.ascent,
        },
    )
}

pub fn stat_value(ui: &mut Ui, text: &str, color: Color32) -> Option<Response> {
    draw_label(
        ui,
        text,
        28.,
        Face::RoundedDigits,
        color,
        LineBox::CenteredStack,
    )
}

pub fn section_heading(ui: &mut Ui, text: &str, color: Color32) -> Option<Response> {
    centered_stack_label(ui, text, 15., true, color)
}

pub fn centered_stack_label(
    ui: &mut Ui, text: &str, size: f32, heavy: bool, color: Color32,
) -> Option<Response> {
    draw_label(ui, text, size, Face::label(heavy), color, LineBox::CenteredStack)
}

pub fn stat_caption(ui: &mut Ui, text: &str, size: f32, color: Color32) -> Option<Response> {
    // A system-face-only caption sits on its normal baseline. When a fallback
    // face supplies glyphs, SwiftUI centers the bitmap inside the stack line.
    let line = if covers_text(&NSFont::systemFontOfSize(f64::from(size)), text) {
        LineBox::Stack
    } else {
        LineBox::CenteredStack
    };
    draw_label(ui, text, size, Face::Regular, color, line)
}

pub fn stacked_label(
    ui: &mut Ui,
    text: &str,
    size: f32,
    heavy: bool,
    color: Color32,
) -> Option<Response> {
    draw_label(ui, text, size, Face::label(heavy), color, LineBox::Stack)
}

pub fn plan_label(ui: &mut Ui, text: &str, color: Color32) -> Option<Response> {
    medium_label(ui, text, 9., color)
}

struct CardLine {
    run: Run,
    size: Vec2,
    ascent: f32,
}

impl CardLine {
    fn measure(
        ui: &Ui,
        text: &str,
        points: f32,
        face: Face,
        color: Color32,
        truncation: Truncation,
    ) -> Option<Self> {
        if text.is_empty() || text.contains('\n') {
            return None;
        }
        let handle = ui.ctx().plugin_opt::<Renderer>()?;
        let run = handle
            .lock()
            .tracked_run(ui.ctx(), text, points, face, color, 0., truncation)?
            .clone();
        let scale = ui.pixels_per_point();
        let font = NSFont::systemFontOfSize(f64::from(points));
        // SwiftUI reserves both sides of the system baseline even when the
        // label is drawn entirely with a shorter fallback face.
        let ascent = font.ascender().ceil() as f32;
        let descent = (font.leading() - font.descender()).ceil() as f32;
        let size = vec2(
            (run.size.x * scale).ceil() / scale,
            (ascent + descent).max(run.metrics.height),
        );
        Some(Self { run, size, ascent })
    }

    fn paint(&self, ui: &Ui, response: &Response, text: &str, color: Color32) {
        ui.painter().add(self.run.preferred_shape(
            response.rect.min,
            self.ascent,
            ui.pixels_per_point(),
            color,
        ));
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), text));
    }
}

pub fn quota_card_titles(
    ui: &mut Ui,
    account: &crate::model::Account,
    p: crate::style::Palette,
) -> Option<()> {
    let provider = CardLine::measure(
        ui,
        account.provider,
        13.,
        Face::Emphasized,
        p.ink,
        Truncation::None,
    )?;
    let plan = match account.snapshot["plan"].as_str() {
        Some(text) => Some((
            text,
            CardLine::measure(ui, text, 9., Face::Medium, p.ink, Truncation::None)?,
        )),
        None => None,
    };
    let row = plan.as_ref().map_or(provider.size, |(_, line)| {
        vec2(
            provider.size.x + 8. + line.size.x + 12.,
            provider.size.y.max(line.size.y + 6.),
        )
    });
    // The trailing spacer and optional progress indicator also take part in
    // the HStack's width proposal to the single-line account name.
    let available = ui.available_width() - 12. - if account.loading { 23. } else { 0. };
    if available < row.x {
        return None;
    }
    let alias = CardLine::measure(
        ui,
        &account.name,
        11.,
        Face::Regular,
        p.muted,
        Truncation::End(available.to_bits()),
    )?;
    let size = vec2(row.x.max(alias.size.x), row.y + 3. + alias.size.y);
    let (id, rect) = ui.allocate_space(size);
    let widget = |key, offset: Vec2, size| {
        ui.interact(
            Rect::from_min_size(rect.min + offset, size),
            id.with(key),
            Sense::hover(),
        )
    };
    let provider_box = widget(
        "provider",
        vec2(0., (row.y - provider.size.y) / 2.),
        provider.size,
    );
    provider.paint(ui, &provider_box, account.provider, p.ink);
    if let Some((text, line)) = plan {
        let badge = Rect::from_min_size(
            rect.min + vec2(provider.size.x + 8., (row.y - line.size.y - 6.) / 2.),
            line.size + vec2(12., 6.),
        );
        crate::mac::rounded_rect(
            ui,
            badge,
            badge.height() / 2.,
            crate::style::surface_fill(p.card, p.ink, 0.05),
            Stroke::NONE,
            crate::mac::Curve::Circular,
        );
        let plan_box = widget("plan", badge.min - rect.min + vec2(6., 3.), line.size);
        line.paint(ui, &plan_box, text, p.ink);
    }
    let alias_box = widget("alias", vec2(0., row.y + 3.), alias.size);
    alias.paint(ui, &alias_box, &account.name, p.muted);
    crate::style::tooltip_text(alias_box, &account.name);
    Some(())
}

pub fn snapshot_label(ui: &mut Ui, text: &str, color: Color32) -> Response {
    if let Some(line) = CardLine::measure(ui, text, 10., Face::Regular, color, Truncation::None)
        && line.size.x <= ui.available_width()
    {
        let (_, response) = ui.allocate_exact_size(line.size, Sense::hover());
        line.paint(ui, &response, text, color);
        return response;
    }
    crate::style::system_label(ui, text, 10., false, color)
}

pub fn medium_label(ui: &mut Ui, text: &str, size: f32, color: Color32) -> Option<Response> {
    draw_label(ui, text, size, Face::Medium, color, LineBox::Stack)
}

pub fn preferred_label(ui: &mut Ui, text: &str, style: PreferredFont, color: Color32) -> Response {
    let font = style.base();
    let size = font.pointSize() as f32;
    // The preferred font supplies line spacing even when its glyphs come
    // from a fallback face. In particular caption and caption2 both reserve
    // 13 points on macOS, but caption2 uses the system's medium face.
    let line = if matches!(style, PreferredFont::Title2Bold | PreferredFont::Title3Bold) {
        LineBox::CenteredStack
    } else {
        let LineMetrics { height, ascent } = PreferredFont::metrics(&font, ui);
        LineBox::Preferred { height, ascent }
    };
    draw_label(ui, text, size, Face::Preferred(style), color, line).unwrap_or_else(|| {
        let heavy = matches!(
            style,
            PreferredFont::Headline
                | PreferredFont::CalloutBold
                | PreferredFont::Title2Bold
                | PreferredFont::Title3Bold
                | PreferredFont::SubheadlineSemibold
                | PreferredFont::CaptionSemibold
        );
        ui.label(crate::style::rich(text, size, heavy, color))
    })
}

pub fn quota_percent(ui: &mut Ui, text: &str, color: Color32) -> Option<Response> {
    draw_label(
        ui,
        text,
        11.,
        Face::TabularDigits(DigitWeight::Semibold),
        color,
        LineBox::CenteredStack,
    )
}

pub fn tabular_label(
    ui: &mut Ui,
    text: &str,
    size: f32,
    weight: DigitWeight,
    color: Color32,
) -> Response {
    draw_label(
        ui,
        text,
        size,
        Face::TabularDigits(weight),
        color,
        LineBox::Glyphs,
    )
    .unwrap_or_else(|| crate::style::label(ui, text, size, weight == DigitWeight::Semibold, color))
}

pub fn activity_token_total(
    ui: &mut Ui,
    total: &str,
    palette: crate::style::Palette,
) -> Option<()> {
    let handle = ui.ctx().plugin_opt::<Renderer>()?;
    let mut renderer = handle.lock();
    let caption = renderer
        .run(ui.ctx(), "Tokens", 8.5, Face::Regular, palette.muted)?
        .clone();
    let amount = renderer
        .run(
            ui.ctx(),
            total,
            13.5,
            Face::TabularDigits(DigitWeight::Semibold),
            palette.ink,
        )?
        .clone();
    drop(renderer);
    let scale = ui.pixels_per_point();
    let system_height = (font_line_height(8.5) * f64::from(scale)).round() as f32 / scale;
    let caption_height = caption.size.y.max(system_height);
    // SwiftUI proposes whole backing pixels to both trailing-aligned Text views.
    for (run, height, text, color) in [
        (&caption, caption_height, "Tokens", palette.muted),
        (&amount, amount.size.y, total, palette.ink),
    ] {
        let width = (run.size.x * scale).ceil() / scale;
        let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        ui.painter()
            .add(run.centered_shape(rect.left_center(), scale, color));
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), text));
    }
    Some(())
}

pub fn activity_model_row(
    ui: &mut Ui,
    model: &str,
    detail: &str,
    total: &str,
    tint: Color32,
    palette: crate::style::Palette,
) -> Option<()> {
    let handle = ui.ctx().plugin_opt::<Renderer>()?;
    let mut renderer = handle.lock();
    let width = ui.available_width();
    let scale = ui.pixels_per_point();
    let amount = renderer
        .run(
            ui.ctx(),
            total,
            10.,
            Face::TabularDigits(DigitWeight::Medium),
            palette.ink,
        )?
        .clone();
    let amount_width = (amount.size.x * scale).ceil() / scale;
    // The HStack has a six-point dot, three six-point gaps and a Spacer
    // with a six-point minimum between the two text columns.
    let limit = (width - 30. - amount_width).max(0.).to_bits();
    let title = renderer
        .tracked_run(
            ui.ctx(),
            model,
            10.5,
            Face::Medium,
            palette.ink,
            0.,
            Truncation::Middle(limit),
        )?
        .clone();
    let counts = renderer
        .tracked_run(
            ui.ctx(),
            detail,
            8.5,
            Face::TabularDigits(DigitWeight::Regular),
            palette.muted,
            0.,
            Truncation::End(limit),
        )?
        .clone();
    drop(renderer);

    // SwiftUI aligns the circle's bottom, the model's first text baseline
    // and the total's baseline. Its second line belongs to the model column.
    let baseline = title.metrics.ascent.max(amount.metrics.ascent).max(6.);
    let title_y = baseline - title.metrics.ascent;
    let counts_y = title_y + title.size.y + 1.;
    let amount_y = baseline - amount.metrics.ascent;
    let height = (counts_y + counts.metrics.height).max(amount_y + amount.metrics.height);
    let (row, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    // The model's texture includes separately rounded ascent and descent.
    // Center it in its logical first line before aligning the circle and total.
    let center = (row.top() + title_y + title.size.y * 0.5) * scale;
    let title_top = (center - title.texture.size_vec2().y * 0.5).floor() / scale;
    let ink_baseline = title_top + title.metrics.ascent;
    ui.painter()
        .circle_filled(pos2(row.left() + 3., ink_baseline - 3.), 3., tint);
    let amount_top = ink_baseline - amount.metrics.ascent;
    for (key, run, text, x, top, color) in [
        ("model", title, model, 12., title_top, palette.ink),
        (
            "counts",
            counts,
            detail,
            12.,
            row.top() + counts_y,
            palette.muted,
        ),
        (
            "total",
            amount,
            total,
            width - amount_width,
            amount_top,
            palette.ink,
        ),
    ] {
        let pos = pos2(row.left() + x, top);
        ui.painter().add(run.shape(pos, scale, color));
        let label = ui.interact(
            Rect::from_min_size(pos, run.size),
            response.id.with(key),
            Sense::hover(),
        );
        label.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), text));
    }
    Some(())
}

pub fn symbol_label(ui: &mut Ui, name: &str, text: &str, style: PreferredFont, color: Color32) {
    let font = style.base();
    let points = font.pointSize() as f32;
    let line = PreferredFont::metrics(&font, ui).height;
    let height =
        crate::mac::symbol_size(ui.ctx(), name, points, false).map_or(line, |s| s.y.max(line));
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), height),
        Layout::left_to_right(Align::Center),
        |ui| {
            // A SwiftUI Label uses its own icon gap inside the wider account grid.
            ui.spacing_mut().item_spacing.x = 8.;
            crate::mac::intrinsic_symbol(ui, name, points, false, color);
            preferred_label(ui, text, style, color);
        },
    );
}

pub fn plain_button(ui: &mut Ui, title: &str, color: Color32) -> Response {
    let points = NSFont::systemFontSize() as f32;
    text_button(
        ui,
        title,
        color,
        Face::Regular,
        points,
        system_line_height(ui, f64::from(points)),
    )
}

pub fn caption_button(ui: &mut Ui, title: &str, color: Color32) -> Response {
    let style = PreferredFont::Caption;
    let font = style.base();
    text_button(
        ui,
        title,
        color,
        Face::Preferred(style),
        font.pointSize() as f32,
        PreferredFont::metrics(&font, ui).height,
    )
}

fn text_button(
    ui: &mut Ui,
    title: &str,
    color: Color32,
    face: Face,
    points: f32,
    height: f32,
) -> Response {
    let native = ui.ctx().plugin_opt::<Renderer>().and_then(|renderer| {
        use eframe::emath::GuiRounding;
        let mut renderer = renderer.lock();
        let run = renderer.run(ui.ctx(), title, points, face, color)?;
        let scale = ui.pixels_per_point();
        let size = vec2((run.size.x * scale).ceil() / scale, height);
        let (id, slot) = ui.allocate_space(size);
        let rect = Rect::from_min_size(slot.min.round_to_pixels(scale), size);
        let response = ui.interact(rect, id, Sense::click());
        // SwiftUI lays out a plain button's Text using the typographic line.
        // A Core Text bitmap can have an extra pixel of ascent or descent.
        let top = pos2(rect.left(), rect.center().y - run.size.y * 0.5);
        ui.painter().add(run.shape(top, scale, color));
        Some(response)
    });
    let response = native.unwrap_or_else(|| {
        ui.add(Button::new(crate::style::rich(title, points, false, color)).frame(false))
    });
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), title));
    response.on_hover_cursor(CursorIcon::PointingHand)
}

pub fn hyperlink(
    ui: &mut Ui,
    title: &str,
    url: &str,
    size: f32,
    color: Color32,
    underline: bool,
) -> Response {
    let native = ui.ctx().plugin_opt::<Renderer>().and_then(|renderer| {
        let mut renderer = renderer.lock();
        let face = if underline {
            Face::Underlined
        } else {
            Face::Preferred(PreferredFont::Body)
        };
        let run = renderer.run(ui.ctx(), title, size, face, color)?;
        let body = PreferredFont::metrics(&PreferredFont::Body.base(), ui);
        let height = if underline {
            system_line_height(ui, f64::from(size)).max(run.size.y)
        } else {
            body.height.max(run.metrics.height)
        };
        let (rect, response) = ui.allocate_exact_size(vec2(run.size.x, height), Sense::click());
        let scale = ui.pixels_per_point();
        let shape = if underline {
            run.centered_shape(rect.left_center(), scale, color)
        } else {
            run.preferred_shape(rect.min, body.ascent, scale, color)
        };
        ui.painter().add(shape);
        Some(response)
    });
    let Some(response) = native else {
        return ui.hyperlink_to(title, url);
    };
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Link, ui.is_enabled(), title));
    let background = response.clicked_with_open_in_background();
    if background || response.clicked() {
        ui.ctx().open_url(OpenUrl {
            url: url.into(),
            new_tab: background,
        });
    }
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if ui.style().url_in_tooltip {
        response.on_hover_text(url)
    } else {
        response
    }
}

enum LineBox {
    Glyphs,
    Centered,
    Control,
    System,
    CenteredSystem,
    Stack,
    CenteredStack,
    Preferred { height: f32, ascent: f32 },
}

fn draw_label(
    ui: &mut Ui,
    text: &str,
    size: f32,
    face: Face,
    color: Color32,
    line_box: LineBox,
) -> Option<Response> {
    if text.is_empty() || text.contains('\n') {
        return None;
    }
    let handle = ui.ctx().plugin_opt::<Renderer>()?;
    let mut renderer = handle.lock();
    let mut run = renderer.run(ui.ctx(), text, size, face, color)?;
    if face == Face::RoundedDigits && run.size.x > ui.available_width() {
        let reduced = size * (ui.available_width() / run.size.x).clamp(0.7, 1.);
        run = renderer.run(ui.ctx(), text, reduced, face, color)?;
    }
    if run.size.x > ui.available_width() {
        return None;
    }
    let height = match line_box {
        LineBox::Glyphs | LineBox::Centered => run.size.y,
        LineBox::System | LineBox::CenteredSystem => {
            let scale = f64::from(ui.ctx().pixels_per_point());
            let system = (font_line_height(f64::from(size)) * scale).round() / scale;
            run.size.y.max(system as f32)
        }
        LineBox::Control => system_line_height(ui, f64::from(size)),
        LineBox::Preferred { height, .. } => height.max(run.metrics.height),
        LineBox::Stack | LineBox::CenteredStack => run
            .metrics
            .height
            .max(system_line_height(ui, f64::from(size))),
    };
    // SwiftUI rounds each Text's proposed width before positioning siblings.
    let width = if matches!(face, Face::Preferred(_)) {
        (run.size.x * ui.pixels_per_point()).ceil() / ui.pixels_per_point()
    } else {
        run.size.x
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let scale = ui.ctx().pixels_per_point();
    let shape = match line_box {
        LineBox::Preferred { ascent, .. } => run.preferred_shape(rect.min, ascent, scale, color),
        // A fallback face can leave an odd backing pixel in the system stack.
        LineBox::Centered | LineBox::CenteredSystem | LineBox::CenteredStack => {
            run.centered_shape(rect.left_center(), scale, color)
        }
        LineBox::Control => {
            // The segmented picker's label uses the system font baseline even
            // when Core Text adds leading or substitutes a shorter fallback.
            let font = NSFont::systemFontOfSize(f64::from(size));
            let ascent = (font.ascender() * f64::from(scale)).ceil() as f32 / scale;
            run.shape(
                rect.min + vec2(0., ascent - run.metrics.ascent),
                scale,
                color,
            )
        }
        _ => run.shape(
            rect.min + vec2(0., (height - run.size.y) * 0.5),
            scale,
            color,
        ),
    };
    ui.painter().add(shape);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), text));
    Some(response)
}

fn paragraph_lines(text: &str, size: f32, width: f32) -> Vec<String> {
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSFontAttributeName, NSLayoutManager, NSLineBreakStrategy, NSMutableParagraphStyle,
        NSParagraphStyleAttributeName, NSTextContainer, NSTextStorage,
    };
    use objc2_foundation::{NSRange, NSSize, NSString};

    let utf16: Vec<_> = text.encode_utf16().collect();
    let range = NSRange::new(0, utf16.len());
    let storage = NSTextStorage::new();
    storage.replaceCharactersInRange_withString(NSRange::new(0, 0), &NSString::from_str(text));
    let style = NSMutableParagraphStyle::new();
    style.setLineBreakStrategy(NSLineBreakStrategy::Standard);
    unsafe {
        storage.addAttribute_value_range(
            NSFontAttributeName,
            &NSFont::systemFontOfSize(f64::from(size)),
            range,
        );
        storage.addAttribute_value_range(NSParagraphStyleAttributeName, &style, range);
    }
    let layout = NSLayoutManager::new();
    let container = NSTextContainer::initWithSize(
        NSTextContainer::alloc(),
        NSSize::new(f64::from(width), 65536.),
    );
    container.setLineFragmentPadding(0.);
    layout.addTextContainer(&container);
    storage.addLayoutManager(&layout);
    let glyphs = layout.glyphRangeForTextContainer(&container);
    let mut next = glyphs.location;
    let mut lines = Vec::new();
    while next < glyphs.location + glyphs.length {
        let mut fragment = NSRange::new(0, 0);
        unsafe {
            layout.lineFragmentUsedRectForGlyphAtIndex_effectiveRange(next, &mut fragment);
        }
        let chars = unsafe {
            layout.characterRangeForGlyphRange_actualGlyphRange(fragment, std::ptr::null_mut())
        };
        lines.push(
            String::from_utf16_lossy(&utf16[chars.location..chars.location + chars.length])
                .trim_end_matches(['\r', '\n'])
                .to_owned(),
        );
        next = fragment.location + fragment.length;
    }
    lines
}

pub fn paragraph(
    ui: &mut Ui,
    text: &str,
    size: f32,
    spacing: f32,
    color: Color32,
) -> Option<Response> {
    use std::sync::Arc;
    let width = ui.available_width().max(1.);
    let scale = ui.ctx().pixels_per_point();
    let id = Id::new((
        "mac-text-wrap",
        text,
        size.to_bits(),
        width.to_bits(),
        scale.to_bits(),
    ));
    let cached = ui.ctx().data(|data| data.get_temp::<Arc<Vec<String>>>(id));
    let lines = cached.unwrap_or_else(|| {
        let lines = Arc::new(paragraph_lines(text, size, width));
        ui.ctx()
            .data_mut(|data| data.insert_temp(id, lines.clone()));
        lines
    });
    let handle = ui.ctx().plugin_opt::<Renderer>()?;
    let mut renderer = handle.lock();
    let mut y = 0.;
    let mut ink_width = 0.0_f32;
    let mut shapes = Vec::with_capacity(lines.len());
    let origin = ui.next_widget_position();
    for line in lines.iter() {
        let run = renderer.run(ui.ctx(), line, size, Face::Regular, color)?;
        shapes.push(run.shape(origin + vec2(0., y), scale, color));
        ink_width = ink_width.max(run.size.x);
        y += run.metrics.height + spacing;
    }
    let (_, response) = ui.allocate_exact_size(
        vec2(ink_width.ceil().min(width), (y - spacing).max(0.)),
        Sense::hover(),
    );
    ui.painter().extend(shapes);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), text));
    Some(response)
}

impl egui::Plugin for Renderer {
    fn debug_name(&self) -> &'static str {
        "Core Text"
    }
    fn on_end_pass(&mut self, ctx: &Context) {
        self.pass += 1;
        self.runs.retain(|_, run| self.pass - run.last_pass <= 4);
        let mut layers = ctx.memory(|m| m.layer_ids().collect::<Vec<_>>());
        if !layers.contains(&LayerId::background()) {
            layers.push(LayerId::background());
        }
        for layer in layers {
            let texts = ctx.graphics(|g| {
                g.get(layer)
                    .map(|list| {
                        list.all_entries()
                            .enumerate()
                            .filter_map(|(i, e)| {
                                let Shape::Text(text) = &e.shape else {
                                    return None;
                                };
                                e.clip_rect
                                    .intersects(e.shape.visual_bounding_rect())
                                    .then(|| (i, text.clone()))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            });
            for (index, text) in texts {
                if let Some(shape) = self.text_shape(ctx, &text) {
                    ctx.graphics_mut(|g| {
                        g.entry(layer)
                            .mutate_shape(layers::ShapeIdx(index), |entry| entry.shape = shape)
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn middle_truncation_preserves_both_ends_inside_the_proposed_width() {
        for scale in [1.0_f32, 2.] {
            let image = |start: &str, end: &str, width: Truncation| {
                let text = format!(
                    "{start} 모델 {} {} {end}",
                    "Extended ".repeat(8),
                    "Context ".repeat(8)
                );
                let key = Key {
                    text,
                    size: 10.5_f32.to_bits(),
                    scale: scale.to_bits(),
                    face: Face::Medium,
                    tracking: 0,
                    truncation: width,
                    light_ink: false,
                    ink_alpha: 216,
                };
                raster(&key, 4096).unwrap().0
            };
            let limit = Truncation::Middle(155_f32.to_bits());
            let sample = image("Atlas", "North", limit);
            assert!(sample.size[0] <= (155. * scale) as usize);
            assert!(sample.pixels.iter().any(|pixel| pixel.a() > 128));
            assert_ne!(sample.pixels, image("Orion", "North", limit).pixels);
            assert_ne!(sample.pixels, image("Atlas", "South", limit).pixels);
            assert_eq!(
                image("Atlas", "North", Truncation::End(155_f32.to_bits())).pixels,
                image("Atlas", "South", Truncation::End(155_f32.to_bits())).pixels
            );
        }
    }

    #[test]
    fn segmented_picker_labels_share_the_system_font_baseline() {
        for scale in [1.0_f32, 2.] {
            let ctx = Context::default();
            ctx.add_plugin(Renderer::default());
            for title in ["Provider", "연결 방법", "한글"] {
                let mut input = RawInput::default();
                input
                    .viewports
                    .get_mut(&ViewportId::ROOT)
                    .unwrap()
                    .native_pixels_per_point = Some(scale);
                let mut expected = None;
                let output = ctx.run(input, |ctx| {
                    CentralPanel::default().show(ctx, |ui| {
                        ui.allocate_ui_with_layout(
                            vec2(100., 22.),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                let response = picker_label(ui, title, Color32::BLACK).unwrap();
                                assert_eq!(response.rect.height(), system_line_height(ui, 13.));
                                let handle = ctx.plugin_opt::<Renderer>().unwrap();
                                let mut renderer = handle.lock();
                                let run = renderer
                                    .run(ctx, title, 13., Face::Regular, Color32::BLACK)
                                    .unwrap();
                                let ascent = (NSFont::systemFontOfSize(13.).ascender()
                                    * f64::from(scale))
                                .ceil() as f32;
                                let top = (response.rect.top() * scale + ascent
                                    - run.metrics.ascent * scale)
                                    .round();
                                expected = Some((run.texture.id(), top));
                            },
                        );
                    });
                });
                let (texture, top) = expected.unwrap();
                let backing_top = output.shapes.iter().find_map(|shape| match &shape.shape {
                    Shape::Mesh(mesh) if mesh.texture_id == texture => {
                        Some(mesh.calc_bounds().top() * scale)
                    }
                    _ => None,
                });
                assert_eq!(backing_top, Some(top), "{title} at {scale}x");
            }
        }
    }

    #[test]
    fn link_underline_preserves_the_native_glyph_advances() {
        let mut key = Key {
            text: "Google 지원 범위 확인".into(),
            size: 12.0_f32.to_bits(),
            scale: 1.0_f32.to_bits(),
            face: Face::Regular,
            tracking: 0,
            truncation: Truncation::None,
            light_ink: false,
            ink_alpha: 255,
        };
        for scale in [1.0_f32, 2.] {
            for light_ink in [false, true] {
                key.scale = scale.to_bits();
                key.light_ink = light_ink;
                key.face = Face::Regular;
                let (plain, size, _) = raster(&key, 4096).unwrap();
                key.face = Face::Underlined;
                let (link, underlined_size, _) = raster(&key, 4096).unwrap();
                assert_eq!(size, underlined_size);
                // The underline crosses the spaces as well as the fallback glyphs.
                assert!(
                    link.pixels
                        .chunks_exact(link.size[0])
                        .zip(plain.pixels.chunks_exact(plain.size[0]))
                        .any(|(underlined, plain)| {
                            underlined
                                .iter()
                                .zip(plain)
                                .filter(|(a, b)| a.a() > b.a())
                                .count()
                                > link.size[0] / 2
                        }),
                    "underline at {scale}x, light ink {light_ink}"
                );
            }
        }
    }

    #[test]
    fn korean_labels_have_distinct_ink_at_both_backing_scales() {
        for scale in [1.0_f32, 2.] {
            let sample = |text: &str, heavy, light_ink| {
                let key = Key {
                    text: text.to_owned(),
                    size: 15.0_f32.to_bits(),
                    scale: scale.to_bits(),
                    face: Face::label(heavy),
                    tracking: 0,
                    truncation: Truncation::None,
                    light_ink,
                    ink_alpha: 255,
                };
                let (image, bounds, _) = raster(&key, 4096).unwrap();
                assert!(bounds.x > 0. && bounds.y > 0.);
                assert!(image.pixels.iter().any(|pixel| pixel.a() > 128));
                assert!(image.pixels.iter().any(|pixel| pixel.a() == 0));
                image
            };
            for ink in [false, true] {
                assert_ne!(sample("한", false, ink), sample("글", false, ink));
                assert_ne!(
                    sample("계정 연결", true, ink),
                    sample("계정 연결", false, ink)
                );
            }
        }
    }
}
