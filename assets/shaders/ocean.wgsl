#import bevy_pbr::forward_io::VertexOutput

@group(2) @binding(0)
var ocean_mask: texture_2d<f32>;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let water = textureLoad(ocean_mask, vec2<i32>(round(mesh.uv)), 0).r;
    if water < 0.5371 {
        discard;
    }

    let lake = vec3<f32>(0.025, 0.34, 0.40);
    let ocean = vec3<f32>(0.01, 0.16, 0.42);
    return vec4<f32>(select(lake, ocean, water > 0.9309), 1.0);
}
