use std::{error::Error, fs, path::Path};

use bevy_math::{DVec3, Vec3};

use image::{Rgba, RgbaImage};

use super::{decode_index, encode_height, index, neighbour, sphere_direction, SEA_LEVEL, SIZE};
use world_core::{
    coordinates::{source_face_uv as projected_uv, CUBE_CORRECTION},
    planet::{RADIUS, RIVER_REACH_VERTICES},
};

const DIRECTIONS: [(i32, i32); 8] = [
    (-1, 0),
    (1, 0),
    (0, -1),
    (0, 1),
    (-1, -1),
    (1, -1),
    (-1, 1),
    (1, 1),
];
use super::hydrology::NO_OUTLET;
const RIVER_AREA: f32 = 390.0;
const MASK_SIDE: u32 = (SIZE * 2) as u32;

pub(super) fn generate(
    heights: &mut [f64],
    encoded: &mut [u16],
    ocean: &[bool],
    output: &Path,
    surface_output: &Path,
    lake_output: &Path,
) -> Result<(), Box<dyn Error>> {
    let drainage = super::hydrology::route(heights, ocean);
    let outlets = &drainage.outlets;
    let area = &drainage.accumulation;
    let land = &drainage.order;
    // Fill shallow numerical sinks, but retain deep basins for lake water.
    // Flow topology is computed from the original terrain, before any carving.
    for &cell in land {
        let i = cell as usize;
        if !drainage.lakes[i] && heights[i] > SEA_LEVEL && drainage.spill[i] > heights[i] {
            heights[i] = drainage.spill[i];
            encoded[i] = encode_height(heights[i]);
        }
    }
    let lake_cells = drainage.lakes.iter().filter(|&&value| value).count();
    println!("Priority-flood drainage: {} routed land cells, {lake_cells} lake cells", land.len());

    let mut atlas = RgbaImage::new(3 * MASK_SIDE, 2 * MASK_SIDE);
    let mut bank_cuts = vec![0.0_f32; heights.len()];
    let mut water_levels = vec![0.0_f32; heights.len()];
    let mut surface = Vec::with_capacity(RIVER_REACH_VERTICES * 1500);
    let mut sections = 0;
    let mut mouths = 0;
    let mut seams = 0;
    for &cell in land {
        let cell = cell as usize;
        if drainage.lakes[cell] || heights[cell] <= SEA_LEVEL
            || area[cell] < RIVER_AREA || outlets[cell] == NO_OUTLET {
            continue;
        }
        let next = outlets[cell] as usize;
        let (face, x, y) = decode_index(cell);
        let (next_face, nx, ny) = decode_index(next);
        let flow = area[cell] / RIVER_AREA;
        let original_height = heights[cell];
        if x > 0 && x < SIZE - 1 && y > 0 && y < SIZE - 1 {
            let incision = (3.0 + flow.sqrt() * 2.3).min(19.0);
            let channel_floor = (heights[next].max(SEA_LEVEL) + 0.1).min(heights[cell]);
            heights[cell] = (heights[cell] - f64::from(incision)).max(channel_floor);
            encoded[cell] = encode_height(heights[cell]);
        }
        // Depression-filled routes may be flat; the downstream spill level
        // gives the physically connected outlet, not the raw neighbor height.
        debug_assert!(drainage.spill[cell] + 1e-9 >= drainage.spill[next]);

        let width = channel_width(flow);
        let from = grid_uv(x, y);
        let to = grid_uv(nx, ny);
        let bank_width = (1.55 + flow.sqrt() * 0.48).min(3.4);
        let bank_depth = ((original_height - heights[cell]) as f32) * 0.78;
        let from_direction = sphere_direction(face, from[0], from[1]);
        let to_direction = sphere_direction(next_face, to[0], to[1]);
        let downstream_water = if drainage.lakes[next] {
            drainage.spill[next]
        } else if water_levels[next] > 0.0 {
            f64::from(water_levels[next])
        } else {
            heights[next].max(SEA_LEVEL)
        };
        let cut = original_height - heights[cell];
        let water_depth = (0.45 + f64::from(flow.sqrt()) * 0.40)
            .min(3.5)
            .min((cut * 0.72).max(0.23));
        let water = (heights[cell] + water_depth).max(downstream_water + 0.01);
        water_levels[cell] = water as f32;
        debug_assert!(water > downstream_water);
        let from_up = DVec3::from_array(from_direction);
        let to_up = DVec3::from_array(to_direction);
        let to_on_face = if face == next_face {
            to
        } else {
            projected_uv(face, to_direction)
        };
        let distance_pixels =
            (to_on_face[0] - from[0]).hypot(to_on_face[1] - from[1]) * f64::from(MASK_SIDE - 1);
        let meters_per_pixel = if distance_pixels > 0.0 {
            RADIUS * from_up.angle_between(to_up) / distance_pixels
        } else {
            65.0
        };
        let from_width = meters_per_pixel * f64::from(width);
        let to_width = if heights[next] > SEA_LEVEL {
            meters_per_pixel * f64::from(channel_width(area[next] / RIVER_AREA))
        } else {
            from_width
        };
        append_surface(
            &mut surface,
            from_up,
            to_up,
            water,
            downstream_water,
            from_width,
            to_width,
        );
        if face == next_face {
            draw_section(&mut atlas, face, from, to, width);
            taper_banks(&mut bank_cuts, face, from, to, bank_width, bank_depth);
        } else {
            let from_on_next_face = projected_uv(next_face, from_direction);
            draw_section(&mut atlas, face, from, to_on_face, width);
            draw_section(&mut atlas, next_face, from_on_next_face, to, width);
            taper_banks(
                &mut bank_cuts,
                face,
                from,
                to_on_face,
                bank_width,
                bank_depth,
            );
            taper_banks(
                &mut bank_cuts,
                next_face,
                from_on_next_face,
                to,
                bank_width,
                bank_depth,
            );
            seams += 1;
        }
        sections += 1;
        mouths += usize::from(ocean[next] || drainage.lakes[next]);
    }
    let mut banks = 0;
    for (cell, cut) in bank_cuts.into_iter().enumerate() {
        if cut == 0.0
            || heights[cell] <= SEA_LEVEL
            || (area[cell] >= RIVER_AREA && outlets[cell] != NO_OUTLET)
        {
            continue;
        }
        let available = (heights[cell] - SEA_LEVEL - 0.35).max(0.0);
        let old = encoded[cell];
        heights[cell] -= f64::from(cut).min(available);
        encoded[cell] = encode_height(heights[cell]);
        banks += usize::from(encoded[cell] < old);
    }
    let diagonal_cuts = open_diagonal_channels(heights, encoded, land, outlets, area, &drainage.lakes);
    // Lake surfaces are separate from the river ribbons: each depression
    // uses its actual priority-flood spill level rather than global sea level.
    // Do not carve these cells back to sea level; their terrain remains below
    // the water surface until its natural outlet is reached.
    let mut lakes = Vec::new();
    let mut lake_cells = 0usize;
    for (cell, &is_lake) in drainage.lakes.iter().enumerate() {
        if !is_lake { continue; }
        let (face, x, y) = decode_index(cell);
        let last = (SIZE - 1) as f64;
        let ux = x as f64;
        let uy = y as f64;
        let left = sphere_direction(face, ((ux - 0.5).max(0.0)) / last, uy / last);
        let right = sphere_direction(face, ((ux + 0.5).min(last)) / last, uy / last);
        let bottom = sphere_direction(face, ux / last, ((uy - 0.5).max(0.0)) / last);
        let top = sphere_direction(face, ux / last, ((uy + 0.5).min(last)) / last);
        let from = DVec3::from_array(left);
        let to = DVec3::from_array(right);
        let width = 0.5 * RADIUS * DVec3::from_array(bottom).angle_between(DVec3::from_array(top));
        if width <= 0.0 || from.distance_squared(to) <= 1.0e-20 { continue; }
        let elevation = drainage.spill[cell] + 0.10;
        append_surface(&mut lakes, from, to, elevation, elevation, width, width);
        lake_cells += 1;
    }
    let lake_bytes: Vec<_> = lakes.iter()
        .flat_map(|p| p.to_array().into_iter().flat_map(f32::to_le_bytes))
        .collect();
    fs::write(lake_output, lake_bytes)?;
    println!("Inland lake surface: {lake_cells} cells / {} triangles", lakes.len() / 3);

    println!(
        "River network: {sections} downstream reaches, {mouths} outlets, {seams} face crossings, {banks} tapered bank samples, {diagonal_cuts} diagonal channel cuts"
    );
    atlas.save(output)?;
    assert_eq!(surface.len(), sections * RIVER_REACH_VERTICES);
    println!("River surface: {} triangles", surface.len() / 3);
    let bytes: Vec<_> = surface
        .iter()
        .flat_map(|position| position.to_array().into_iter().flat_map(f32::to_le_bytes))
        .collect();
    fs::write(surface_output, bytes)?;
    Ok(())
}

fn open_diagonal_channels(
    heights: &mut [f64],
    encoded: &mut [u16],
    land: &[u32],
    outlets: &[u32],
    area: &[f32],
    lakes: &[bool],
) -> usize {
    let is_channel = |cell: usize| !lakes[cell]
        && outlets[cell] != NO_OUTLET && area[cell] >= RIVER_AREA;
    let mut smoothing = vec![0.0_f32; heights.len()];
    let mut count = 0;
    for &cell in land {
        let cell = cell as usize;
        if !is_channel(cell) {
            continue;
        }
        let next = outlets[cell] as usize;
        let (face, x, y) = decode_index(cell);
        let (next_face, nx, ny) = decode_index(next);
        if face != next_face || x == nx || y == ny {
            continue;
        }
        let sides = [index(face, x, ny), index(face, nx, y)];
        let available = sides.map(|side| {
            let (_, sx, sy) = decode_index(side);
            if is_channel(side) || sx == 0 || sy == 0 || sx == SIZE - 1 || sy == SIZE - 1 {
                0.0
            } else {
                (heights[side] - 0.35).max(0.0)
            }
        });
        let excess =
            (heights[sides[0]] + heights[sides[1]] - heights[cell] - heights[next]).max(0.0);
        let first_cut = (excess * 0.5).max(excess - available[1]).min(available[0]);
        let second_cut = (excess - first_cut).min(available[1]);
        for (side, cut) in sides.into_iter().zip([first_cut, second_cut]) {
            if cut <= 0.0 {
                continue;
            }
            let (_, sx, sy) = decode_index(side);
            let center = grid_uv(sx, sy);
            taper_banks(
                &mut smoothing,
                face,
                center,
                center,
                1.9,
                (cut * 0.50) as f32,
            );
            heights[side] -= cut;
            encoded[side] = encode_height(heights[side]);
            count += 1;
        }
    }
    for (cell, cut) in smoothing.into_iter().enumerate() {
        if cut > 0.0 && heights[cell] > 0.35 && !is_channel(cell) {
            heights[cell] -= f64::from(cut).min(heights[cell] - 0.35);
            encoded[cell] = encode_height(heights[cell]);
        }
    }
    count
}

fn channel_width(flow: f32) -> f32 {
    (0.36 + flow.sqrt() * 0.32).min(2.6)
}

fn append_surface(
    vertices: &mut Vec<Vec3>,
    from: DVec3,
    to: DVec3,
    from_height: f64,
    to_height: f64,
    from_width: f64,
    to_width: f64,
) {
    let Some(axis) = from.cross(to).try_normalize() else {
        return;
    };
    let start = from * (RADIUS + from_height);
    let end = to * (RADIUS + to_height);
    let start_left = on_surface(start - axis * from_width, from_height);
    let start_right = on_surface(start + axis * from_width, from_height);
    let end_left = on_surface(end - axis * to_width, to_height);
    let end_right = on_surface(end + axis * to_width, to_height);
    vertices.extend([
        start_left,
        end_left,
        start_right,
        start_right,
        end_left,
        end_right,
    ]);

    for sector in 0..8 {
        let point = |sector: i32| {
            let angle = f64::from(sector) * std::f64::consts::TAU / 8.0;
            let offset = DVec3::new(angle.cos(), angle.sin(), 0.0);
            let tangent = axis * offset.x + from.cross(axis) * offset.y;
            on_surface(start + tangent * from_width, from_height)
        };
        vertices.extend([start.as_vec3(), point(sector), point(sector + 1)]);
    }
}

fn on_surface(position: DVec3, height: f64) -> Vec3 {
    (position.normalize() * (RADIUS + height)).as_vec3()
}

fn cell_area(x: usize, y: usize) -> f32 {
    let [u, v] = grid_uv(x, y).map(|axis| axis as f32 * 2.0 - 1.0);
    let correction = CUBE_CORRECTION as f32;
    let sx = 1.0 + correction - correction * u * u;
    let sy = 1.0 + correction - correction * v * v;
    let jacobian = (1.0 + correction).powi(2) / (sx * sy).powf(1.5);
    let area = jacobian / (1.0 + u * u / sx + v * v / sy).powf(1.5);
    let overlap_x = if x == 0 || x == SIZE - 1 { 0.5 } else { 1.0 };
    let overlap_y = if y == 0 || y == SIZE - 1 { 0.5 } else { 1.0 };
    area * overlap_x * overlap_y
}

fn grid_uv(x: usize, y: usize) -> [f64; 2] {
    [x as f64, y as f64].map(|coordinate| coordinate / (SIZE - 1) as f64)
}

fn taper_banks(
    cuts: &mut [f32],
    face: usize,
    from: [f64; 2],
    to: [f64; 2],
    width: f32,
    depth: f32,
) {
    if depth <= 0.0 {
        return;
    }
    let last = (SIZE - 1) as f32;
    let [ax, ay] = from.map(|coordinate| coordinate as f32 * last);
    let [bx, by] = to.map(|coordinate| coordinate as f32 * last);
    let dx = bx - ax;
    let dy = by - ay;
    let length_squared = dx * dx + dy * dy;
    let min_x = (ax.min(bx) - width).floor().clamp(1.0, last - 1.0) as usize;
    let min_y = (ay.min(by) - width).floor().clamp(1.0, last - 1.0) as usize;
    let max_x = (ax.max(bx) + width).ceil().clamp(1.0, last - 1.0) as usize;
    let max_y = (ay.max(by) + width).ceil().clamp(1.0, last - 1.0) as usize;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let sx = x as f32 - ax;
            let sy = y as f32 - ay;
            let t = if length_squared > 0.0 {
                ((sx * dx + sy * dy) / length_squared).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let distance = (sx - t * dx).hypot(sy - t * dy);
            let falloff = 1.0 - (distance / width).clamp(0.0, 1.0);
            let cut = depth * falloff * falloff * (3.0 - 2.0 * falloff);
            let cell = index(face, x, y);
            cuts[cell] = cuts[cell].max(cut);
        }
    }
}

fn draw_section(atlas: &mut RgbaImage, face: usize, from: [f64; 2], to: [f64; 2], width: f32) {
    let last = (MASK_SIDE - 1) as f32;
    let [ax, ay] = from.map(|coordinate| coordinate as f32 * last);
    let [bx, by] = to.map(|coordinate| coordinate as f32 * last);
    let dx = bx - ax;
    let dy = by - ay;
    let length = dx.hypot(dy);
    if length == 0.0 {
        return;
    }
    let red = ((dx / length * 0.5 + 0.5) * 255.0).round() as u8;
    let green = ((dy / length * 0.5 + 0.5) * 255.0).round() as u8;
    let radius = width + 0.75;
    let min_x = (ax.min(bx) - radius).floor().clamp(0.0, last) as u32;
    let min_y = (ay.min(by) - radius).floor().clamp(0.0, last) as u32;
    let max_x = (ax.max(bx) + radius).ceil().clamp(0.0, last) as u32;
    let max_y = (ay.max(by) + radius).ceil().clamp(0.0, last) as u32;
    let start_x = (face as u32 % 3) * MASK_SIDE;
    let start_y = (face as u32 / 3) * MASK_SIDE;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let sx = x as f32 - ax;
            let sy = y as f32 - ay;
            let t = ((sx * dx + sy * dy) / (length * length)).clamp(0.0, 1.0);
            let distance = (sx - t * dx).hypot(sy - t * dy);
            let coverage = ((radius - distance) / 1.1).clamp(0.0, 1.0);
            let alpha = (coverage * coverage * (3.0 - 2.0 * coverage) * 255.0).round() as u8;
            let pixel = atlas.get_pixel_mut(start_x + x, start_y + y);
            if alpha > pixel.0[3] {
                *pixel = Rgba([red, green, (width * 85.0).round() as u8, alpha]);
            }
        }
    }
}
