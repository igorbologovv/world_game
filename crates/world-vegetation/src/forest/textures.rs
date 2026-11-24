use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

use super::{Random, Species, FOLIAGE_CUTOFF};

const SWATCH: usize = 128;

pub(super) fn canopy_uv(uv: Vec2, individual_leaf: bool) -> Vec2 {
    let offset = if individual_leaf { 0.5 } else { 0.0 };
    (Vec2::splat(0.025) + uv * 0.95) * Vec2::new(0.5, 1.0) + Vec2::new(offset, 0.0)
}

pub(super) fn canopy_texture(species: Species) -> Image {
    let mut pixels = vec![[188, 188, 188, 0]; SWATCH * SWATCH * 2];
    let mut rng = Random::new(724391 ^ species as u64 * 32719);
    for _ in 0..150 {
        let angle = rng.range(0.0, std::f32::consts::TAU);
        let radius = rng.unit().sqrt();
        let (sin, cos) = angle.sin_cos();
        let spread_y = if matches!(species, Species::Pine) {
            0.92
        } else {
            0.76
        };
        let center = Vec2::new(
            0.5 + radius * cos * 0.36,
            0.49 + radius * sin * 0.38 * spread_y,
        );
        let size = rng.range(0.09, 0.23);
        paint_leaf(
            &mut pixels,
            0,
            species,
            center,
            angle + rng.range(-0.8, 0.8),
            size,
            rng.range(0.67, 1.0),
        );
    }
    paint_leaf(
        &mut pixels,
        SWATCH,
        species,
        Vec2::splat(0.5),
        0.0,
        0.86,
        1.0,
    );
    mipmapped(pixels, SWATCH * 2, SWATCH, 2, Some(FOLIAGE_CUTOFF), false)
}

fn paint_leaf(
    pixels: &mut [[u8; 4]],
    offset: usize,
    species: Species,
    center: Vec2,
    angle: f32,
    length: f32,
    light: f32,
) {
    let (sin, cos) = angle.sin_cos();
    let half = length * 0.54;
    let min = ((center - half) * SWATCH as f32)
        .floor()
        .max(Vec2::ZERO)
        .as_uvec2();
    let max = ((center + half) * SWATCH as f32)
        .ceil()
        .min(Vec2::splat((SWATCH - 1) as f32))
        .as_uvec2();
    for y in min.y..=max.y {
        for x in min.x..=max.x {
            let uv = (Vec2::new(x as f32, y as f32) + 0.5) / SWATCH as f32 - center;
            let lateral = (uv.x * cos + uv.y * sin) / length;
            let along = (-uv.x * sin + uv.y * cos) / length + 0.5;
            if !(0.0..=1.0).contains(&along) {
                continue;
            }
            let envelope = (std::f32::consts::PI * along).sin().max(0.0);
            let width = match species {
                Species::Oak => envelope.powf(0.72) * (0.25 + 0.047 * (along * 34.0).cos()),
                Species::Pine => envelope * (0.035 + 0.15 * (along * 50.0).sin().abs()),
                Species::Acacia => {
                    envelope * (0.035 + 0.22 * (along * 31.0 + lateral * 3.0).cos().max(0.0))
                }
                Species::Jungle => envelope.powf(0.82) * (0.32 - 0.11 * along),
                Species::Palm => {
                    envelope * (0.05 + 0.24 * (along * 48.0 - lateral.abs() * 6.0).sin().abs())
                }
            };
            let alpha = ((width - lateral.abs()) * length * SWATCH as f32 + 0.5).clamp(0.0, 1.0);
            if alpha == 0.0 {
                continue;
            }
            let vein = (1.0 - (lateral.abs() * 95.0).min(1.0)) * 0.25;
            let ribs = ((along * 33.0 + lateral.abs() * 30.0).cos().max(0.0)).powi(13) * 0.085;
            let lamina = (0.82 + 0.26 * lateral - 0.18 * along + vein + ribs) * light;
            let shade = (255.0 * lamina.clamp(0.0, 1.0)).round() as u8;
            let index = y as usize * SWATCH * 2 + offset + x as usize;
            let opacity = (alpha * 255.0).round() as u8;
            if opacity >= pixels[index][3] {
                pixels[index] = [shade, shade, shade, opacity];
            }
        }
    }
}

pub(super) fn bark_texture() -> Image {
    const SIDE: usize = 128;
    let mut pixels = Vec::with_capacity(SIDE * SIDE);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let u = x as f32 / SIDE as f32 * std::f32::consts::TAU;
            let v = y as f32 / SIDE as f32 * std::f32::consts::TAU;
            let warp = 0.42 * (v * 2.0 + u * 3.0).sin() + 0.23 * (v * 5.0 - u * 2.0).sin();
            let ridges = 0.5 + 0.5 * (u * 15.0 + warp + 0.9 * (u * 6.0).sin()).sin();
            let fissures = ridges.powi(7);
            let grain = 0.5 + 0.5 * (u * 41.0 + v * 9.0 + warp * 2.5).sin();
            let flecks = (u * 29.0 - v * 21.0).cos() * (u * 17.0 + v * 35.0).sin();
            let tint = (0.80 - fissures * 0.37 + grain * 0.13 + flecks * 0.06).clamp(0.0, 1.0);
            let gray = (tint * 255.0) as u8;
            pixels.push([gray, gray, gray, 255]);
        }
    }
    mipmapped(pixels, SIDE, SIDE, 1, None, true)
}

pub(super) fn import_mipmapped(
    images: &mut Assets<Image>,
    source: &Handle<Image>,
    cutoff: Option<f32>,
    repeat: bool,
) -> Handle<Image> {
    let Some(image) = images.get(source) else {
        return source.clone();
    };
    let size = image.texture_descriptor.size;
    if image.texture_descriptor.format != TextureFormat::Rgba8UnormSrgb
        || image.texture_descriptor.mip_level_count != 1
        || size.depth_or_array_layers != 1
    {
        return source.clone();
    }
    let Some(source_bytes) = image.data.as_ref() else {
        return source.clone();
    };
    let pixels = source_bytes
        .chunks_exact(4)
        .map(|p| [p[0], p[1], p[2], p[3]])
        .collect();
    images.add(mipmapped(
        pixels,
        size.width as usize,
        size.height as usize,
        1,
        cutoff,
        repeat,
    ))
}

pub(super) fn mipmapped(
    mut pixels: Vec<[u8; 4]>,
    width: usize,
    height: usize,
    tiles: usize,
    alpha_cutoff: Option<f32>,
    repeat: bool,
) -> Image {
    let mut data = Vec::new();
    let mut levels = 0;
    let (mut w, mut h) = (width, height);
    loop {
        data.extend(pixels.iter().flatten().copied());
        levels += 1;
        if w / tiles <= 4 || h <= 4 {
            break;
        }
        let mut next = Vec::with_capacity(w * h / 4);
        for y in (0..h - 1).step_by(2) {
            for x in (0..w - 1).step_by(2) {
                let block = [
                    pixels[y * w + x],
                    pixels[y * w + x + 1],
                    pixels[(y + 1) * w + x],
                    pixels[(y + 1) * w + x + 1],
                ];
                let opacity: u32 = block.iter().map(|pixel| u32::from(pixel[3])).sum();
                let mut pixel = block[0];
                if opacity > 0 {
                    for channel in 0..3 {
                        pixel[channel] = (block
                            .iter()
                            .map(|p| u32::from(p[channel]) * u32::from(p[3]))
                            .sum::<u32>()
                            / opacity) as u8;
                    }
                }
                pixel[3] = (opacity / 4) as u8;
                next.push(pixel);
            }
        }
        if let Some(cutoff) = alpha_cutoff {
            for tile in 0..tiles {
                preserve_coverage(&pixels, &mut next, w, tile, tiles, cutoff);
            }
        }
        pixels = next;
        w /= 2;
        h /= 2;
    }
    let size = Extent3d {
        width: width as u32,
        height: height as u32,
        depth_or_array_layers: 1,
    };
    let mut image = Image::new_fill(
        size,
        TextureDimension::D2,
        &[0; 4],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    let address = if repeat {
        ImageAddressMode::Repeat
    } else {
        ImageAddressMode::ClampToEdge
    };
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: address,
        address_mode_v: address,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

fn preserve_coverage(
    previous: &[[u8; 4]],
    next: &mut [[u8; 4]],
    width: usize,
    tile: usize,
    tiles: usize,
    cutoff: f32,
) {
    let cutoff = (cutoff * 255.0).ceil() as u8;
    let side = width / tiles;
    let visible = previous
        .chunks_exact(width)
        .flat_map(|row| &row[tile * side..(tile + 1) * side])
        .filter(|pixel| pixel[3] >= cutoff)
        .count();
    let target = (visible + 2) / 4;
    if target == 0 {
        return;
    }
    let half_side = side / 2;
    let crop = tile * half_side..(tile + 1) * half_side;
    let mut histogram = [0_usize; 256];
    for row in next.chunks_exact(width / 2) {
        for pixel in &row[crop.clone()] {
            histogram[pixel[3] as usize] += 1;
        }
    }
    let mut count = 0;
    let alpha = (1..=255)
        .rev()
        .find(|&alpha| {
            count += histogram[alpha];
            count >= target
        })
        .unwrap_or(1);
    let scale = (f32::from(cutoff) / alpha as f32).clamp(0.70, 1.65);
    for row in next.chunks_exact_mut(width / 2) {
        for pixel in &mut row[crop.clone()] {
            pixel[3] = (f32::from(pixel[3]) * scale).round().min(255.0) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_atlas_contains_cutouts_and_complete_mipmaps() {
        for species in Species::ALL {
            let image = canopy_texture(species);
            let texture = &image.texture_descriptor;
            let pixels = (texture.size.width * texture.size.height) as usize;
            let opaque = image.data.as_ref().unwrap()[..pixels * 4]
                .chunks_exact(4)
                .filter(|p| p[3] >= 100)
                .count();
            assert!(
                (pixels / 12..pixels * 3 / 4).contains(&opaque),
                "{species:?}: {opaque}/{pixels}"
            );
            let mip_bytes = (0..texture.mip_level_count)
                .map(|level| {
                    ((texture.size.width >> level) * (texture.size.height >> level) * 4) as usize
                })
                .sum::<usize>();
            assert_eq!(image.data.as_ref().unwrap().len(), mip_bytes);
        }
    }

    #[test]
    fn impostor_mips_filter_linear_albedo_and_folded_normals() {
        use super::super::impostor::{decode_normal, encode_normal};

        let side = 12;
        let mut colors = Vec::new();
        let mut normals = Vec::new();
        for y in 0..side {
            for x in 0..side {
                let gray = if x % 2 == 0 { 0 } else { 255 };
                colors.push([gray, gray, gray, 255]);
                let directions = [-0.05, 0.05];
                let normal = Vec3::new(directions[x % 2], directions[y % 2], -1.0);
                let [u, v] = encode_normal(normal);
                normals.push([u, v, 46, 128]);
            }
        }

        let (colors, normals, levels) = impostor_mips(colors, normals, side, side, 1);
        assert_eq!(levels, 2);
        let mip = side * side * 4;
        assert_eq!(&colors[mip..mip + 3], &[188; 3]);
        let normal = decode_normal([normals[mip], normals[mip + 1]]);
        assert!(normal.z < -0.99, "{normal:?}");
    }
}

/// Build matching, tile-isolated albedo and normal mip chains from one view.
pub(super) fn impostor_mips(
    mut colors: Vec<[u8; 4]>,
    mut normals: Vec<[u8; 4]>,
    mut width: usize,
    mut height: usize,
    tiles: usize,
) -> (Vec<u8>, Vec<u8>, u32) {
    let mut color_data = Vec::new();
    let mut normal_data = Vec::new();
    let mut levels = 0;
    loop {
        let tile_width = width / tiles;
        for y in 0..height {
            for x in 0..width {
                let index = y * width + x;
                let mut color = colors[index];
                let mut normal = normals[index];
                if color[3] == 0 {
                    // Extend edges into empty texels so filtering doesn't mix the
                    // visible silhouette with black or a neighboring tile.
                    let neighbors = [
                        (x % tile_width > 0, index.wrapping_sub(1)),
                        (x % tile_width + 1 < tile_width, index + 1),
                        (y > 0, index.wrapping_sub(width)),
                        (y + 1 < height, index + width),
                    ];
                    if let Some((_, adjacent)) = neighbors
                        .into_iter()
                        .find(|&(valid, adjacent)| valid && colors[adjacent][3] > 0)
                    {
                        color = colors[adjacent];
                        color[3] = 0;
                        normal = normals[adjacent];
                    }
                }
                color_data.extend(color);
                normal_data.extend(normal);
            }
        }
        levels += 1;
        if tile_width <= 6 {
            break;
        }
        let mut next_colors = Vec::with_capacity(colors.len() / 4);
        let mut next_normals = Vec::with_capacity(normals.len() / 4);
        for y in (0..height).step_by(2) {
            for x in (0..width).step_by(2) {
                let i = y * width + x;
                let block = [i, i + 1, i + width, i + width + 1];
                let weights = block.map(|i| u32::from(colors[i][3]));
                let sum = weights.iter().sum::<u32>();
                if sum == 0 {
                    next_colors.push([0; 4]);
                    next_normals.push([0; 4]);
                    continue;
                }
                let mut albedo = Vec3::ZERO;
                let mut direction = Vec3::ZERO;
                for (index, weight) in block.into_iter().zip(weights) {
                    let [red, green, blue, _] = colors[index];
                    let [x, y, _, _] = normals[index];
                    let weight = weight as f32 / sum as f32;
                    albedo += LinearRgba::from(Srgba::rgb_u8(red, green, blue)).to_vec3() * weight;
                    direction += super::impostor::decode_normal([x, y]) * weight;
                }
                let color = Srgba::from(LinearRgba::rgb(albedo.x, albedo.y, albedo.z));
                let byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
                next_colors.push([
                    byte(color.red),
                    byte(color.green),
                    byte(color.blue),
                    (sum / 4) as u8,
                ]);
                // Octahedral coordinates wrap at -Z. Average directions in
                // 3D so texels on opposite sides of the fold keep that normal.
                let [x, y] = if direction.length_squared() > 1e-4 {
                    super::impostor::encode_normal(direction)
                } else {
                    let index = block.into_iter().max_by_key(|&i| colors[i][3]).unwrap();
                    [normals[index][0], normals[index][1]]
                };
                let average = |channel| {
                    (block
                        .into_iter()
                        .zip(weights)
                        .map(|(index, weight)| u32::from(normals[index][channel]) * weight)
                        .sum::<u32>()
                        / sum) as u8
                };
                next_normals.push([x, y, average(2), average(3)]);
            }
        }
        for tile in 0..tiles {
            preserve_coverage(&colors, &mut next_colors, width, tile, tiles, 0.46);
        }
        colors = next_colors;
        normals = next_normals;
        width /= 2;
        height /= 2;
    }
    (color_data, normal_data, levels)
}
