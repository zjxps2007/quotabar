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
};
use core_text::{
    font::{CTFont, CTFontRef},
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
enum Face {
    Regular,
    Medium,
    Monospace,
    Underlined,
    Emphasized,
    Bold,
    TabularDigits,
    RoundedDigits,
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
    light_ink: bool,
}

fn attributed(value: &str, size: f64, face: Face, tracking: f32) -> CFMutableAttributedString {
    // NSFont and CTFont are toll-free bridged. Use the AppKit font at its
    // logical point size so Retina drawing preserves its optical size.
    let weight = unsafe {
        match face {
            Face::Regular | Face::Monospace | Face::Underlined => NSFontWeightRegular,
            Face::Medium => NSFontWeightMedium,
            Face::Bold => NSFontWeightBold,
            Face::Emphasized if size >= 17. => NSFontWeightBold,
            Face::Emphasized | Face::TabularDigits | Face::RoundedDigits => NSFontWeightSemibold,
        }
    };
    let nsfont = if face == Face::RoundedDigits {
        let digits = NSFont::monospacedDigitSystemFontOfSize_weight(size, weight);
        digits
            .fontDescriptor()
            .fontDescriptorWithDesign(unsafe { objc2_app_kit::NSFontDescriptorSystemDesignRounded })
            .and_then(|descriptor| NSFont::fontWithDescriptor_size(&descriptor, size))
            .unwrap_or(digits)
    } else if face == Face::Monospace {
        NSFont::monospacedSystemFontOfSize_weight(size, weight)
    } else if face == Face::TabularDigits {
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

struct LineMetrics {
    height: f32,
    ascent: f32,
}

fn raster(key: &Key, limit: usize) -> Option<(ColorImage, Vec2, LineMetrics)> {
    let size = f64::from(f32::from_bits(key.size));
    let scale = f64::from(f32::from_bits(key.scale));
    let text = attributed(&key.text, size, key.face, f32::from_bits(key.tracking));
    let line = CTLine::new_with_attributed_string(text.as_concrete_TypeRef());
    let bounds = line.get_typographic_bounds();
    let ascent = (bounds.ascent * scale).ceil();
    let height = (ascent + ((bounds.descent + bounds.leading) * scale).ceil()).max(1.) as usize;
    let width = (bounds.width * scale).ceil().max(1.) as usize;
    if width > limit || height > limit || width.checked_mul(height)? > 8_000_000 {
        return None;
    }
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
    let ink = if key.light_ink { 1. } else { 0. };
    let paper = 1. - ink;
    canvas.set_rgb_fill_color(paper, paper, paper, 1.);
    canvas.fill_rect(CGRect::new(
        &CGPoint::new(0., 0.),
        &CGSize::new(width as f64, height as f64),
    ));
    canvas.scale(scale, scale);
    canvas.set_rgb_fill_color(ink, ink, ink, 1.);
    canvas.set_should_antialias(true);
    canvas.set_allows_font_smoothing(true);
    canvas.set_should_smooth_fonts(true);
    canvas.set_text_position(0., (height as f64 - ascent) / scale);
    line.draw(&canvas);
    let pixels = canvas
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|rgb| {
            let shade = ((u16::from(rgb[0]) + u16::from(rgb[1]) + u16::from(rgb[2]) + 1) / 3) as u8;
            Color32::from_white_alpha(if key.light_ink { shade } else { 255 - shade })
        })
        .collect();
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
        self.tracked_run(ctx, text, size, face, color, 0.)
    }

    fn tracked_run(
        &mut self,
        ctx: &Context,
        text: &str,
        size: f32,
        face: Face,
        color: Color32,
        tracking: f32,
    ) -> Option<&Run> {
        let [r, g, b, _] = color.to_srgba_unmultiplied();
        let ink = Color32::from_rgb(r, g, b).intensity();
        let key = Key {
            text: text.to_owned(),
            size: size.to_bits(),
            scale: ctx.pixels_per_point().to_bits(),
            face,
            tracking: tracking.to_bits(),
            light_ink: ink > ctx.style().visuals.panel_fill.intensity(),
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
        Shape::image(
            self.texture.id(),
            Rect::from_min_size(pos, self.texture.size_vec2() / scale),
            Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
            color,
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
    let renderer = ui.ctx().plugin_opt::<Renderer>();
    for (index, line) in text.lines().enumerate() {
        let top = start + vec2(0., index as f32 * advance);
        if let Some(renderer) = &renderer
            && let Some(run) =
                renderer
                    .lock()
                    .run(ui.ctx(), line, 12., Face::label(selected), color)
        {
            ui.painter().add(run.shape(top, scale, color));
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

pub fn dashboard_heading(ui: &mut Ui, text: &str, color: Color32) -> Option<Response> {
    draw_label(ui, text, 25., Face::Bold, color, LineBox::CenteredSystem)
}

pub fn stat_value(ui: &mut Ui, text: &str, color: Color32) -> Option<Response> {
    draw_label(ui, text, 28., Face::RoundedDigits, color, LineBox::Stack)
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

pub fn bold_label(ui: &mut Ui, text: &str, size: f32, color: Color32) -> Option<Response> {
    draw_label(ui, text, size, Face::Bold, color, LineBox::Stack)
}

pub fn plan_label(ui: &mut Ui, text: &str, color: Color32) -> Option<Response> {
    draw_label(ui, text, 9., Face::Medium, color, LineBox::Stack)
}

pub fn preferred_label(
    ui: &mut Ui,
    text: &str,
    style: &objc2_app_kit::NSFontTextStyle,
    bold: bool,
    color: Color32,
) -> Response {
    let font = unsafe {
        NSFont::preferredFontForTextStyle_options(style, &objc2_foundation::NSDictionary::new())
    };
    let size = font.pointSize() as f32;
    let face = if bold { Face::Bold } else { Face::Regular };
    draw_label(ui, text, size, face, color, LineBox::Stack)
        .unwrap_or_else(|| ui.label(crate::style::rich(text, size, bold, color)))
}

pub fn quota_percent(ui: &mut Ui, text: &str, color: Color32) -> Option<Response> {
    draw_label(ui, text, 11., Face::TabularDigits, color, LineBox::Stack)
}

pub fn plain_button(ui: &mut Ui, title: &str, color: Color32) -> Response {
    let points = NSFont::systemFontSize() as f32;
    let native = ui.ctx().plugin_opt::<Renderer>().and_then(|renderer| {
        use eframe::emath::GuiRounding;
        let mut renderer = renderer.lock();
        let run = renderer.run(ui.ctx(), title, points, Face::Regular, color)?;
        let scale = ui.pixels_per_point();
        let size = vec2(
            (run.size.x * scale).ceil() / scale,
            system_line_height(ui, f64::from(points)),
        );
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
            Face::Regular
        };
        let run = renderer.run(ui.ctx(), title, size, face, color)?;
        let height = system_line_height(ui, f64::from(size)).max(run.size.y);
        let (rect, response) = ui.allocate_exact_size(vec2(run.size.x, height), Sense::click());
        let top = pos2(rect.left(), rect.center().y - run.size.y * 0.5);
        ui.painter()
            .add(run.shape(top, ui.ctx().pixels_per_point(), color));
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
        LineBox::Stack => run
            .metrics
            .height
            .max(system_line_height(ui, f64::from(size))),
    };
    let (rect, response) = ui.allocate_exact_size(vec2(run.size.x, height), Sense::hover());
    let scale = ui.ctx().pixels_per_point();
    let shape = match line_box {
        LineBox::Centered | LineBox::CenteredSystem => {
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
            light_ink: false,
        };
        let (plain, size, _) = raster(&key, 4096).unwrap();
        key.face = Face::Underlined;
        let (link, underlined_size, _) = raster(&key, 4096).unwrap();
        assert_eq!(size, underlined_size);
        assert!(
            plain
                .pixels
                .iter()
                .zip(link.pixels)
                .filter(|(a, b)| a.a() < b.a())
                .count()
                > 12
        );
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
                    light_ink,
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
