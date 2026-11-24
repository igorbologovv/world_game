//! Texture fields generated once and shared across views and frames.

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDataOrder, TextureDimension, TextureFormat, TextureViewDescriptor,
        TextureViewDimension,
    },
};

const WEATHER_SIZE: u32 = 256;
const BILLOW_SIZE: u32 = 64;

pub(super) fn weather() -> Image {
    let mut data = Vec::with_capacity((6 * WEATHER_SIZE.pow(2) * 4) as usize);
    for face in 0..6 {
        for y in 0..WEATHER_SIZE {
            for x in 0..WEATHER_SIZE {
                let uv = (UVec2::new(x, y).as_vec2() + 0.5) * (2.0 / WEATHER_SIZE as f32) - 1.0;
                data.extend(weather_sample(cube_direction(face, uv)).map(unorm));
            }
        }
    }
    let mut image = with_mips(
        WEATHER_SIZE,
        6,
        TextureDimension::D2,
        TextureFormat::Rgba8Unorm,
        data,
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    image
}

fn cube_direction(face: u32, uv: Vec2) -> Vec3 {
    // WebGPU cubemap layer order: +X, -X, +Y, -Y, +Z, -Z.
    match face {
        0 => Vec3::new(1.0, -uv.y, -uv.x),
        1 => Vec3::new(-1.0, -uv.y, uv.x),
        2 => Vec3::new(uv.x, 1.0, uv.y),
        3 => Vec3::new(uv.x, -1.0, -uv.y),
        4 => Vec3::new(uv.x, -uv.y, 1.0),
        _ => Vec3::new(-uv.x, -uv.y, -1.0),
    }
    .normalize()
}

fn weather_sample(direction: Vec3) -> [f32; 4] {
    // Warp the low frequencies in 3D before sampling across the sphere.
    // Latitude-dependent shear stretches some of the systems into fronts.
    let p = direction * 11.0;
    let warp = Vec3::new(
        value_noise(p * 0.61, 0, 19),
        value_noise(p * 0.61, 0, 79),
        value_noise(p * 0.61, 0, 143),
    ) - 0.5;
    let q = p + 2.25 * warp;
    let shear = q + Vec3::new(q.y, 0.0, -q.y) * 0.75;
    let front = 0.61 * value_noise(shear * 0.86, 0, 263)
        + 0.27 * value_noise(shear * 1.91, 0, 89)
        + 0.12 * value_noise(q * 4.37, 0, 31);
    let bands = value_noise(shear * Vec3::new(2.1, 5.9, 2.1), 0, 37);
    let cover = (0.88 * front + 0.12 * bands - 0.31) / 0.37;
    let development = value_noise(q * 3.21, 0, 57);
    let upper = value_noise((shear + warp) * Vec3::new(1.7, 4.5, 1.7), 0, 97);
    let ice = (0.68 * upper + 0.32 * front - 0.44) / 0.30;
    // A separate lift field lets the same coverage form shallow sheets or
    // taller convection. The first three channels also serve the legacy pass.
    let lift = 0.72 * value_noise(q * 1.43 + Vec3::new(2.1, 5.8, 1.4), 0, 71)
        + 0.28 * value_noise(shear * 3.34, 0, 53);
    [cover, development, ice, lift]
}

pub(super) fn billows() -> Image {
    let size = BILLOW_SIZE;
    let mut data = Vec::with_capacity((size.pow(3) * 2) as usize);
    for z in 0..size {
        for y in 0..size {
            for x in 0..size {
                let p = (UVec3::new(x, y, z).as_vec3() + 0.5) / size as f32;
                let mass = 0.74 * value_noise(p * 4.0, 4, 41)
                    + 0.20 * value_noise(p * 8.0, 8, 113)
                    + 0.06 * value_noise(p * 16.0, 16, 174);
                let erosion = 0.70 * value_noise(p * 8.0, 8, 317)
                    + 0.22 * value_noise(p * 16.0, 16, 97)
                    + 0.08 * value_noise(p * 32.0, 32, 237);
                data.extend([unorm(mass), unorm(erosion)]);
            }
        }
    }
    let mut image = with_mips(
        size,
        size,
        TextureDimension::D3,
        TextureFormat::Rg8Unorm,
        data,
    );
    image.sampler = filtered(ImageAddressMode::Repeat);
    image
}

pub(super) fn shape() -> Image {
    let size = BILLOW_SIZE;
    let low = Worley::new(4);
    let high = Worley::new(8);
    let mut data = Vec::with_capacity((size.pow(3) * 4) as usize);
    for z in 0..size {
        for y in 0..size {
            for x in 0..size {
                let p = (UVec3::new(x, y, z).as_vec3() + 0.5) / size as f32;
                let broad = low.sample(p);
                let fine = high.sample(p);
                let base = 0.67 * value_noise(p * 4.0, 4, 41)
                    + 0.24 * value_noise(p * 8.0, 8, 113)
                    + 0.09 * value_noise(p * 16.0, 16, 174);
                let billow = 0.69 * broad + 0.31 * fine;
                let mass = 0.68 * base + 0.32 * billow;
                let grain = value_noise(p * 16.0, 16, 397);
                let erosion = 1.0 - (0.56 * fine + 0.27 * broad + 0.17 * grain);
                let core = 0.62 * value_noise(p * 8.0, 8, 307) + 0.23 * grain + 0.15 * fine;
                data.extend([mass, billow, erosion, core].map(unorm));
            }
        }
    }
    let mut image = with_mips(
        size,
        size,
        TextureDimension::D3,
        TextureFormat::Rgba8Unorm,
        data,
    );
    image.sampler = filtered(ImageAddressMode::Repeat);
    image
}

struct Worley {
    period: i32,
    points: Vec<Vec3>,
}

impl Worley {
    fn new(period: i32) -> Self {
        let mut points = Vec::with_capacity(period.pow(3) as usize);
        for z in 0..period {
            for y in 0..period {
                for x in 0..period {
                    let p = Vec3::new(x as f32, y as f32, z as f32);
                    points.push(Vec3::new(
                        value_noise(p, period, 11),
                        value_noise(p, period, 117),
                        value_noise(p, period, 293),
                    ));
                }
            }
        }
        Self { period, points }
    }

    fn sample(&self, p: Vec3) -> f32 {
        let p = p * self.period as f32;
        let floor = p.floor();
        let cell = floor.as_ivec3();
        let fraction = p - floor;
        let mut nearest = 1.0_f32;
        for z in -1..=1 {
            for y in -1..=1 {
                for x in -1..=1 {
                    let offset = IVec3::new(x, y, z);
                    let c = (cell + offset) & IVec3::splat(self.period - 1);
                    let index = (c.z * self.period + c.y) * self.period + c.x;
                    let point = offset.as_vec3() + self.points[index as usize];
                    nearest = nearest.min(point.distance_squared(fraction));
                }
            }
        }
        1.0 - nearest.sqrt()
    }
}

fn unorm(value: f32) -> u8 {
    (255.0 * value.clamp(0.0, 1.0)).round() as u8
}

fn value_noise(position: Vec3, period: i32, seed: u32) -> f32 {
    let floor = position.floor();
    let cell = floor.as_ivec3();
    let f = position - floor;
    let weight = f * f * (3.0 - 2.0 * f);
    let lattice = |x, y, z| {
        let mut c = cell + IVec3::new(x, y, z);
        if period != 0 {
            c = c.rem_euclid(IVec3::splat(period));
        }
        let mut h = seed
            ^ (c.x as u32).wrapping_mul(0x9e37_79b9)
            ^ (c.y as u32).wrapping_mul(0x85eb_ca6b)
            ^ (c.z as u32).wrapping_mul(0xc2b2_ae35);
        h = (h ^ (h >> 16)).wrapping_mul(0x7feb_352d);
        h = (h ^ (h >> 15)).wrapping_mul(0x846c_a68b);
        ((h ^ (h >> 16)) >> 8) as f32 * (1.0 / 16_777_215.0)
    };
    let slice = |z| {
        let a = lattice(0, 0, z).lerp(lattice(1, 0, z), weight.x);
        let b = lattice(0, 1, z).lerp(lattice(1, 1, z), weight.x);
        a.lerp(b, weight.y)
    };
    slice(0).lerp(slice(1), weight.z)
}

fn filtered(mode: ImageAddressMode) -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: mode,
        address_mode_v: mode,
        address_mode_w: mode,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    })
}

fn with_mips(
    side: u32,
    depth: u32,
    dimension: TextureDimension,
    format: TextureFormat,
    base_level: Vec<u8>,
) -> Image {
    let mut image = Image::new(
        Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: depth,
        },
        dimension,
        base_level,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = filtered(ImageAddressMode::ClampToEdge);
    image.texture_descriptor.mip_level_count = side.ilog2() + 1;
    image.data_order = TextureDataOrder::MipMajor;

    // Keep cube faces as separate layers in each mip. For the volume, each
    // texel averages a 2x2x2 block and the depth halves with width and height.
    let texels = image.data.as_mut().unwrap();
    let channels = if format == TextureFormat::Rg8Unorm {
        2
    } else {
        4
    };
    let z_stride = if dimension == TextureDimension::D3 {
        2
    } else {
        1
    };
    let mut offset = 0;
    let mut width = side as usize;
    let mut layers = depth as usize;
    while width > 1 {
        for z in (0..layers).step_by(z_stride) {
            for y in (0..width).step_by(2) {
                for x in (0..width).step_by(2) {
                    for channel in 0..channels {
                        let mut sum = 0;
                        for dz in 0..z_stride {
                            for dy in 0..2 {
                                let index = ((z + dz) * width + y + dy) * width + x;
                                let index = offset + index * channels + channel;
                                sum += texels[index] as usize + texels[index + channels] as usize;
                            }
                        }
                        let samples = 4 * z_stride;
                        texels.push(((sum + samples / 2) / samples) as u8);
                    }
                }
            }
        }
        offset += layers * width * width * channels;
        width /= 2;
        layers /= z_stride;
    }
    image
}
