// Projection onto the existing ocean triangles. The detail mesh subdivides the
// same spherical datum and reads the same texel coordinates as the global mesh.
struct CubeChart {
    face: u32,
    uv: vec2<f32>,
}

struct OceanPlane {
    uv: vec2<f32>,
    altitude: f32,
    normal: vec3<f32>,
}

fn cube_chart(up: vec3<f32>, correction: f32) -> CubeChart {
    let a = abs(up);
    var face: u32;
    var plane: vec2<f32>;
    if a.x > a.y && a.x > a.z {
        face = select(0u, 3u, up.x > 0.0);
        plane = select(vec2<f32>(up.z, -up.y), vec2<f32>(-up.y, up.z), up.x > 0.0) / a.x;
    } else if a.z > a.y {
        face = select(4u, 1u, up.z > 0.0);
        plane = select(vec2<f32>(-up.y, up.x), vec2<f32>(up.x, -up.y), up.z > 0.0) / a.z;
    } else {
        face = select(5u, 2u, up.y > 0.0);
        plane = select(up.zx, up.xz, up.y > 0.0) / a.y;
    }
    let uv = plane * sqrt((1.0 + correction) / (1.0 + correction * plane * plane));
    return CubeChart(face, saturate(uv * 0.5 + 0.5));
}

fn cube_direction(face: u32, uv: vec2<f32>, correction: f32) -> vec3<f32> {
    let cube = uv * 2.0 - 1.0;
    let p = cube * inverseSqrt(1.0 + correction - correction * cube * cube);
    switch face {
        case 0u: { return normalize(vec3<f32>(-1.0, -p.y, p.x)); }
        case 1u: { return normalize(vec3<f32>(p.x, -p.y, 1.0)); }
        case 2u: { return normalize(vec3<f32>(p.x, 1.0, p.y)); }
        case 3u: { return normalize(vec3<f32>(1.0, -p.x, p.y)); }
        case 4u: { return normalize(vec3<f32>(p.y, -p.x, -1.0)); }
        default: { return normalize(vec3<f32>(p.y, -1.0, p.x)); }
    }
}

fn source_pixels(uv: vec2<f32>, size: f32, offset: f32) -> vec2<f32> {
    return clamp((uv - offset) * size - 0.5, vec2<f32>(0.0), vec2<f32>(size - 1.0));
}

fn ocean_plane(
    up: vec3<f32>, chart: CubeChart, segments: f32, radius: f32,
    size: f32, correction: f32, texel_offset: f32,
) -> OceanPlane {
    let cell = min(floor(chart.uv * segments), vec2<f32>(segments - 1.0));
    let a_uv = cell / segments;
    let b_uv = (cell + vec2<f32>(1.0, 0.0)) / segments;
    let c_uv = (cell + vec2<f32>(0.0, 1.0)) / segments;
    let d_uv = (cell + 1.0) / segments;
    let a = cube_direction(chart.face, a_uv, correction);
    let b = cube_direction(chart.face, b_uv, correction);
    let c = cube_direction(chart.face, c_uv, correction);
    let diagonal = cross(b, c - b);
    let upper = dot(up - b, diagonal) * dot(a - b, diagonal) >= 0.0;
    var start = a;
    if !upper {
        start = cube_direction(chart.face, d_uv, correction);
    }

    let edge_b = b - start;
    let edge_c = c - start;
    let n = cross(edge_b, edge_c);
    let relative = up - start;
    let to_plane = -dot(n, relative) / dot(n, up);
    let point = relative + up * to_plane;
    let weights = vec2<f32>(dot(n, cross(point, edge_c)), dot(n, cross(edge_b, point))) / dot(n, n);
    let start_uv = source_pixels(select(d_uv, a_uv, upper), size, texel_offset);
    let b_pixel = source_pixels(b_uv, size, texel_offset);
    let c_pixel = source_pixels(c_uv, size, texel_offset);
    let origin = vec2<f32>(f32(chart.face % 3u), f32(chart.face / 3u)) * size;
    let pixel = origin + start_uv + (b_pixel - start_uv) * weights.x + (c_pixel - start_uv) * weights.y;

    // dot(up, a) = 1 - |up - a|²/2 for unit vectors. This retains the
    // original chord sag with no subtraction of planet-scale radii.
    let db = up - b;
    let dc = up - c;
    let sag = -0.5 * radius * dot(vec3<f32>(1.0 - weights.x - weights.y, weights),
                                  vec3<f32>(dot(relative, relative), dot(db, db), dot(dc, dc)));
    return OceanPlane(pixel, sag, normalize(select(n, -n, dot(n, up) < 0.0)));
}

struct AtlasGradient {
    u: vec3<f32>,
    v: vec3<f32>,
}

// Gradients in texels per meter, including the cube warp. Their precision is
// independent of the absolute texel coordinates and the view's pixel size.
fn atlas_gradient(up: vec3<f32>, face: u32, size: f32, radius: f32, correction: f32) -> AtlasGradient {
    let x = vec3<f32>(1.0, 0.0, 0.0);
    let y = vec3<f32>(0.0, 1.0, 0.0);
    let z = vec3<f32>(0.0, 0.0, 1.0);
    var u: vec3<f32>;
    var v: vec3<f32>;
    var q: vec3<f32>;
    switch face {
        case 0u: { u = z; v = -y; q = -x; }
        case 1u: { u = x; v = -y; q = z; }
        case 2u: { u = x; v = z; q = y; }
        case 3u: { u = -y; v = z; q = x; }
        case 4u: { u = -y; v = x; q = -z; }
        default: { u = z; v = x; q = -y; }
    }
    let inv = 1.0 / dot(q, up);
    let plane = vec2<f32>(dot(u, up), dot(v, up)) * inv;
    let scale = inverseSqrt(1.0 + correction * plane * plane);
    let derivative = (size * 0.5 / radius) * sqrt(1.0 + correction) * inv * scale * scale * scale;
    return AtlasGradient((u - plane.x * q) * derivative.x, (v - plane.y * q) * derivative.y);
}
