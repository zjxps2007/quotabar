use eframe::egui::{epaint::RectShape, *};
use std::collections::HashMap;

/// WPF paints a Border's pen and inner background in two draws. Their
/// independently antialiased edges are visible even when both brushes are black.
#[derive(Default)]
pub struct BorderRenderer {
    tiles: HashMap<Tile, (TextureHandle, u64)>,
    pass: u64,
}

// Stretch just the uniform center texel of each axis, retaining the edge
// coverage and corners at device resolution at any window size or DPI.
struct Axis {
    origin: f32,
    length: usize,
    edge: usize,
    tile: usize,
}

impl Axis {
    fn new(start: f32, end: f32, corner: usize) -> Self {
        let origin = start.floor();
        let length = (end.ceil() - origin) as usize;
        let tile = length.min(corner * 2 + 1);
        Self {
            origin,
            length,
            edge: tile / 2,
            tile,
        }
    }

    fn vertices(&self) -> [(f32, f32); 4] {
        let left = self.edge as f32;
        let tile = self.tile as f32;
        let length = self.length as f32;
        [
            (self.origin, 0.),
            (self.origin + left, left / tile),
            (
                self.origin + length - (tile - left - 1.),
                (left + 1.) / tile,
            ),
            (self.origin + length, 1.),
        ]
    }
}

#[derive(Hash, PartialEq, Eq)]
struct Tile {
    size: [usize; 2],
    inset: [u32; 4],
    radius: u32,
    stroke: u32,
    pen: Color32,
    fill: Color32,
}

impl Tile {
    // Count the covered samples on the Windows software rasterizer's 8×8 grid.
    fn coverage(rect: Rect, radius: f32, x: f32, y: f32) -> u32 {
        let radius = radius.max(0.).min(rect.width().min(rect.height()) * 0.5);
        if (x >= rect.left() + radius
            && x + 1. <= rect.right() - radius
            && y >= rect.top()
            && y + 1. <= rect.bottom())
            || (y >= rect.top() + radius
                && y + 1. <= rect.bottom() - radius
                && x >= rect.left()
                && x + 1. <= rect.right())
        {
            return 64;
        }
        let sample_index = |edge: f32| ((edge - x) * 8. - 0.5).ceil().clamp(0., 8.) as u32;
        let mut covered = 0;
        for row in 0..8 {
            let sample = y + (row as f32 + 0.5) / 8.;
            if sample < rect.top() || sample >= rect.bottom() {
                continue;
            }
            let dy = (rect.top() + radius - sample)
                .max(sample - rect.bottom() + radius)
                .max(0.);
            let inset = if dy == 0. {
                0.
            } else {
                radius - (radius * radius - dy * dy).max(0.).sqrt()
            };
            covered += sample_index(rect.right() - inset)
                .saturating_sub(sample_index(rect.left() + inset));
        }
        covered
    }

    fn image(&self) -> ColorImage {
        let [left, top, right, bottom] = self.inset.map(f32::from_bits);
        let [width, height] = self.size;
        let outer = Rect::from_min_max(
            pos2(left, top),
            pos2(width as f32 - right, height as f32 - bottom),
        );
        let stroke = f32::from_bits(self.stroke);
        let inner = outer.shrink(stroke);
        let radius = f32::from_bits(self.radius);
        let mut pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let outside = Self::coverage(outer, radius + 0.5 * stroke, x as f32, y as f32);
                let fill = Self::coverage(inner, radius - 0.5 * stroke, x as f32, y as f32);
                let pen = outside.saturating_sub(fill) * (64 - fill);
                let background = fill * 64;
                let mix = |a: u8, b: u8| {
                    ((u32::from(a) * pen + u32::from(b) * background + 2048) / 4096) as u8
                };
                pixels.push(Color32::from_rgba_premultiplied(
                    mix(self.pen.r(), self.fill.r()),
                    mix(self.pen.g(), self.fill.g()),
                    mix(self.pen.b(), self.fill.b()),
                    mix(self.pen.a(), self.fill.a()),
                ));
            }
        }
        ColorImage::new(self.size, pixels)
    }
}

impl BorderRenderer {
    fn paint(&mut self, ctx: &Context, shape: &RectShape) -> Option<Shape> {
        let radius = shape.corner_radius;
        let stroke = shape.stroke;
        if shape.stroke_kind != StrokeKind::Inside
            || radius.nw == 0
            || !radius.is_same()
            || stroke.width != 1.
            || stroke.color.a() != 255
            || shape.fill.a() != 255
            || shape.blur_width != 0.
            || shape.brush.is_some()
            || !shape.rect.is_finite()
        {
            return None;
        }
        self.rounded(ctx, shape.rect, f32::from(radius.nw), stroke, shape.fill)
    }

    fn rounded(
        &mut self,
        ctx: &Context,
        rect: Rect,
        radius: f32,
        stroke: Stroke,
        fill: Color32,
    ) -> Option<Shape> {
        let scale = ctx.pixels_per_point();
        let r = radius * scale;
        let s = stroke.width * scale;
        let rect = rect * scale;
        if rect.width().min(rect.height()) <= s * 2. {
            return None;
        }
        let corner = (r + s * 0.5).ceil() as usize + 1;
        let x = Axis::new(rect.left(), rect.right(), corner);
        let y = Axis::new(rect.top(), rect.bottom(), corner);
        let tile = Tile {
            size: [x.tile, y.tile],
            inset: [
                rect.left() - x.origin,
                rect.top() - y.origin,
                rect.right().ceil() - rect.right(),
                rect.bottom().ceil() - rect.bottom(),
            ]
            .map(f32::to_bits),
            radius: r.to_bits(),
            stroke: s.to_bits(),
            pen: stroke.color,
            fill,
        };
        let texture = match self.tiles.entry(tile) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let image = entry.key().image();
                entry.insert((
                    ctx.load_texture("Control border", image, TextureOptions::NEAREST),
                    self.pass,
                ))
            }
        };
        texture.1 = self.pass;
        let mut mesh = Mesh::with_texture(texture.0.id());
        for (dy, v) in y.vertices() {
            for (dx, u) in x.vertices() {
                mesh.vertices.push(epaint::Vertex {
                    pos: pos2(dx / scale, dy / scale),
                    uv: pos2(u, v),
                    color: Color32::WHITE,
                });
            }
        }
        for row in 0..3 {
            for column in 0..3 {
                let i = row * 4 + column;
                mesh.indices.extend([i, i + 1, i + 4, i + 1, i + 5, i + 4]);
            }
        }
        Some(Shape::mesh(mesh))
    }
}

pub fn rounded_rectangle(ui: &Ui, rect: Rect, radius: f32, fill: Color32, pen: Stroke) {
    if !rect.is_positive() || !ui.is_rect_visible(rect) {
        return;
    }
    let shape = ui
        .ctx()
        .plugin_opt::<BorderRenderer>()
        .and_then(|renderer| renderer.lock().rounded(ui.ctx(), rect, radius, pen, fill));
    if let Some(shape) = shape {
        ui.painter().add(shape);
    } else {
        ui.painter()
            .rect(rect, radius, fill, pen, StrokeKind::Inside);
    }
}

pub fn activity_cell(ui: &Ui, rect: Rect, track: Color32, fill: Option<Color32>, border: Stroke) {
    rounded_rectangle(ui, rect, 2., track, border);
    let child = rect.shrink(border.width);
    if let Some(fill) = fill.filter(|_| child.is_positive()) {
        rounded_rectangle(ui, child, 2., fill, Stroke::NONE);
    }
}

impl Plugin for BorderRenderer {
    fn debug_name(&self) -> &'static str {
        "Windows control borders"
    }

    fn on_end_pass(&mut self, ctx: &Context) {
        self.pass += 1;
        self.tiles.retain(|_, (_, pass)| self.pass - *pass <= 3);
        let mut layers = ctx.memory(|m| m.layer_ids().collect::<Vec<_>>());
        if !layers.contains(&LayerId::background()) {
            layers.push(LayerId::background());
        }
        for layer in layers {
            let rects = ctx.graphics(|g| {
                g.get(layer)
                    .into_iter()
                    .flat_map(|list| list.all_entries().enumerate())
                    .filter_map(|(i, entry)| match &entry.shape {
                        Shape::Rect(rect) if entry.clip_rect.intersects(rect.rect) => {
                            Some((i, rect.clone()))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            });
            for (index, rect) in rects {
                if let Some(shape) = self.paint(ctx, &rect) {
                    ctx.graphics_mut(|g| {
                        g.entry(layer)
                            .mutate_shape(layers::ShapeIdx(index), |entry| entry.shape = shape)
                    });
                }
            }
        }
    }
}
