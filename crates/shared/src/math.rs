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

/// Deterministic centered spawn, with the original JS's wall margin and
/// cardinal heading. A connection's unique entity bits provide its seed.
pub fn spawn_from_seed(mut seed: u64) -> (Vec2, Vec2) {
    let mut random = || {
        seed = seed.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = seed;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        ((value ^ (value >> 31)) >> 40) as f32 / 16_777_216.0
    };
    let x = (random() - 0.5) * (crate::ARENA_WIDTH - 200.0);
    let y = (random() - 0.5) * (crate::ARENA_HEIGHT - 200.0);
    let direction = match (random() * 4.0) as u32 {
        0 => Vec2::X,
        1 => Vec2::Y,
        2 => Vec2::NEG_X,
        _ => Vec2::NEG_Y,
    };
    (Vec2::new(x, y), direction)
}
