@group(constants::MATERIAL_BIND_GROUP) @binding(1)
var terrain_sampler: sampler;

fn noise_hash(cell: vec3<f32>) -> f32 {
    var h = fract(cell * vec3<f32>(0.1031, 0.1030, 0.0973));
    h += dot(h, h.yxz + 33.33);
    return fract((h.x + h.y) * h.z);
}

fn macro_noise(position: vec3<f32>) -> f32 {
    let cell = floor(position);
    return noise_cell(cell, fract(position));
}

fn noise_cell(cell: vec3<f32>, t: vec3<f32>) -> f32 {
    let w = t * t * (3.0 - 2.0 * t);
    let near = mix(
        mix(noise_hash(cell), noise_hash(cell + vec3<f32>(1.0, 0.0, 0.0)), w.x),
        mix(noise_hash(cell + vec3<f32>(0.0, 1.0, 0.0)), noise_hash(cell + vec3<f32>(1.0, 1.0, 0.0)), w.x),
        w.y,
    );
    let far_cell = cell + vec3<f32>(0.0, 0.0, 1.0);
    let far = mix(
        mix(noise_hash(far_cell), noise_hash(far_cell + vec3<f32>(1.0, 0.0, 0.0)), w.x),
        mix(noise_hash(far_cell + vec3<f32>(0.0, 1.0, 0.0)), noise_hash(far_cell + vec3<f32>(1.0, 1.0, 0.0)), w.x),
        w.y,
    );
    return mix(near, far, w.z) - 0.5;
}

fn filtered_noise(position: vec3<f32>, strength: f32) -> f32 {
    if strength > 0.0 {
        return macro_noise(position) * strength;
    }
    return 0.0;
}

struct Projection {
    uv: vec2<f32>,
    dx: vec2<f32>,
    dy: vec2<f32>,
    tangent: vec3<f32>,
    bitangent: vec3<f32>,
    weight: f32,
}

struct SurfaceSample {
    color: vec3<f32>,
    normal_offset: vec3<f32>,
    roughness: f32,
}

fn triplanar_projections(
    position: vec3<f32>, dx: vec3<f32>, dy: vec3<f32>, normal: vec3<f32>,
) -> array<Projection, 3> {
    let sign = select(vec3<f32>(-1.0), vec3<f32>(1.0), normal >= vec3<f32>(0.0));
    let squared = normal * normal;
    let axis_weights = squared * squared;
    let normalized = axis_weights / dot(axis_weights, vec3<f32>(1.0));
    // Fade negligible projections to zero before sampling. Renormalizing
    // keeps both the color and the normal blend continuous across slopes.
    let contributing = normalized * smoothstep(vec3<f32>(0.005), vec3<f32>(0.02), normalized);
    let weights = contributing / dot(contributing, vec3<f32>(1.0));

    // Flip the V axis on negative faces so each normal map has a right-handed basis.
    let x_uv = vec2<f32>(1.0, sign.x);
    let y_uv = vec2<f32>(1.0, sign.y);
    let z_uv = vec2<f32>(1.0, sign.z);
    return array(
        Projection(position.yz * x_uv, dx.yz * x_uv, dy.yz * x_uv,
            vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, sign.x), weights.x),
        Projection(position.zx * y_uv, dx.zx * y_uv, dy.zx * y_uv,
            vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(sign.y, 0.0, 0.0), weights.y),
        Projection(position.xy * z_uv, dx.xy * z_uv, dy.xy * z_uv,
            vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, sign.z, 0.0), weights.z),
    );
}
