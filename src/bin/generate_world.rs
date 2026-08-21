use image::{ImageBuffer, Luma};
use noise::{NoiseFn, Perlin};
use std::{
    collections::VecDeque,
    error::Error,
    fs,
    path::Path,
};

const FACE_COUNT: usize = 6;
const SIZE: usize = 1024;

const MIN_HEIGHT: f64 = -12_000.0;
const MAX_HEIGHT: f64 = 9_000.0;
const SEA_LEVEL: f64 = 0.0;

const C_SQR: f64 = 0.87 * 0.87;

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

fn main() -> Result<(), Box<dyn Error>> {
    let height_output =
        Path::new("assets/terrains/spherical/source/height");

    let ocean_output =
        Path::new("assets/world/ocean");

    let basin_output =
        Path::new("assets/world/basins");

    fs::create_dir_all(height_output)?;
    fs::create_dir_all(ocean_output)?;
    fs::create_dir_all(basin_output)?;

    let continents = Perlin::new(42);
    let detail = Perlin::new(1337);
    let mountains = Perlin::new(9001);

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

                let continental = fbm(
                    &continents,
                    direction,
                    0.75,
                    5,
                    2.0,
                    0.52,
                );

                let regional = fbm(
                    &detail,
                    direction,
                    3.0,
                    5,
                    2.05,
                    0.50,
                );

                let ridge_noise = fbm(
                    &mountains,
                    direction,
                    2.2,
                    5,
                    2.1,
                    0.52,
                );

                let ridges =
                    (1.0 - ridge_noise.abs()).powf(3.2);

                let continent =
                    continental * 0.82 + regional * 0.18;

                // Это не уровень воды.
                // Это только threshold генератора материков.
                let continent_threshold = 0.02;

                let elevation =
                    if continent <= continent_threshold {
                        let depth = (
                            (continent_threshold - continent) / 0.75
                        )
                            .clamp(0.0, 1.0);

                        -150.0 - depth.powf(1.65) * 9_500.0
                    } else {
                        let land = (
                            (continent - continent_threshold) / 0.65
                        )
                            .clamp(0.0, 1.0);

                        let base = 80.0
                            + land.powf(1.4) * 2_100.0
                            + regional.max(0.0) * 650.0;

                        let inland_mask =
                            smoothstep(0.06, 0.38, land);

                        let mountain_height =
                            ridges * inland_mask * 6_500.0;

                        base + mountain_height
                    };

                let index = index(face, x, y);

                heights[index] = elevation;
                encoded[index] = encode_height(elevation);
            }
        }
    }

    save_heightmaps(&encoded, height_output)?;

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

        let component_size = flood_fill_component(
            i,
            next_label,
            &heights,
            &mut labels,
        );

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
        if labels[i] == largest_label {
            ocean_mask[i] = true;
            ocean_count += 1;
        } else if heights[i] < SEA_LEVEL {
            basin_count += 1;
        }
    }

    save_masks(
        &ocean_mask,
        &heights,
        ocean_output,
        basin_output,
    )?;

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
        "dry basins <0m:  {:>6.2}%",
        basin_count as f64 / total_pixels as f64 * 100.0
    );

    println!(
        "land >=0m:       {:>6.2}%",
        (total_pixels - below_sea_count) as f64
            / total_pixels as f64
            * 100.0
    );

    println!();
    println!("Generated:");
    println!("  heightmaps -> assets/terrains/spherical/source/height/");
    println!("  ocean mask -> assets/world/ocean/");
    println!("  basin mask -> assets/world/basins/");

    Ok(())
}

fn flood_fill_component(
    start: usize,
    label: u32,
    heights: &[f64],
    labels: &mut [u32],
) -> usize {
    let mut queue = VecDeque::new();

    labels[start] = label;
    queue.push_back(start);

    let mut count = 0;

    while let Some(current) = queue.pop_front() {
        count += 1;

        let (face, x, y) = decode_index(current);

        for (dx, dy) in [
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
        ] {
            let (nf, nx, ny) =
                neighbour(face, x, y, dx, dy);

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

fn neighbour(
    face: usize,
    x: usize,
    y: usize,
    dx: i32,
    dy: i32,
) -> (usize, usize, usize) {
    let nx = x as i32 + dx;
    let ny = y as i32 + dy;

    if nx >= 0
        && nx < SIZE as i32
        && ny >= 0
        && ny < SIZE as i32
    {
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

    let clamped_x =
        nx.clamp(0, SIZE as i32 - 1) as usize;

    let clamped_y =
        ny.clamp(0, SIZE as i32 - 1) as usize;

    let neighbour_face =
        NEIGHBOURING_SIDES[face][edge_index];

    let info =
        SideInfo::project_to_side(face, neighbour_face);

    let projected = info.map(|info| match info {
        SideInfo::Fixed0 => 0,
        SideInfo::Fixed1 => SIZE - 1,
        SideInfo::PositiveS => clamped_x,
        SideInfo::PositiveT => clamped_y,
    });

    (
        neighbour_face,
        projected[0],
        projected[1],
    )
}

fn save_heightmaps(
    encoded: &[u16],
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    for face in 0..FACE_COUNT {
        let start = face * SIZE * SIZE;
        let end = start + SIZE * SIZE;

        let image =
            ImageBuffer::<Luma<u16>, Vec<u16>>::from_raw(
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
    for face in 0..FACE_COUNT {
        let mut ocean =
            ImageBuffer::<Luma<u8>, Vec<u8>>::new(
                SIZE as u32,
                SIZE as u32,
            );

        let mut basins =
            ImageBuffer::<Luma<u8>, Vec<u8>>::new(
                SIZE as u32,
                SIZE as u32,
            );

        for y in 0..SIZE {
            for x in 0..SIZE {
                let i = index(face, x, y);

                ocean.put_pixel(
                    x as u32,
                    y as u32,
                    Luma([
                        if ocean_mask[i] { 255 } else { 0 }
                    ]),
                );

                let is_dry_basin =
                    heights[i] < SEA_LEVEL
                        && !ocean_mask[i];

                basins.put_pixel(
                    x as u32,
                    y as u32,
                    Luma([
                        if is_dry_basin { 255 } else { 0 }
                    ]),
                );
            }
        }

        ocean.save(
            ocean_output.join(format!("face{face}.png"))
        )?;

        basins.save(
            basin_output.join(format!("face{face}.png"))
        )?;
    }

    Ok(())
}

fn index(
    face: usize,
    x: usize,
    y: usize,
) -> usize {
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

fn sphere_direction(
    face: usize,
    u: f64,
    v: f64,
) -> [f64; 3] {
    let mut u = (u - 0.5) / 0.5;
    let mut v = (v - 0.5) / 0.5;

    u /= (1.0 + C_SQR - C_SQR * u * u).sqrt();
    v /= (1.0 + C_SQR - C_SQR * v * v).sqrt();

    let p = match face {
        0 => [-1.0, -v, u],
        1 => [u, -v, 1.0],
        2 => [u, 1.0, v],
        3 => [1.0, -u, v],
        4 => [v, -u, -1.0],
        5 => [v, -1.0, u],
        _ => unreachable!(),
    };

    normalize(p)
}

fn normalize(v: [f64; 3]) -> [f64; 3] {
    let len =
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
            .sqrt();

    [
        v[0] / len,
        v[1] / len,
        v[2] / len,
    ]
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

fn smoothstep(
    edge0: f64,
    edge1: f64,
    x: f64,
) -> f64 {
    let t =
        ((x - edge0) / (edge1 - edge0))
            .clamp(0.0, 1.0);

    t * t * (3.0 - 2.0 * t)
}

fn encode_height(height: f64) -> u16 {
    let height =
        height.clamp(MIN_HEIGHT, MAX_HEIGHT);

    let normalized =
        (height - MIN_HEIGHT)
            / (MAX_HEIGHT - MIN_HEIGHT);

    (normalized * u16::MAX as f64).round() as u16
}