import bevy_pbr::render::mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT;
import bevy_pbr::render::pbr_types::PbrInput;

// Integrate the distance a ray travels through an approximate crown volume.
// Coordinates are relative to the canonical tree's impostor framing sphere.
fn crown_path(position: vec3<f32>, direction: vec3<f32>) -> f32 {
    let radii = vec3<f32>(0.76, 0.78, 0.76);
    let point = (position - vec3<f32>(0.0, 0.13, 0.0)) / radii;
    let ray = normalize(direction / radii);
    let along = -dot(point, ray);
    let discriminant = along * along + 1.0 - dot(point, point);
    let half_chord = sqrt(max(discriminant, 0.0));
    return max(along + half_chord, 0.0) - max(along - half_chord, 0.0);
}

fn shade_broadleaf(
    pbr: ptr<function, PbrInput>,
    position: vec3<f32>,
    sun: vec3<f32>,
    foliage: f32,
) {
    // Long paths represent overlapping leaves. Keep some diffuse fill from
    // the remaining sky and light scattered by the surrounding foliage.
    let sky = mix(0.34, 1.0, exp(-1.4 * crown_path(position, vec3<f32>(0.0, 1.0, 0.0))));
    let direct = mix(0.15, 1.0, exp(-1.8 * crown_path(position, sun)));
    (*pbr).diffuse_occlusion *= mix(1.0, sky, foliage);
    (*pbr).specular_occlusion *= mix(1.0, sky, foliage);
    (*pbr).flags |= MESH_FLAGS_SHADOW_RECEIVER_BIT;
    (*pbr).directional_shadow_factor *= mix(1.0, direct, foliage);
    // Bevy evaluates diffuse transmission as a separate back-light lobe.
    // Attenuate the light reaching that lobe through the crown as well.
    (*pbr).material.diffuse_transmission *= mix(1.0, mix(sky, direct, 0.75), foliage);
    (*pbr).material.perceptual_roughness = mix((*pbr).material.perceptual_roughness, 0.84, foliage);
    (*pbr).material.reflectance = mix((*pbr).material.reflectance, vec3<f32>(0.28), foliage);
}
