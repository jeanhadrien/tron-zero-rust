//! Draw order of the 2D scene, back to front.
//!
//! Sprites and meshes are layered by the z of their `Transform`. Gizmos
//! (trails, riders, walls, debug overlay) ignore depth in 2D: they all draw
//! after every sprite, in system order, so nothing can sit between or behind
//! them unless it is a sprite or mesh. Put such things at a `Z_*` below.

/// Distance of the camera from the z = 0 plane.
pub const CAMERA_Z: f32 = 999.0;
/// Lowest z the camera still renders. Anything below is clipped.
pub const MIN_VISIBLE_Z: f32 = -10.0;
/// Projection far plane, derived so `MIN_VISIBLE_Z` is the lowest visible z.
pub const CAMERA_FAR: f32 = CAMERA_Z - MIN_VISIBLE_Z;

/// Arena grid, behind every gizmo.
pub const Z_GRID: f32 = -5.0;

/// Rider name labels (`Text2d`). Still drawn before gizmos, so under trails.
pub const Z_LABEL: f32 = 10.0;

const _: () = assert!(
    Z_GRID >= MIN_VISIBLE_Z && Z_LABEL >= MIN_VISIBLE_Z,
    "layer would be clipped by the camera"
);
