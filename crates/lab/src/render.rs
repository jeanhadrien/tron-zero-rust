//! PNG snapshots of a lab world: arena, walls, trails, heads and deaths.
//! World y points up (as in the client), so rows are flipped.

use glam::Vec2;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::sim::Lab;

/// Rider colors in scenario order, with names for the text legend.
pub const PALETTE: [(&str, [u8; 3]); 8] = [
    ("red", [255, 68, 68]),
    ("cyan", [68, 255, 255]),
    ("yellow", [255, 204, 0]),
    ("green", [68, 255, 68]),
    ("magenta", [255, 68, 255]),
    ("orange", [255, 136, 0]),
    ("blue", [68, 136, 255]),
    ("violet", [136, 68, 255]),
];

/// Square world window to draw: `center` and side length in world units.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub center: Vec2,
    pub size: f32,
}

impl View {
    /// Whole arena plus a small margin.
    pub fn arena(lab: &Lab) -> Self {
        let size = lab.size();
        Self {
            center: Vec2::ZERO,
            size: size.width.max(size.height) * 1.04,
        }
    }
}

struct Canvas {
    pixmap: Pixmap,
    view: View,
}

impl Canvas {
    fn scale(&self) -> f32 {
        self.pixmap.width() as f32 / self.view.size
    }

    fn to_px(&self, p: Vec2) -> (f32, f32) {
        let origin = self.view.center - Vec2::splat(self.view.size * 0.5);
        let s = self.scale();
        (
            (p.x - origin.x) * s,
            self.pixmap.height() as f32 - (p.y - origin.y) * s,
        )
    }

    fn polyline(&mut self, points: &[Vec2], rgb: [u8; 3], alpha: u8, width: f32) {
        let mut path = PathBuilder::new();
        for (i, p) in points.iter().enumerate() {
            let (x, y) = self.to_px(*p);
            if i == 0 {
                path.move_to(x, y);
            } else {
                path.line_to(x, y);
            }
        }
        let Some(path) = path.finish() else { return };
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        let paint = paint(rgb, alpha);
        self.pixmap
            .stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }

    fn circle(&mut self, center: Vec2, radius_px: f32, rgb: [u8; 3], alpha: u8, fill: bool) {
        let (x, y) = self.to_px(center);
        let Some(path) = PathBuilder::from_circle(x, y, radius_px) else {
            return;
        };
        let paint = paint(rgb, alpha);
        if fill {
            self.pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        } else {
            let stroke = Stroke {
                width: 1.0,
                ..Stroke::default()
            };
            self.pixmap
                .stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }
}

fn paint(rgb: [u8; 3], alpha: u8) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(rgb[0], rgb[1], rgb[2], alpha));
    paint.anti_alias = true;
    paint
}

/// Render the current lab state into a `pixels`-wide square image.
pub fn render(lab: &Lab, view: View, pixels: u32) -> Pixmap {
    let mut canvas = Canvas {
        pixmap: Pixmap::new(pixels, pixels).expect("non-zero image size"),
        view,
    };
    canvas.pixmap.fill(Color::from_rgba8(16, 20, 24, 255));

    // Grid: 100-unit cells when they are at least 8 px apart, else 500.
    let step = if 100.0 * canvas.scale() >= 8.0 {
        100.0
    } else {
        500.0
    };
    let half = view.size * 0.5;
    let (lo, hi) = (view.center - half, view.center + half);
    let mut x = (lo.x / step).ceil() * step;
    while x <= hi.x {
        canvas.polyline(
            &[Vec2::new(x, lo.y), Vec2::new(x, hi.y)],
            [40, 50, 60],
            255,
            1.0,
        );
        x += step;
    }
    let mut y = (lo.y / step).ceil() * step;
    while y <= hi.y {
        canvas.polyline(
            &[Vec2::new(lo.x, y), Vec2::new(hi.x, y)],
            [40, 50, 60],
            255,
            1.0,
        );
        y += step;
    }

    for (i, w) in lab.walls().iter().enumerate() {
        // The first four walls are the arena boundary.
        let rgb = if i < 4 {
            [230, 230, 230]
        } else {
            [150, 165, 180]
        };
        canvas.polyline(
            &[Vec2::new(w[0], w[1]), Vec2::new(w[2], w[3])],
            rgb,
            255,
            2.0,
        );
    }

    for rider in lab.riders() {
        // The rubber zone widens with speed: it is the step it would cancel.
        let step = shared::BASE_SPEED * rider.target * shared::TICK_SECS;
        let zone = shared::SLOW_DOWN_DISTANCE.max(step / shared::RUBBER_APPROACH);
        let zone_px = zone * canvas.scale();
        let rgb = PALETTE[rider.index % PALETTE.len()].1;
        for death in &rider.deaths {
            let (d, r) = (*death, 5.0 / canvas.scale());
            canvas.polyline(&[d - Vec2::splat(r), d + Vec2::splat(r)], rgb, 255, 2.0);
            canvas.polyline(&[d + Vec2::new(-r, r), d + Vec2::new(r, -r)], rgb, 255, 2.0);
        }
        if !rider.alive {
            continue;
        }
        canvas.polyline(&rider.trail, rgb, 255, 2.0);
        if zone_px >= 6.0 {
            canvas.circle(rider.position, zone_px, rgb, 90, false);
        }
        canvas.circle(rider.position, 4.0, rgb, 255, true);
        let nose = rider.position + rider.direction * (12.0 / canvas.scale());
        canvas.polyline(&[rider.position, nose], [255, 255, 255], 255, 2.0);
    }
    canvas.pixmap
}
