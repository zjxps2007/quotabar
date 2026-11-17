use eframe::egui::{epaint::RectShape, *};
use std::collections::HashMap;

/// WPF paints a Border's pen and inner background in two draws. Their
/// independently antialiased edges are visible even when both brushes are black.
#[derive(Default)]
pub struct BorderRenderer {
    tiles: HashMap<Tile, (TextureHandle, u64)>,
    pass: u64,
    pub window_target: bool,
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
    window_target: bool,
}

#[derive(Clone, Copy)]
struct EdgeRamp {
    center: f32,
    slope: f32,
}

impl EdgeRamp {
    fn width(self) -> f32 {
        1. + self.slope.abs()
    }

    fn separated_from(self, next: Self) -> bool {
        // WPF tests both ends of the row. Its fixed-point rasterizer bounds
        // the expansion of each sloped edge in whole eighths of a pixel.
        let expansion = |slope: f32| {
            let delta = 4. * slope.abs();
            if slope.fract() == 0. {
                delta
            } else {
                delta.floor() + 1.
            }
        };
        let distance = next.center - self.center - (next.slope - self.slope).abs() * 0.5;
        distance * 8. >= 8. + expansion(self.slope) + expansion(next.slope)
    }
}

struct Coverage {
    // First included and first excluded sample on each 8×8 subscanline.
    samples: Vec<[i32; 2]>,
    // WPF can interpolate coverage when a pair of edges spans an entire device row.
    ramps: Vec<[Option<EdgeRamp>; 2]>,
}

impl Coverage {
    fn new(rect: Rect, radius: f32, height: usize, window_target: bool) -> Self {
        let fixed = |p: Pos2| (p * 16. + Vec2::splat(0.5)).floor();
        let first = |x: f32| (x * 0.5).ceil() as i32;
        let min = fixed(rect.min);
        let max = fixed(rect.max);
        let mut mask = Self {
            samples: vec![[0, 0]; height * 8],
            ramps: if window_target {
                vec![[None; 2]; height]
            } else {
                Vec::new()
            },
        };
        let start = first(min.y).clamp(0, mask.samples.len() as i32) as usize;
        let end = first(max.y).clamp(start as i32, mask.samples.len() as i32) as usize;
        mask.samples[start..end].fill([first(min.x), first(max.x)]);
        let radius = radius.max(0.).min(rect.width().min(rect.height()) * 0.5);
        for (side, x) in [rect.left(), rect.right()].into_iter().enumerate() {
            mask.set_ramp(
                side,
                fixed(pos2(x, rect.top() + radius)),
                fixed(pos2(x, rect.bottom() - radius)),
            );
        }
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
                        mask.edge(side, previous, to);
                        previous = to;
                    });
                }
            }
        }
        mask
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

    fn edge(&mut self, side: usize, a: Pos2, b: Pos2) {
        let (lo, hi) = if a.y <= b.y { (a, b) } else { (b, a) };
        self.set_ramp(side, lo, hi);
        let rows = &mut self.samples;
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

    fn set_ramp(&mut self, side: usize, lo: Pos2, hi: Pos2) {
        if self.ramps.is_empty() || hi.y <= lo.y {
            return;
        }
        // Edge endpoints are in sixteenths of a pixel. The active edge changes
        // on the first eighth-pixel sample at or after the endpoint.
        let first = ((lo.y * 0.5).ceil() / 8.).ceil().max(0.) as usize;
        let end = ((hi.y * 0.5).ceil() / 8.).floor().max(0.) as usize;
        let slope = (hi.x - lo.x) / (hi.y - lo.y);
        for y in first..end.min(self.ramps.len()) {
            let center_y = (y as f32 + 0.5) * 16.;
            self.ramps[y][side] = Some(EdgeRamp {
                center: (lo.x + (center_y - lo.y) * slope) / 16.,
                slope,
            });
        }
    }

    fn window_fraction(&self, x: usize, y: usize) -> f32 {
        if let [Some(left), Some(right)] = self.ramps[y]
            && left.separated_from(right)
        {
            let center = x as f32 + 0.5;
            let from_left = 0.5 + (center - left.center) / left.width();
            let from_right = 0.5 + (right.center - center) / right.width();
            return from_left.min(from_right).clamp(0., 1.);
        }
        self.pixel(x, y) as f32 / 64.
    }

    fn can_interpolate_stroke(&self, inner: &Self, y: usize) -> bool {
        let [Some(left), Some(right)] = self.ramps[y] else {
            return false;
        };
        match inner.ramps[y] {
            [Some(a), Some(b)] => {
                left.separated_from(a) && a.separated_from(b) && b.separated_from(right)
            }
            [None, None] => {
                left.separated_from(right)
                    && inner.samples[y * 8..y * 8 + 8].iter().all(|&[a, b]| a == b)
            }
            _ => false,
        }
    }

    fn pixel(&self, x: usize, y: usize) -> u32 {
        let start = (x * 8) as i32;
        self.samples[y * 8..y * 8 + 8]
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
        let pen_coverage = Coverage::new(outer, radius + 0.5 * stroke, height, self.window_target);
        let fill_coverage = (stroke > 0.)
            .then(|| Coverage::new(inner, radius - 0.5 * stroke, height, self.window_target));
        let mut pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            let interpolate_stroke = self.window_target
                && fill_coverage
                    .as_ref()
                    .is_some_and(|inner| pen_coverage.can_interpolate_stroke(inner, y));
            for x in 0..width {
                if self.window_target {
                    let Some(inner) = &fill_coverage else {
                        pixels.push(self.fill.gamma_multiply(pen_coverage.window_fraction(x, y)));
                        continue;
                    };
                    let fill = inner.window_fraction(x, y);
                    // The pen is a hollow path. Overlapping edge ramps make
                    // the whole pen row use samples, including its straight parts.
                    let stroke = if interpolate_stroke {
                        (pen_coverage.window_fraction(x, y) - fill).max(0.)
                    } else {
                        pen_coverage.pixel(x, y).saturating_sub(inner.pixel(x, y)) as f32 / 64.
                    };
                    let pen = stroke * (1. - fill * f32::from(self.fill.a()) / 255.);
                    let mix =
                        |a: u8, b: u8| (f32::from(a) * pen + f32::from(b) * fill).round() as u8;
                    pixels.push(Color32::from_rgba_premultiplied(
                        mix(self.pen.r(), self.fill.r()),
                        mix(self.pen.g(), self.fill.g()),
                        mix(self.pen.b(), self.fill.b()),
                        mix(self.pen.a(), self.fill.a()),
                    ));
                    continue;
                }
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
    fn window_and_bitmap_preserve_their_distinct_wpf_edge_coverage() {
        // White calendar cells over black, captured on both WPF drawing targets.
        let size = vec2(17. + 19. / 26., 16.);
        for (origin, x, y, window, bitmap) in [
            (pos2(1. / 32., 19. / 32.), 0, 3, 239, 223),
            (pos2(1. / 32., 19. / 32.), 0, 1, 92, 92),
            (Pos2::ZERO, 0, 14, 183, 199),
            (Pos2::ZERO, 17, 14, 133, 155),
        ] {
            let mask = Coverage::new(Rect::from_min_size(origin, size), 2., 17, true);
            assert_eq!((255. * mask.window_fraction(x, y)).round() as u32, window);
            assert_eq!((255 * mask.pixel(x, y) + 32) / 64, bitmap);
        }
    }

    #[test]
    fn outlined_window_controls_interpolate_only_separated_edges() {
        // WPF uses trapezoids along the straight sides of a one-pixel pen and
        // falls back to samples where the inner and outer corner ramps overlap.
        for (origin, size, radius, x, y, expected) in [
            (
                pos2(2. / 64., 38. / 64.),
                vec2(17. + 19. / 26., 16.),
                2.0_f32,
                0,
                5,
                239_u8,
            ),
            (
                pos2(1. / 64., 19. / 64.),
                vec2(60.6875, 40.25),
                16.,
                7,
                3,
                216,
            ),
            (
                pos2(1. / 64., 19. / 64.),
                vec2(60.6875, 40.25),
                16.,
                4,
                6,
                215,
            ),
        ] {
            let end = origin + size;
            let tile = Tile {
                size: [end.x.ceil() as usize, end.y.ceil() as usize],
                inset: [
                    origin.x,
                    origin.y,
                    end.x.ceil() - end.x,
                    end.y.ceil() - end.y,
                ]
                .map(f32::to_bits),
                radius: radius.to_bits(),
                stroke: 1.0_f32.to_bits(),
                pen: Color32::WHITE,
                fill: Color32::from_gray(128),
                window_target: true,
            };
            assert!(tile.image()[(x, y)].r().abs_diff(expected) <= 1);
        }
    }

    #[test]
    fn fractional_switch_focus_border_covers_both_adjacent_rows() {
        let bounds = Rect::from_min_size(pos2(0., 1. / 16.), vec2(34., 22.));
        // In WPF's switch focus visual the straight edge covers 7/8 of one
        // physical pixel and 1/8 of the next on both sides of the track.
        let outer = Coverage::new(bounds, 10.5, 23, false);
        let inner = Coverage::new(bounds.shrink(1.), 9.5, 23, false);
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
                    let mask = Coverage::new(relative, 7., 25, false);
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
            let coverage = Coverage::new(rect, radius, 50, false);
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
            window_target: self.window_target,
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
