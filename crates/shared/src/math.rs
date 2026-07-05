//! Pure math helpers: rotations, arena wall generation.

use glam::Vec2;

/// Rotate a ±X/±Y heading 90° counter-clockwise: `(x, y) -> (-y, x)`.
#[inline]
pub fn rotate_left(v: Vec2) -> Vec2 {
    Vec2::new(-v.y, v.x)
}

/// Rotate a ±X/±Y heading 90° clockwise: `(x, y) -> (y, -x)`.
#[inline]
pub fn rotate_right(v: Vec2) -> Vec2 {
    Vec2::new(v.y, -v.x)
}

/// Build the four boundary walls of an arena centred at the origin.
pub fn arena_walls(width: f32, height: f32) -> Vec<[f32; 4]> {
    let hw = width * 0.5;
    let hh = height * 0.5;
    vec![
        [-hw, -hh, hw, -hh],
        [hw, -hh, hw, hh],
        [hw, hh, -hw, hh],
        [-hw, hh, -hw, -hh],
    ]
}
