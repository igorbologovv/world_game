import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings::view;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> sun: vec4<f32>;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let normal = normalize(in.world_normal);
    let daylight = max(dot(normal, sun.xyz), 0.0);
    let rock = 0.72 + 0.14 * sin(dot(normal, vec3<f32>(12.0, 7.0, 5.0)))
        * cos(dot(normal, vec3<f32>(4.0, -9.0, 16.0)));
    let radiance = rock * (4600.0 * daylight + 34.0);
    return vec4<f32>(vec3<f32>(0.94, 0.95, 1.0) * radiance * view.exposure, 1.0);
}
