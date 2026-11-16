// Fog of war: opaque where the mask is black, clear where it is white. The
// dim floor grid is drawn here from world coordinates, so it never sits over
// trails (the real grid is under the scene, hidden by this quad in the fog).
#import bevy_sprite::mesh2d_vertex_output::VertexOutput

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> floor_color: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> grid_color: vec4<f32>;
// Arena half width, half height, grid step, line thickness.
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<uniform> arena: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var mask_sampler: sampler;

// How much of the grid color shows in the fog.
const DIM: f32 = 0.5;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let seen = textureSample(mask, mask_sampler, in.uv).r;
    let p = in.world_position.xy;
    let cell = (p + arena.xy) / arena.z;
    let dist = abs(cell - round(cell)) * arena.z;
    // Antialiased: one screen pixel of falloff around the line's edge.
    let px = fwidth(p);
    let d = min(dist.x, dist.y);
    let w = max(px.x, px.y);
    let inside = abs(p.x) <= arena.x + arena.w && abs(p.y) <= arena.y + arena.w;
    let line = select(0.0, 1.0 - smoothstep(arena.w * 0.5 - w, arena.w * 0.5 + w, d), inside);
    let color = mix(floor_color, grid_color, line * DIM);
    return vec4<f32>(color.rgb, 1.0 - seen);
}
