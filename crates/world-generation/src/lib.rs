#![warn(missing_docs)]
//! Deterministic baking of the planet's heightmaps, watersheds, rivers, and water masks.

mod river_generation;

use image::{ImageBuffer, Luma};
use noise::{NoiseFn, Perlin};
use std::{collections::VecDeque, error::Error, fs, path::Path};
use world_core::{
    coordinates::source_direction as sphere_direction,
    planet::{
        encode_height, HEIGHT_MAP_SIZE, HEIGHT_SOURCE_PATH, RIVER_ATLAS_PATH, RIVER_SURFACE_PATH,
        SEA_LEVEL,
    },
};

const FACE_COUNT: usize = 6;
const SIZE: usize = HEIGHT_MAP_SIZE as usize;

// Взято из cube-sphere topology bevy_terrain.
// index:
// 0 = same face
// 1 = x < 0
// 2 = y < 0
// 3 = x >= size
// 4 = y >= size
const NEIGHBOURING_SIDES: [[usize; 5]; 6] = [
    [0, 4, 2, 1, 5],
    [1, 0, 2, 3, 5],
    [2, 0, 4, 3, 1],
    [3, 2, 4, 5, 1],
    [4, 2, 0, 5, 3],
    [5, 4, 0, 1, 3],
];

#[derive(Clone, Copy)]
enum SideInfo {
    Fixed0,
    Fixed1,
    PositiveS,
    PositiveT,
}

impl SideInfo {
    const EVEN_LIST: [[SideInfo; 2]; 6] = [
        [SideInfo::PositiveS, SideInfo::PositiveT],
        [SideInfo::Fixed0, SideInfo::PositiveT],
        [SideInfo::Fixed0, SideInfo::PositiveS],
        [SideInfo::PositiveT, SideInfo::PositiveS],
        [SideInfo::PositiveT, SideInfo::Fixed0],
        [SideInfo::PositiveS, SideInfo::Fixed0],
    ];

    const ODD_LIST: [[SideInfo; 2]; 6] = [
        [SideInfo::PositiveS, SideInfo::PositiveT],
        [SideInfo::PositiveS, SideInfo::Fixed1],
        [SideInfo::PositiveT, SideInfo::Fixed1],
        [SideInfo::PositiveT, SideInfo::PositiveS],
        [SideInfo::Fixed1, SideInfo::PositiveS],
        [SideInfo::Fixed1, SideInfo::PositiveT],
    ];

    fn project_to_side(side: usize, other_side: usize) -> [SideInfo; 2] {
        let index = (6 + other_side - side) % 6;

        if side % 2 == 0 {
            Self::EVEN_LIST[index]
        } else {
            Self::ODD_LIST[index]
        }
    }
}

/// Bake the existing planet into the given assets directory.
///
/// Files at the generated paths are overwritten. Seeds, cube topology, and
/// on-disk asset formats are part of the existing world definition.
pub fn generate_world(assets: &Path) -> Result<(), Box<dyn Error>> {
    let height_output = assets.join(HEIGHT_SOURCE_PATH);
    let ocean_output = assets.join("world/ocean");
    let basin_output = assets.join("world/basins");

    fs::create_dir_all(&height_output)?;
    fs::create_dir_all(&ocean_output)?;
    fs::create_dir_all(&basin_output)?;

    let continents = Perlin::new(73);
    let detail = Perlin::new(2531);
    let mountains = Perlin::new(12097);
    let archipelagos = Perlin::new(27109);
    let lakes = Perlin::new(8431);

    let pixels_per_face = SIZE * SIZE;
    let total_pixels = FACE_COUNT * pixels_per_face;

    let mut heights = vec![0.0_f64; total_pixels];
    let mut encoded = vec![0_u16; total_pixels];

    println!("Generating terrain...");
    println!("6 faces × {SIZE} × {SIZE}");

    for face in 0..FACE_COUNT {
        println!("  generating face{face}");

        for y in 0..SIZE {
            for x in 0..SIZE {
                let u = x as f64 / (SIZE - 1) as f64;
                let v = y as f64 / (SIZE - 1) as f64;

                let direction = sphere_direction(face, u, v);

                let continental = fbm(&continents, direction, 0.75, 5, 2.0, 0.52);
                let regional = fbm(&detail, direction, 3.0, 5, 2.05, 0.50);
                let coast = fbm(&detail, direction, 24.0, 4, 2.0, 0.55);
                let continent = continental * 0.82 + regional * 0.18;
                let chain_noise = fbm(&archipelagos, direction, 4.9, 3, 2.0, 0.5);
                let chain = (1.0 - chain_noise.abs() * 2.6).max(0.0).powi(3);
                let islands = fbm(&archipelagos, direction, 19.0, 4, 2.0, 0.54);
                let island_rise = chain * smoothstep(0.05, 0.48, islands) * 0.48;
                let shore = continent - 0.028 + coast * 0.075 + island_rise;

                let elevation = if shore <= 0.0 {
                    -shore.abs() * 850.0 - (shore.abs() / 0.7).powf(1.65) * 9_500.0
                } else {
                    let inland = smoothstep(0.008, 0.09, shore);
                    let mountain_noise = fbm(&mountains, direction, 4.5, 3, 2.0, 0.52);
                    let mountain_belt = smoothstep(0.05, 0.49, mountain_noise);
                    let ridge = ridge_height(&mountains, direction);
                    let upland = smoothstep(0.055, 0.29, shore);
                    let mountain_height = mountain_belt * upland * ridge * 9_000.0;
                    let relief = fbm(&detail, direction, 14.0, 6, 2.0, 0.53);
                    let ground = (shore * 3_600.0 + inland * (relief * 850.0 + 250.0)).max(8.0)
                        + mountain_height;

                    let lake_noise = fbm(&lakes, direction, 17.0, 4, 2.1, 0.48);
                    let lowland =
                        smoothstep(0.045, 0.075, shore) * (1.0 - smoothstep(0.15, 0.27, shore));
                    let depression = smoothstep(0.16, 0.37, lake_noise)
                        * lowland
                        * (1.0 - smoothstep(0.25, 0.65, mountain_belt));
                    ground * (1.0 - depression) - depression * 220.0
                };

                let index = index(face, x, y);

                heights[index] = elevation;
                encoded[index] = encode_height(elevation);
            }
        }
    }

    let river_output = assets.join(RIVER_ATLAS_PATH);
    let river_surface = assets.join(RIVER_SURFACE_PATH);
    river_generation::generate(&mut heights, &mut encoded, &river_output, &river_surface)?;
    save_heightmaps(&encoded, &height_output)?;

    println!();
    println!("Finding connected areas below sea level...");

    let mut labels = vec![0_u32; total_pixels];

    let mut next_label = 1_u32;
    let mut largest_label = 0_u32;
    let mut largest_size = 0_usize;

    let mut component_count = 0_usize;
    let mut below_sea_count = 0_usize;

    for i in 0..total_pixels {
        if heights[i] >= SEA_LEVEL {
            continue;
        }

        below_sea_count += 1;

        if labels[i] != 0 {
            continue;
        }

        let component_size = flood_fill_component(i, next_label, &heights, &mut labels);

        component_count += 1;

        if component_size > largest_size {
            largest_size = component_size;
            largest_label = next_label;
        }

        next_label += 1;
    }

    println!("  below sea components: {component_count}");
    println!("  largest component:    {largest_size} cells");

    let mut ocean_mask = vec![false; total_pixels];
    let mut ocean_count = 0_usize;
    let mut basin_count = 0_usize;

    for i in 0..total_pixels {
        if largest_label != 0 && labels[i] == largest_label {
            ocean_mask[i] = true;
            ocean_count += 1;
        } else if heights[i] < SEA_LEVEL {
            basin_count += 1;
        }
    }

    save_masks(&ocean_mask, &heights, &ocean_output, &basin_output)?;

    println!();
    println!("WORLD SUMMARY");
    println!("-------------");

    println!(
        "below sea level: {:>6.2}%",
        below_sea_count as f64 / total_pixels as f64 * 100.0
    );

    println!(
        "ocean:           {:>6.2}%",
        ocean_count as f64 / total_pixels as f64 * 100.0
    );

    println!(
        "inland lakes:   {:>6.2}%",
        basin_count as f64 / total_pixels as f64 * 100.0
    );

    println!(
        "land >=0m:       {:>6.2}%",
        (total_pixels - below_sea_count) as f64 / total_pixels as f64 * 100.0
    );

    println!();
    println!("Generated:");
    println!("  heightmaps -> {}", height_output.display());
    println!("  ocean mask -> {}", ocean_output.display());
    println!("  lake masks -> {}", basin_output.display());
    println!(
        "  water atlas -> {}",
        assets.join(world_core::planet::OCEAN_ATLAS_PATH).display()
    );
    println!("  river atlas -> {}", river_output.display());
    println!("  river surface -> {}", river_surface.display());

    Ok(())
}

fn flood_fill_component(start: usize, label: u32, heights: &[f64], labels: &mut [u32]) -> usize {
    let mut queue = VecDeque::new();

    labels[start] = label;
    queue.push_back(start);

    let mut count = 0;

    while let Some(current) = queue.pop_front() {
        count += 1;

        let (face, x, y) = decode_index(current);

        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let (nf, nx, ny) = neighbour(face, x, y, dx, dy);

            let neighbour_index = index(nf, nx, ny);

            if labels[neighbour_index] != 0 {
                continue;
            }

            if heights[neighbour_index] >= SEA_LEVEL {
                continue;
            }

            labels[neighbour_index] = label;
            queue.push_back(neighbour_index);
        }
    }

    count
}

fn neighbour(face: usize, x: usize, y: usize, dx: i32, dy: i32) -> (usize, usize, usize) {
    let nx = x as i32 + dx;
    let ny = y as i32 + dy;

    if nx >= 0 && nx < SIZE as i32 && ny >= 0 && ny < SIZE as i32 {
        return (face, nx as usize, ny as usize);
    }

    let edge_index = if nx < 0 {
        1
    } else if ny < 0 {
        2
    } else if nx >= SIZE as i32 {
        3
    } else {
        4
    };

    let clamped_x = nx.clamp(0, SIZE as i32 - 1) as usize;

    let clamped_y = ny.clamp(0, SIZE as i32 - 1) as usize;

    let neighbour_face = NEIGHBOURING_SIDES[face][edge_index];

    let info = SideInfo::project_to_side(face, neighbour_face);

    let projected = info.map(|info| match info {
        SideInfo::Fixed0 => 0,
        SideInfo::Fixed1 => SIZE - 1,
        SideInfo::PositiveS => clamped_x,
        SideInfo::PositiveT => clamped_y,
    });

    (neighbour_face, projected[0], projected[1])
}

fn save_heightmaps(encoded: &[u16], output: &Path) -> Result<(), Box<dyn Error>> {
    for face in 0..FACE_COUNT {
        let start = face * SIZE * SIZE;
        let end = start + SIZE * SIZE;

        let image = ImageBuffer::<Luma<u16>, Vec<u16>>::from_raw(
            SIZE as u32,
            SIZE as u32,
            encoded[start..end].to_vec(),
        )
        .expect("invalid height image");

        image.save(output.join(format!("face{face}.tif")))?;
    }

    Ok(())
}

fn save_masks(
    ocean_mask: &[bool],
    heights: &[f64],
    ocean_output: &Path,
    basin_output: &Path,
) -> Result<(), Box<dyn Error>> {
    let side = SIZE as u32;
    let mut atlas = ImageBuffer::<Luma<u8>, Vec<u8>>::new(side * 3, side * 2);

    for face in 0..FACE_COUNT {
        let mut ocean = ImageBuffer::<Luma<u8>, Vec<u8>>::new(side, side);
        let mut lakes = ImageBuffer::<Luma<u8>, Vec<u8>>::new(side, side);
        let atlas_x = (face as u32 % 3) * side;
        let atlas_y = (face as u32 / 3) * side;

        for y in 0..side {
            for x in 0..side {
                let i = index(face, x as usize, y as usize);

                if ocean_mask[i] {
                    ocean.put_pixel(x, y, Luma([255]));
                    atlas.put_pixel(atlas_x + x, atlas_y + y, Luma([255]));
                } else if heights[i] < SEA_LEVEL {
                    lakes.put_pixel(x, y, Luma([255]));
                    atlas.put_pixel(atlas_x + x, atlas_y + y, Luma([192]));
                }
            }
        }

        ocean.save(ocean_output.join(format!("face{face}.png")))?;
        lakes.save(basin_output.join(format!("face{face}.png")))?;
    }

    atlas.save(ocean_output.with_file_name("ocean_atlas.png"))?;

    Ok(())
}

fn index(face: usize, x: usize, y: usize) -> usize {
    face * SIZE * SIZE + y * SIZE + x
}

fn decode_index(index: usize) -> (usize, usize, usize) {
    let pixels_per_face = SIZE * SIZE;

    let face = index / pixels_per_face;
    let local = index % pixels_per_face;

    let y = local / SIZE;
    let x = local % SIZE;

    (face, x, y)
}

fn fbm(
    noise: &Perlin,
    direction: [f64; 3],
    frequency: f64,
    octaves: usize,
    lacunarity: f64,
    persistence: f64,
) -> f64 {
    let mut frequency = frequency;
    let mut amplitude = 1.0;
    let mut value = 0.0;
    let mut weight = 0.0;

    for _ in 0..octaves {
        value += noise.get([
            direction[0] * frequency,
            direction[1] * frequency,
            direction[2] * frequency,
        ]) * amplitude;

        weight += amplitude;
        frequency *= lacunarity;
        amplitude *= persistence;
    }

    value / weight
}

fn ridge_height(noise: &Perlin, direction: [f64; 3]) -> f64 {
    let mut frequency = 9.0;
    let mut amplitude = 0.56;
    let mut height = 0.0;

    for _ in 0..6 {
        let value = noise.get(direction.map(|coordinate| coordinate * frequency));
        let ridge = (1.0 - value.abs()).powi(4);
        height += ridge * amplitude;
        frequency *= 2.12;
        amplitude *= 0.47;
    }

    height
}

fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);

    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use bevy_math::DVec3;

    use super::*;

    #[test]
    fn drainage_neighbours_match_cube_face_seams() {
        let last = SIZE - 1;
        let offset = SIZE / 3;
        let direction = |face, x, y| {
            DVec3::from_array(sphere_direction(
                face,
                x as f64 / last as f64,
                y as f64 / last as f64,
            ))
        };
        for face in 0..FACE_COUNT {
            for (x, y, dx, dy) in [
                (0, offset, -1, 0),
                (last, offset, 1, 0),
                (offset, 0, 0, -1),
                (offset, last, 0, 1),
            ] {
                let (other, nx, ny) = neighbour(face, x, y, dx, dy);
                assert_ne!(face, other);
                assert!(direction(face, x, y).abs_diff_eq(direction(other, nx, ny), 1e-12));
            }
        }
    }
}
