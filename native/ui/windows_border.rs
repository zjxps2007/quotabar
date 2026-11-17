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

// Each scan line contains the first included and first excluded sample on
// the WPF software rasterizer's 8×8 grid.
struct Coverage(Vec<[i32; 2]>);

impl Coverage {
    fn new(rect: Rect, radius: f32, height: usize) -> Self {
        let fixed = |p: Pos2| (p * 16. + Vec2::splat(0.5)).floor();
        let first = |x: f32| (x * 0.5).ceil() as i32;
        let min = fixed(rect.min);
        let max = fixed(rect.max);
        let mut rows = vec![[0, 0]; height * 8];
        let start = first(min.y).clamp(0, rows.len() as i32) as usize;
        let end = first(max.y).clamp(start as i32, rows.len() as i32) as usize;
        rows[start..end].fill([first(min.x), first(max.x)]);
        let radius = radius.max(0.).min(rect.width().min(rect.height()) * 0.5);
        if radius > 0. {
            // Border supplies cubic quarter arcs. Both their control points
            // and the vertices emitted by the flattener use 28.4 coordinates.
            let control = radius * (1. - 0.552_284_8);
            for (x, dx, side) in [(rect.left(), 1., 0), (rect.right(), -1., 1)] {
                for (y, dy) in [(rect.top(), 1.), (rect.bottom(), -1.)] {
                    let points = [(radius, 0.), (control, 0.), (0., control), (0., radius)]
                        .map(|(u, v)| fixed(pos2(x + dx * u, y + dy * v)));
                    let mut previous = points[0];
                    Self::flatten(points, &mut |to| {
                        Self::edge(&mut rows, side, previous, to);
                        previous = to;
                    });
                }
            }
        }
        Self(rows)
    }

    fn flatten([a, b, c, d]: [Pos2; 4], emit: &mut impl FnMut(Pos2)) {
        // At one quarter of a device pixel (four fixed units), the chord
        // represents the curve at WPF's flattening tolerance.
        let bend = ((b - a) - (c - b)).abs().max(((c - b) - (d - c)).abs());
        if bend.x.max(bend.y) <= 4. {
            emit((d + Vec2::splat(0.5)).floor());
            return;
        }
        let ab = a.lerp(b, 0.5);
        let bc = b.lerp(c, 0.5);
        let cd = c.lerp(d, 0.5);
        let left = ab.lerp(bc, 0.5);
        let right = bc.lerp(cd, 0.5);
        let middle = left.lerp(right, 0.5);
        Self::flatten([a, ab, left, middle], emit);
        Self::flatten([middle, right, cd, d], emit);
    }

    fn edge(rows: &mut [[i32; 2]], side: usize, a: Pos2, b: Pos2) {
        let (lo, hi) = if a.y <= b.y { (a, b) } else { (b, a) };
        let first = (lo.y * 0.5).ceil().clamp(0., rows.len() as f32) as usize;
        let end = (hi.y * 0.5).ceil().clamp(0., rows.len() as f32) as usize;
        let dx = (hi.x - lo.x) as i64;
        let dy = (hi.y - lo.y) as i64;
        for (i, row) in rows[first..end].iter_mut().enumerate() {
            let y = 2 * (first + i) as i64;
            let x = lo.x as i64 * dy + (y - lo.y as i64) * dx;
            row[side] = (x + 2 * dy - 1).div_euclid(2 * dy) as i32;
        }
    }

    fn pixel(&self, x: usize, y: usize) -> u32 {
        let start = (x * 8) as i32;
        self.0[y * 8..y * 8 + 8]
            .iter()
            .map(|&[left, right]| (right.min(start + 8) - left.max(start)).max(0) as u32)
            .sum()
    }
}

impl Tile {
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
        let pen_coverage = Coverage::new(outer, radius + 0.5 * stroke, height);
        let fill_coverage =
            (stroke > 0.).then(|| Coverage::new(inner, radius - 0.5 * stroke, height));
        let mut pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let outside = pen_coverage.pixel(x, y);
                let fill = fill_coverage
                    .as_ref()
                    .map_or(outside, |mask| mask.pixel(x, y));
                let fill_opacity = (fill * u32::from(self.fill.a()) + 127) / 255;
                let pen = outside.saturating_sub(fill) * (64 - fill_opacity);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_switch_focus_border_covers_both_adjacent_rows() {
        let bounds = Rect::from_min_size(pos2(0., 1. / 16.), vec2(34., 22.));
        // In WPF's switch focus visual the straight edge covers 7/8 of one
        // physical pixel and 1/8 of the next on both sides of the track.
        let outer = Coverage::new(bounds, 10.5, 23);
        let inner = Coverage::new(bounds.shrink(1.), 9.5, 23);
        let samples = [0, 1, 21, 22].map(|y| outer.pixel(16, y) - inner.pixel(16, y));
        assert_eq!(samples, [56, 8, 56, 8]);
    }

    #[test]
    fn straight_edges_change_at_the_same_subpixels_as_wpf() {
        // Sampled from WPF Borders placed 1/64 device pixel apart. At phases
        // 2, 10, ..., 58 one more of the eight sample rows leaves the top edge.
        for scale in [1., 2.] {
            for origin in [-8., 0., 12.] {
                for phase in 0..=64 {
                    let y = origin + phase as f32 / 64.;
                    let rect = Rect::from_min_size(pos2(0., y / scale), vec2(80., 24.) / scale);
                    let relative = (rect * scale).translate(vec2(0., -origin));
                    let mask = Coverage::new(relative, 7., 25);
                    let expected = 64 - ((phase + 6) / 8) * 8;
                    assert_eq!(mask.pixel(40, 0), expected);
                    assert_eq!(mask.pixel(40, 24), 64 - expected);
                }
            }
        }
    }
    #[test]
    fn corner_samples_match_wpf_at_fractional_origins_and_radii() {
        for (origin, radius, x, y, expected) in [
            (pos2(0., 0.), 2., 0, 0, [13, 46, 64]),
            (pos2(17. / 64., 3. / 64.), 3.5, 1, 0, [5, 38, 52]),
            (pos2(23. / 64., 53. / 64.), 7., 2, 2, [8, 48, 64]),
            (pos2(27. / 64., 1. / 64.), 16., 5, 3, [0, 21, 61]),
        ] {
            let rect = Rect::from_min_size(origin, vec2(64.6875, 48.25));
            let coverage = Coverage::new(rect, radius, 50);
            assert_eq!([x, x + 1, x + 2].map(|i| coverage.pixel(i, y)), expected);
        }
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
