use core_foundation::{
    attributed_string::CFMutableAttributedString,
    base::{CFRange, TCFType},
    boolean::CFBoolean,
    string::CFString,
};
use core_graphics::{base::kCGImageAlphaNone, color_space::CGColorSpace, context::CGContext};
use core_text::{
    font::{CTFont, CTFontRef},
    line::CTLine,
    string_attributes::{kCTFontAttributeName, kCTForegroundColorFromContextAttributeName},
};
use eframe::egui::{self, epaint::TextShape, *};
use objc2::rc::Retained;
use objc2_app_kit::{NSFont, NSFontWeightBold, NSFontWeightRegular, NSFontWeightSemibold};
use std::collections::HashMap;

#[derive(Hash, Eq, PartialEq)]
struct Key {
    text: String,
    size: u32,
    scale: u32,
    heavy: bool,
}

fn raster(key: &Key, limit: usize) -> Option<(ColorImage, Vec2)> {
    let size = f64::from(f32::from_bits(key.size));
    let scale = f64::from(f32::from_bits(key.scale));
    // NSFont and CTFont are toll-free bridged. Use the AppKit font at its
    // logical point size so Retina drawing preserves its optical size.
    let weight = unsafe {
        if !key.heavy {
            NSFontWeightRegular
        } else if size >= 17. {
            NSFontWeightBold
        } else {
            NSFontWeightSemibold
        }
    };
    let nsfont = NSFont::systemFontOfSize_weight(size, weight);
    let font = unsafe { CTFont::wrap_under_get_rule(Retained::as_ptr(&nsfont) as CTFontRef) };
    let mut text = CFMutableAttributedString::new();
    text.replace_str(&CFString::new(&key.text), CFRange::init(0, 0));
    let range = CFRange::init(0, text.char_len());
    text.set_attribute(range, unsafe { kCTFontAttributeName }, &font);
    text.set_attribute(
        range,
        unsafe { kCTForegroundColorFromContextAttributeName },
        &CFBoolean::true_value(),
    );
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
        width,
        &CGColorSpace::create_device_gray(),
        kCGImageAlphaNone,
    );
    canvas.scale(scale, scale);
    canvas.set_gray_fill_color(1., 1.);
    canvas.set_should_antialias(true);
    canvas.set_text_position(0., (height as f64 - ascent) / scale);
    line.draw(&canvas);
    let pixels = canvas
        .data()
        .iter()
        .map(|&alpha| Color32::from_white_alpha(alpha))
        .collect();
    Some((
        ColorImage::new([width, height], pixels),
        vec2(bounds.width as f32, (height as f64 / scale) as f32),
    ))
}

struct Run {
    texture: TextureHandle,
    size: Vec2,
    last_pass: u64,
}

#[derive(Default)]
pub struct Renderer {
    runs: HashMap<Key, Run>,
    pass: u64,
}

impl Renderer {
    fn run(&mut self, ctx: &Context, text: &str, size: f32, heavy: bool) -> Option<&Run> {
        let key = Key {
            text: text.to_owned(),
            size: size.to_bits(),
            scale: ctx.pixels_per_point().to_bits(),
            heavy,
        };
        let cached = match self.runs.entry(key) {
            std::collections::hash_map::Entry::Occupied(slot) => slot.into_mut(),
            std::collections::hash_map::Entry::Vacant(slot) => {
                let (image, size) = raster(slot.key(), ctx.input(|i| i.max_texture_side))?;
                slot.insert(Run {
                    texture: ctx.load_texture("Core Text", image, TextureOptions::NEAREST),
                    size,
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
        let heavy = match &format.font_id.family {
            FontFamily::Proportional => false,
            FontFamily::Name(name) if name.as_ref() == "system-semibold" => true,
            _ => return None,
        };
        if text.angle != 0.
            || format.italics
            || !text.underline.is_empty()
            || format.extra_letter_spacing != 0.
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
            let run = self.run(ctx, &string, format.font_id.size, heavy)?;
            let origin = text.pos + row.pos.to_vec2() + vec2(0., (row.size.y - run.size.y) * 0.5);
            shapes.push(run.shape(origin, ctx.pixels_per_point(), color));
        }
        Some(Shape::Vec(shapes))
    }
}

impl Run {
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
    Some(handle.lock().run(ui.ctx(), text, size, heavy)?.size)
}

pub fn label(ui: &mut Ui, text: &str, size: f32, heavy: bool, color: Color32) -> Option<Response> {
    if text.is_empty() || text.contains('\n') {
        return None;
    }
    let handle = ui.ctx().plugin_opt::<Renderer>()?;
    let mut renderer = handle.lock();
    let run = renderer.run(ui.ctx(), text, size, heavy)?;
    if run.size.x > ui.available_width() {
        return None;
    }
    let (rect, response) = ui.allocate_exact_size(run.size, Sense::hover());
    ui.painter()
        .add(run.shape(rect.min, ui.ctx().pixels_per_point(), color));
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
    fn korean_labels_have_distinct_ink_at_both_backing_scales() {
        for scale in [1.0_f32, 2.] {
            let sample = |text: &str, heavy| {
                let key = Key {
                    text: text.to_owned(),
                    size: 15.0_f32.to_bits(),
                    scale: scale.to_bits(),
                    heavy,
                };
                let (image, bounds) = raster(&key, 4096).unwrap();
                assert!(bounds.x > 0. && bounds.y > 0.);
                assert!(image.pixels.iter().any(|pixel| pixel.a() > 128));
                image
            };
            assert_ne!(sample("한", false), sample("글", false));
            assert_ne!(sample("계정 연결", true), sample("계정 연결", false));
        }
    }
}
