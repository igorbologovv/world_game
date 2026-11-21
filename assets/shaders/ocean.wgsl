#import "shaders/water_common.wgsl"::{WaterVertex, water_vertex, water_light}
#import bevy_pbr::forward_io::Vertex
#import bevy_pbr::mesh_view_bindings::view

@group(2) @binding(0)
var ocean_mask: texture_2d<f32>;
@group(2) @binding(1)
var ocean_sampler: sampler;

@vertex
fn vertex(mesh: Vertex) -> WaterVertex {
    return water_vertex(mesh);
}

@fragment
fn fragment(mesh: WaterVertex) -> @location(0) vec4<f32> {
    // Keep the existing generated ocean mask and shoreline cutouts.
    let water_texel = textureLoad(ocean_mask, vec2<i32>(round(mesh.uv)), 0).rg;
    if water_texel.x < 0.5371 {
        discard;
    }

    // Imported woodland pond, material_3 (Blender principled BSDF):
    // Base Color (1.0, 0.937488, 0.892829), Alpha 0.6,
    // Roughness 0.5, Metallic 0, Transmission 0, BLENDED.
    // This is an approximation of its PBR shading, not an imported texture.
    let up = normalize(mesh.planet_position);
    let eye = normalize(view.world_position - mesh.world_position);
    let base_color = vec3<f32>(1.0, 0.937488, 0.892829);
    let surface = water_light(base_color, up, up, eye, 0.5);
    return vec4<f32>(surface, 0.6);
}
