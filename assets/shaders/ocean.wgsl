#import bevy_pbr::forward_io::VertexOutput

@group(2) @binding(0)
var ocean_mask: texture_2d<f32>;

const C_SQR: f32 = 0.87 * 0.87;

struct FaceUv {
    face: u32,
    uv: vec2<f32>,
}

fn inverse_cube_correction(q: f32) -> f32 {
    return q * sqrt(
        (1.0 + C_SQR) /
        (1.0 + C_SQR * q * q)
    );
}

fn direction_to_face_uv(direction: vec3<f32>) -> FaceUv {
    let d = normalize(direction);
    let a = abs(d);

    var face: u32 = 0u;
    var corrected_u: f32 = 0.0;
    var corrected_v: f32 = 0.0;

    if a.x >= a.y && a.x >= a.z {
        if d.x < 0.0 {
            // face 0: [-1, -v, u]
            face = 0u;
            corrected_u = d.z / a.x;
            corrected_v = -d.y / a.x;
        } else {
            // face 3: [1, -u, v]
            face = 3u;
            corrected_u = -d.y / a.x;
            corrected_v = d.z / a.x;
        }
    } else if a.y >= a.x && a.y >= a.z {
        if d.y > 0.0 {
            // face 2: [u, 1, v]
            face = 2u;
            corrected_u = d.x / a.y;
            corrected_v = d.z / a.y;
        } else {
            // face 5: [v, -1, u]
            face = 5u;
            corrected_u = d.z / a.y;
            corrected_v = d.x / a.y;
        }
    } else {
        if d.z > 0.0 {
            // face 1: [u, -v, 1]
            face = 1u;
            corrected_u = d.x / a.z;
            corrected_v = -d.y / a.z;
        } else {
            // face 4: [v, -u, -1]
            face = 4u;
            corrected_u = -d.y / a.z;
            corrected_v = d.x / a.z;
        }
    }

    let u =
        inverse_cube_correction(corrected_u) * 0.5 + 0.5;

    let v =
        inverse_cube_correction(corrected_v) * 0.5 + 0.5;

    return FaceUv(
        face,
        vec2<f32>(
            clamp(u, 0.0, 1.0),
            clamp(v, 0.0, 1.0),
        ),
    );
}

fn read_ocean_mask(face_uv: FaceUv) -> f32 {
    let dimensions = textureDimensions(ocean_mask);

    let face_width = dimensions.x / 3u;
    let face_height = dimensions.y / 2u;

    let column = face_uv.face % 3u;
    let row = face_uv.face / 3u;

    let local_x = min(
        u32(round(face_uv.uv.x * f32(face_width - 1u))),
        face_width - 1u,
    );

    let local_y = min(
        u32(round(face_uv.uv.y * f32(face_height - 1u))),
        face_height - 1u,
    );

    let atlas_x = column * face_width + local_x;
    let atlas_y = row * face_height + local_y;

    return textureLoad(
        ocean_mask,
        vec2<i32>(
            i32(atlas_x),
            i32(atlas_y),
        ),
        0,
    ).r;
}

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    let face_uv =
        direction_to_face_uv(mesh.world_normal);

    let water = read_ocean_mask(face_uv);
    if water < 0.25 {
        discard;
    }

    let lake = vec3<f32>(0.025, 0.34, 0.40);
    let ocean = vec3<f32>(0.01, 0.16, 0.42);
    return vec4<f32>(select(lake, ocean, water > 0.85), 1.0);
}
