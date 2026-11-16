//! Fog of war (`docs/specs/fog.md`): cone, proximity, glow and sparks.
//!
//! When the server announces `FogRules` (and this is no tutorial) the scene is
//! rebuilt around four cameras, back to front:
//!
//! * mask camera (order -2, layer [`MASK_LAYER`]): draws the visible area
//!   white on black into a half-resolution texture, from one mesh rebuilt each
//!   frame;
//! * scene camera (order -1, layer 0): the whole world, as before;
//! * the main camera (order 0, hosts egui) moves to [`FOG_LAYER`] and stops
//!   clearing: it draws the fog quad and the walls over the scene, then egui.
//!
//! All cameras are `Camera2d`, so `follow_player` keeps them on the same spot.
//! Without `FogRules` none of this exists.

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::camera::{ClearColorConfig, RenderTarget};
use bevy::gizmos::config::{GizmoConfigGroup, GizmoConfigStore};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, TextureFormat};
use bevy::shader::ShaderRef;
use bevy::sprite_render::{AlphaMode2d, Material2d, Material2dPlugin};
use bevy::window::PrimaryWindow;
use shared::WallSegments;
use shared::physics::Segment;
use shared::visibility::{
    FOG_GLOW_RADIUS, FOG_PROXIMITY_RADIUS, FOG_SPARKS_RADIUS, FOG_SPARKS_SECS, Vertex,
    visibility_polygon, visible_parts,
};
use shared::{ARENA_HEIGHT, ARENA_WIDTH, Arena, Direction, FogRules, IsAlive, IsSliding, Player};

use super::arena::{GRID_STEP, GRID_THICKNESS};
use super::camera::CAMERA_VIEW_HEIGHT;
use super::smooth::{OwnRider, RenderSet, Rendered};
use crate::theme::Theme;
use crate::tutorial::TutorialState;

/// Render layer of the mask camera and the shapes it draws.
const MASK_LAYER: usize = 1;
/// Render layer of what draws over the fog: the fog quad and the walls.
const FOG_LAYER: usize = 2;
/// The mask texture is this many times smaller than the window (soft edges).
const MASK_DOWNSCALE: u32 = 2;
/// Sides of the polygon that stands for a circle in the mask.
const CIRCLE_SIDES: u32 = 32;
/// Distance of the fog quad in front of its camera.
const QUAD_DEPTH: f32 = 100.0;

/// Gizmo group drawn by the fog camera, above the fog (arena walls).
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct FogGizmos;

/// Marks everything [`fog_lifecycle`] spawned, to despawn it with the fog.
#[derive(Component)]
struct FogEntity;

#[derive(Component)]
struct FogQuad;

/// Debug builds: F3 shows the arena without fog.
#[derive(Resource, Default)]
struct FogDebugOff(bool);

/// The fog scene, present while fog is on.
#[derive(Resource)]
pub struct FogScene {
    mask: Handle<Image>,
    shapes: Handle<Mesh>,
    material: Handle<FogMaterial>,
    sparks: Sparks,
}

/// The fog quad: opaque where the mask is black, with the dim grid drawn on it
/// (`fog.wgsl`).
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct FogMaterial {
    #[uniform(0)]
    floor: LinearRgba,
    #[uniform(1)]
    grid: LinearRgba,
    /// Arena half width, half height, grid step, line thickness.
    #[uniform(2)]
    arena: Vec4,
    #[texture(3)]
    #[sampler(4)]
    mask: Handle<Image>,
}

impl Material2d for FogMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://tron_zero_client/render/fog.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Blend
    }
}

pub fn add_fog(app: &mut App) {
    embedded_asset!(app, "fog.wgsl");
    app.add_plugins(Material2dPlugin::<FogMaterial>::default());
    app.init_gizmo_group::<FogGizmos>();
    app.world_mut()
        .resource_mut::<GizmoConfigStore>()
        .config_mut::<FogGizmos>()
        .0
        .render_layers = RenderLayers::layer(FOG_LAYER);
    app.init_resource::<FogDebugOff>();
    #[cfg(debug_assertions)]
    app.add_systems(Update, toggle_debug_off);
    app.add_systems(
        Update,
        (fog_lifecycle, update_fog).chain().in_set(RenderSet::Draw),
    );
}

#[cfg(debug_assertions)]
fn toggle_debug_off(keys: Res<ButtonInput<KeyCode>>, mut off: ResMut<FogDebugOff>) {
    if keys.just_pressed(KeyCode::F3) {
        off.0 = !off.0;
    }
}

/// Build the fog scene when fog turns on, tear it down when it turns off.
#[allow(clippy::too_many_arguments)]
fn fog_lifecycle(
    mut commands: Commands,
    rules: Query<(), With<FogRules>>,
    tutorial: Res<TutorialState>,
    off: Res<FogDebugOff>,
    scene: Option<Res<FogScene>>,
    mut main: Query<
        (Entity, &mut Camera, &Projection, &Transform),
        (With<Camera2d>, Without<FogEntity>),
    >,
    fog: Query<Entity, With<FogEntity>>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut colors: ResMut<Assets<ColorMaterial>>,
    mut materials: ResMut<Assets<FogMaterial>>,
) {
    let want = !rules.is_empty() && !tutorial.active() && !off.0;
    let Ok((cam, mut camera, projection, transform)) = main.single_mut() else {
        return;
    };
    if !want {
        if scene.is_some() {
            for entity in &fog {
                commands.entity(entity).despawn();
            }
            commands.remove_resource::<FogScene>();
            commands.entity(cam).remove::<RenderLayers>();
            camera.clear_color = ClearColorConfig::Default;
        }
        return;
    }
    if scene.is_some() {
        return;
    }

    let mask = images.add(Image::new_target_texture(
        2,
        2,
        TextureFormat::Rgba8Unorm,
        None,
    ));
    let material = materials.add(FogMaterial {
        floor: LinearRgba::BLACK,
        grid: LinearRgba::BLACK,
        arena: Vec4::new(
            ARENA_WIDTH * 0.5,
            ARENA_HEIGHT * 0.5,
            GRID_STEP,
            GRID_THICKNESS,
        ),
        mask: mask.clone(),
    });
    let mut initial = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    let mut first = Mask::default();
    first.circle(Vec2::ZERO, 1.0);
    first.apply(&mut initial);
    let shapes = meshes.add(initial);

    // The main camera keeps egui and the top of the stack; the world moves to
    // a camera below it.
    camera.clear_color = ClearColorConfig::None;
    commands.entity(cam).insert(RenderLayers::layer(FOG_LAYER));
    commands.entity(cam).with_child((
        FogEntity,
        FogQuad,
        Mesh2d(meshes.add(Rectangle::default())),
        MeshMaterial2d(material.clone()),
        Transform::from_xyz(0.0, 0.0, -QUAD_DEPTH),
        RenderLayers::layer(FOG_LAYER),
        Visibility::Hidden,
    ));
    commands.spawn((
        FogEntity,
        Camera2d,
        Camera {
            order: -1,
            ..default()
        },
        projection.clone(),
        *transform,
    ));
    commands.spawn((
        FogEntity,
        Camera2d,
        Camera {
            order: -2,
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        RenderTarget::Image(mask.clone().into()),
        Msaa::Off,
        projection.clone(),
        *transform,
        RenderLayers::layer(MASK_LAYER),
    ));
    commands.spawn((
        FogEntity,
        Mesh2d(shapes.clone()),
        MeshMaterial2d(colors.add(ColorMaterial::default())),
        RenderLayers::layer(MASK_LAYER),
        // The mesh is rebuilt every frame and its bounds are not.
        NoFrustumCulling,
    ));
    commands.insert_resource(FogScene {
        mask,
        shapes,
        material,
        sparks: Sparks::default(),
    });
}

/// Each frame: size the mask and the quad to the window, rebuild the visible
/// area from the local rider's pose and every trail drawn.
#[allow(clippy::too_many_arguments)]
fn update_fog(
    time: Res<Time>,
    theme: Res<Theme>,
    window: Query<&Window, With<PrimaryWindow>>,
    rules: Query<&FogRules>,
    own: Query<(&Rendered, &Direction, &IsAlive), (With<Player>, With<OwnRider>)>,
    riders: Query<(Entity, &Rendered, &IsSliding, &IsAlive), With<Player>>,
    walls: Query<&WallSegments, With<Arena>>,
    mut quad: Query<(&mut Transform, &mut Visibility), With<FogQuad>>,
    scene: Option<ResMut<FogScene>>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FogMaterial>>,
) {
    let (Some(mut scene), Ok(window), Ok(rules), Ok((mut quad_at, mut quad_vis))) =
        (scene, window.single(), rules.single(), quad.single_mut())
    else {
        return;
    };

    let size = Extent3d {
        width: (window.physical_width() / MASK_DOWNSCALE).max(1),
        height: (window.physical_height() / MASK_DOWNSCALE).max(1),
        depth_or_array_layers: 1,
    };
    if images
        .get(&scene.mask)
        .is_some_and(|image| image.texture_descriptor.size != size)
        && let Some(mut image) = images.get_mut(&scene.mask)
    {
        image.resize(size);
    }
    let aspect = window.width() / window.height().max(1.0);
    quad_at.scale = Vec3::new(CAMERA_VIEW_HEIGHT * aspect, CAMERA_VIEW_HEIGHT, 1.0);

    if (theme.is_changed() || scene.is_added())
        && let Some(mut material) = materials.get_mut(&scene.material)
    {
        material.floor = theme.background.bevy().to_linear();
        material.grid = theme.grid.bevy().to_linear();
    }

    // Dead or not yet riding: nothing to see from, no fog for now.
    let Ok((own, dir, alive)) = own.single() else {
        *quad_vis = Visibility::Hidden;
        return;
    };
    if !alive.0 {
        *quad_vis = Visibility::Hidden;
        return;
    }
    *quad_vis = Visibility::Inherited;

    let eye = own.head;
    let heading = own.heading(dir.0);
    let mut segments: Vec<Segment> = Vec::new();
    for (entity, rendered, _, _) in &riders {
        segments.extend(Segment::trail_legs(&rendered.trail, entity.to_bits()));
    }
    let trail_legs = segments.len();
    if let Ok(walls) = walls.single() {
        segments.extend(walls.0.iter().map(|w| Segment {
            a: Vec2::new(w[0], w[1]),
            b: Vec2::new(w[2], w[3]),
            owner: None,
            from_head: 0,
        }));
    }

    let polygon = visibility_polygon(eye, heading, rules.cone_angle, rules.cone_range, &segments);
    let mut mask = Mask::default();
    mask.fan(eye, polygon.iter().map(|v| v.point));
    mask.circle(eye, FOG_PROXIMITY_RADIUS);
    if rules.glow {
        let grow = Vec2::splat(FOG_GLOW_RADIUS);
        for (lo, hi) in seen_spans(&polygon, trail_legs) {
            mask.rect(lo - grow, hi + grow);
        }
        // Your whole trail lights the map, seen or not.
        for leg in own.trail.windows(2) {
            mask.rect(leg[0].min(leg[1]) - grow, leg[0].max(leg[1]) + grow);
        }
    }
    if rules.sparks {
        scene.sparks.step(
            time.delta_secs(),
            riders
                .iter()
                .map(|(e, r, sliding, alive)| (e, r.head, sliding.0 && alive.0)),
        );
        for (at, radius) in scene.sparks.circles() {
            mask.circle(at, radius);
        }
    }
    if let Some(mut mesh) = meshes.get_mut(&scene.shapes) {
        mask.apply(&mut mesh);
    }
}

/// The seen stretch of each trail leg (`min`, `max` corners), merged from the
/// pieces of the polygon. Indices from `trail_legs` on are walls, which need
/// no glow.
fn seen_spans(polygon: &[Vertex], trail_legs: usize) -> Vec<(Vec2, Vec2)> {
    let mut spans: HashMap<usize, (Vec2, Vec2)> = HashMap::default();
    for (leg, a, b) in visible_parts(polygon).filter(|p| p.0 < trail_legs) {
        let span = spans.entry(leg).or_insert((a, a));
        span.0 = span.0.min(a).min(b);
        span.1 = span.1.max(a).max(b);
    }
    spans.into_values().collect()
}

/// Grinding circles, kept client-side: a rider that grinds reveals a circle
/// around its head; when the grind ends the circle stays where it was and
/// shrinks to nothing over `FOG_SPARKS_SECS`.
#[derive(Default)]
struct Sparks(HashMap<Entity, (Vec2, f32)>);

impl Sparks {
    /// Advance by `dt`. `riders` yields `(rider, head, grinding)`.
    fn step(&mut self, dt: f32, riders: impl Iterator<Item = (Entity, Vec2, bool)>) {
        for (_, (_, left)) in self.0.iter_mut() {
            *left -= dt;
        }
        for (rider, head, grinding) in riders {
            if grinding {
                self.0.insert(rider, (head, FOG_SPARKS_SECS));
            }
        }
        self.0.retain(|_, (_, left)| *left > 0.0);
    }

    /// `(center, radius)` of every circle to reveal.
    fn circles(&self) -> impl Iterator<Item = (Vec2, f32)> + '_ {
        self.0
            .values()
            .map(|&(at, left)| (at, FOG_SPARKS_RADIUS * (left / FOG_SPARKS_SECS).min(1.0)))
    }
}

/// Triangles of the visible area, built on the CPU and uploaded as one mesh.
#[derive(Default)]
struct Mask {
    pos: Vec<[f32; 3]>,
    idx: Vec<u32>,
}

impl Mask {
    fn vert(&mut self, p: Vec2) -> u32 {
        self.pos.push([p.x, p.y, 0.0]);
        self.pos.len() as u32 - 1
    }

    /// Triangle fan from `center` over `ring`, counter-clockwise.
    fn fan(&mut self, center: Vec2, ring: impl IntoIterator<Item = Vec2>) {
        let c = self.vert(center);
        let mut prev = None;
        for p in ring {
            let i = self.vert(p);
            if let Some(q) = prev {
                self.idx.extend([c, q, i]);
            }
            prev = Some(i);
        }
    }

    fn circle(&mut self, center: Vec2, radius: f32) {
        self.fan(
            center,
            (0..=CIRCLE_SIDES).map(|i| {
                center
                    + radius
                        * Vec2::from_angle(i as f32 / CIRCLE_SIDES as f32 * std::f32::consts::TAU)
            }),
        );
    }

    fn rect(&mut self, lo: Vec2, hi: Vec2) {
        let base = self.pos.len() as u32;
        for p in [lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)] {
            self.vert(p);
        }
        self.idx
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    fn apply(self, mesh: &mut Mesh) {
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.pos);
        mesh.insert_indices(Indices::U32(self.idx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vertex(x: f32, y: f32, hit: Option<usize>) -> Vertex {
        Vertex {
            point: Vec2::new(x, y),
            hit,
        }
    }

    #[test]
    fn spans_merge_pieces_of_a_leg_and_skip_walls() {
        let polygon = [
            vertex(5.0, 0.0, Some(0)),
            vertex(5.0, 10.0, Some(0)),
            vertex(5.0, 20.0, None),
            vertex(5.0, 40.0, Some(0)),
            vertex(5.0, 30.0, Some(0)),
            vertex(9.0, 30.0, Some(1)),
            vertex(9.0, 31.0, Some(1)),
        ];
        // Leg 1 is a wall: only legs below `trail_legs` count.
        let spans = seen_spans(&polygon, 1);
        assert_eq!(spans, vec![(Vec2::new(5.0, 0.0), Vec2::new(5.0, 40.0))]);
    }

    #[test]
    fn sparks_hold_while_grinding_then_shrink_to_nothing() {
        let rider = Entity::from_raw_u32(1).unwrap();
        let mut sparks = Sparks::default();
        sparks.step(0.5, [(rider, Vec2::X, true)].into_iter());
        let (at, radius) = sparks.circles().next().unwrap();
        assert_eq!((at, radius), (Vec2::X, FOG_SPARKS_RADIUS));
        // The grind ended and the rider moved on: the circle stays put.
        sparks.step(FOG_SPARKS_SECS / 2.0, [(rider, Vec2::Y, false)].into_iter());
        let (at, radius) = sparks.circles().next().unwrap();
        assert_eq!(at, Vec2::X);
        assert!((radius - FOG_SPARKS_RADIUS / 2.0).abs() < 1e-3);
        sparks.step(FOG_SPARKS_SECS, [(rider, Vec2::Y, false)].into_iter());
        assert_eq!(sparks.circles().count(), 0);
    }

    #[test]
    fn mask_shapes_index_their_own_vertices() {
        let mut mask = Mask::default();
        mask.circle(Vec2::ZERO, 1.0);
        mask.rect(Vec2::ZERO, Vec2::ONE);
        assert_eq!(mask.idx.len(), CIRCLE_SIDES as usize * 3 + 6);
        assert!(mask.idx.iter().all(|&i| (i as usize) < mask.pos.len()));
    }
}
