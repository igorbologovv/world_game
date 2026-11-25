use std::{error::Error, fs::File, io::BufWriter, io::Write, path::Path};

use bevy_math::{DVec2, DVec3, Vec3};
use image::{Rgba, RgbaImage};
use world_core::{
    coordinates::{
        cube_direction, height_source_uv, source_face_uv as projected_uv, surface_uv,
        terrain_baked_uv,
    },
    planet::{BAKED_WATER_MESH_HEADER, RADIUS, RIVER_REACH_VERTICES, WATER_SURFACE_ELEVATION},
};

use super::{
    decode_index, encode_height,
    hydrology::{self, Drainage, Lakes, AREA_UNIT_KM2, NO_LAKE, NO_OUTLET},
    index, neighbour, sphere_direction, HydrologySettings, SIZE,
};

const MASK_SIDE: u32 = (SIZE * 2) as u32;
const LAKE_SURFACE_OFFSET: f64 = 0.08;
const REFERENCE_CATCHMENT: f64 = 90_000.0;

pub(super) fn generate(
    heights: &mut [f64],
    encoded: &mut [u16],
    ocean: &[bool],
    output: &Path,
    surface_output: &Path,
    lake_output: &Path,
    settings: &HydrologySettings,
) -> Result<Vec<bool>, Box<dyn Error>> {
    println!("Hydrology settings: {settings:?}");
    let mut drainage = hydrology::route(heights, ocean);
    let depressions = drainage.depressions(heights);
    let largest = depressions
        .basins
        .iter()
        .map(|lake| lake.area_km2)
        .fold(0.0, f64::max);
    let area: f64 = depressions.basins.iter().map(|lake| lake.area_km2).sum();
    println!(
        "Undrained depressions: {} / {area:.2} km² (largest {largest:.2} km²)",
        depressions.basins.len()
    );
    let (spillways, cuts) =
        hydrology::lower_spillways(heights, ocean, &drainage, depressions, settings);
    println!("Lowered spillways: {spillways} / {cuts} edited terrain samples");
    if cuts > 0 {
        drop(drainage);
        drainage = hydrology::route(heights, ocean);
    }
    let depressions = drainage.depressions(heights);
    let lakes = drainage.retain_lakes(depressions, heights, settings);
    let channels = select_channels(&drainage, &lakes, settings);
    let water = grade_channels(heights, ocean, &drainage, &lakes, &channels);
    for (sample, &height) in encoded.iter_mut().zip(heights.iter()) {
        *sample = encode_height(height);
    }

    let bed = BedSupport {
        heights,
        lakes: &lakes.ids,
        ocean,
        depth_limit: settings.river_bank_depth_limit_m,
    };
    let mut atlas = RgbaImage::new(3 * MASK_SIDE, 2 * MASK_SIDE);
    let mut surface = Vec::new();
    let mut distance = 0.0;
    let mut sections = 0;
    let mut seams = 0;
    let mut incoming = vec![false; heights.len()];
    let mut path_length = vec![0.0_f64; heights.len()];
    let mut longest: f64 = 0.0;
    let mut no_ocean = 0;
    let mut ocean_mouths = 0;
    let mut routed_to_ocean = ocean.to_vec();
    for &node in &drainage.order {
        let cell = node as usize;
        let next = drainage.outlets[cell];
        if next == NO_OUTLET {
            continue;
        }
        let next = next as usize;
        routed_to_ocean[cell] = routed_to_ocean[next];
        if lakes.ids[cell] != NO_LAKE && !channels[cell] {
            // An interior lake node drains directly to its representative outlet.
            path_length[cell] = path_length[next]
                + sample_direction(cell).angle_between(sample_direction(next)) * RADIUS;
            continue;
        }
        if !channels[cell] {
            continue;
        }
        no_ocean += usize::from(!routed_to_ocean[cell]);
        ocean_mouths += usize::from(ocean[next]);
        incoming[next] = true;
        let (face, x, y) = decode_index(cell);
        let (next_face, nx, ny) = decode_index(next);
        let from = grid_uv(x, y);
        let to = grid_uv(nx, ny);
        let upstream = cube_direction(face, terrain_baked_uv(DVec2::from_array(from)));
        let downstream = cube_direction(next_face, terrain_baked_uv(DVec2::from_array(to)));
        let seam = (face != next_face).then(|| {
            let mut uv = terrain_baked_uv(DVec2::from_array(from));
            if x == 0 || x == SIZE - 1 {
                uv.x = from[0];
            }
            if y == 0 || y == SIZE - 1 {
                uv.y = from[1];
            }
            cube_direction(face, uv)
        });
        let length = RADIUS
            * seam.map_or_else(
                || upstream.angle_between(downstream),
                |edge| upstream.angle_between(edge) + edge.angle_between(downstream),
            );
        seams += usize::from(seam.is_some());
        path_length[cell] = path_length[next] + length;
        longest = longest.max(path_length[cell]);
        if length < 0.01 {
            continue;
        }
        let width = channel_width(
            settings,
            f64::from(drainage.accumulation[cell]) * AREA_UNIT_KM2,
        );
        let next_width = if channels[next] {
            channel_width(
                settings,
                f64::from(drainage.accumulation[next]) * AREA_UNIT_KM2,
            )
        } else {
            width
        };
        let end = if ocean[next] {
            WATER_SURFACE_ELEVATION
        } else {
            water[next]
        };
        debug_assert!(
            water[cell] + 1e-8 >= end,
            "channel surface climbs at its outlet"
        );
        let drawn_width = if let Some(edge) = seam {
            // At a seam, each face clamps its boundary sample over a half texel.
            // Go through the shared baked edge instead of cutting the corner.
            let stage = (water[cell] + end) * 0.5;
            let edge_width = (width + next_width) * 0.25;
            let first = append_surface(
                &mut surface,
                &bed,
                upstream,
                edge,
                water[cell],
                stage,
                width * 0.5,
                edge_width,
            );
            let second = append_surface(
                &mut surface,
                &bed,
                edge,
                downstream,
                stage,
                end,
                edge_width,
                next_width * 0.5,
            );
            sections += 2;
            (first + second) * 0.5
        } else {
            sections += 1;
            append_surface(
                &mut surface,
                &bed,
                upstream,
                downstream,
                water[cell],
                end,
                width * 0.5,
                next_width * 0.5,
            )
        };
        let to_direction = sphere_direction(next_face, to[0], to[1]);
        let to_on_face = if face == next_face {
            to
        } else {
            projected_uv(face, to_direction)
        };
        let pixel_distance =
            (to_on_face[0] - from[0]).hypot(to_on_face[1] - from[1]) * f64::from(MASK_SIDE - 1);
        let half_width_px = if pixel_distance > 0.001 {
            (drawn_width * pixel_distance / length) as f32
        } else {
            (drawn_width / (2.0 * RADIUS / f64::from(MASK_SIDE - 1))) as f32
        };
        draw_section(&mut atlas, face, from, to_on_face, half_width_px);
        if face != next_face {
            draw_section(
                &mut atlas,
                next_face,
                projected_uv(next_face, sphere_direction(face, from[0], from[1])),
                to,
                half_width_px,
            );
        }
        distance += length;
    }

    let headwaters = channels
        .iter()
        .enumerate()
        .filter(|&(i, visible)| *visible && lakes.ids[i] == NO_LAKE && !incoming[i])
        .count();
    println!(
        "Rivers: {headwaters} channel heads, {ocean_mouths} ocean mouths, {sections} reaches, {:.2} km total, {:.2} km longest to sea, {seams} face crossings, {no_ocean} disconnected reaches",
        distance / 1000.0, longest / 1000.0
    );
    if ocean.iter().any(|&sea| sea) && no_ocean != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "a visible reach does not drain to the ocean",
        )
        .into());
    }

    let mut lake_file = BufWriter::new(File::create(lake_output)?);
    lake_file.write_all(BAKED_WATER_MESH_HEADER)?;
    let mut lake_count = 0;
    let lake_area: f64 = lakes.basins.iter().map(|lake| lake.area_km2).sum();
    let largest = lakes
        .basins
        .iter()
        .map(|lake| lake.area_km2)
        .fold(0.0, f64::max);
    for lake in &lakes.basins {
        let height = lake.level + LAKE_SURFACE_OFFSET;
        for &node in &lake.cells {
            let (face, x, y) = decode_index(node as usize);
            write_positions(&mut lake_file, &lake_tile(face, x, y, height))?;
            let ax = (face as u32 % 3) * MASK_SIDE + 2 * x as u32;
            let ay = (face as u32 / 3) * MASK_SIDE + 2 * y as u32;
            for dy in 0..2 {
                for dx in 0..2 {
                    atlas.get_pixel_mut(ax + dx, ay + dy).0[3] = 255;
                }
            }
            lake_count += 1;
        }
    }
    lake_file.flush()?;
    println!(
        "Lakes: {} / {lake_count} cells / {lake_area:.2} km² (largest {largest:.2} km²), one outlet per lake",
        lakes.basins.len()
    );
    let wet = inland_area(&atlas, ocean);
    println!("Area-weighted inland water footprint: {wet:.2} km²");
    atlas.save(output)?;
    let mut river_file = BufWriter::new(File::create(surface_output)?);
    river_file.write_all(BAKED_WATER_MESH_HEADER)?;
    assert_eq!(surface.len(), sections * RIVER_REACH_VERTICES);
    write_positions(&mut river_file, &surface)?;
    river_file.flush()?;
    Ok(lakes.ids.iter().map(|&id| id != NO_LAKE).collect())
}

fn select_channels(drainage: &Drainage, lakes: &Lakes, settings: &HydrologySettings) -> Vec<bool> {
    let threshold = (settings.river_catchment_km2 / AREA_UNIT_KM2) as f32;
    let mut channels: Vec<_> = drainage
        .accumulation
        .iter()
        .enumerate()
        .map(|(cell, &area)| {
            lakes.ids[cell] == NO_LAKE && area >= threshold && drainage.outlets[cell] != NO_OUTLET
        })
        .collect();
    for lake in &lakes.basins {
        let mut cell = lake.outlet;
        channels[cell as usize] = true;
        cell = drainage.outlets[cell as usize];
        while cell != NO_OUTLET {
            let i = cell as usize;
            if lakes.ids[i] != NO_LAKE || channels[i] || drainage.outlets[i] == NO_OUTLET {
                break;
            }
            channels[i] = true;
            cell = drainage.outlets[i];
        }
    }
    channels
}

fn grade_channels(
    heights: &mut [f64],
    ocean: &[bool],
    drainage: &Drainage,
    lakes: &Lakes,
    channels: &[bool],
) -> Vec<f64> {
    let mut water = vec![0.0; heights.len()];
    for lake in &lakes.basins {
        for &cell in &lake.cells {
            water[cell as usize] = lake.level + LAKE_SURFACE_OFFSET;
        }
    }
    let mut bank_cuts = vec![0.0_f32; heights.len()];
    let mut raised = 0;
    let mut largest_raise: f64 = 0.0;
    for &node in &drainage.order {
        let cell = node as usize;
        if !channels[cell] || lakes.ids[cell] != NO_LAKE {
            continue;
        }
        let next = drainage.outlets[cell] as usize;
        let (face, x, y) = decode_index(cell);
        let (next_face, nx, ny) = decode_index(next);
        let downstream = if ocean[next] {
            WATER_SURFACE_ELEVATION
        } else {
            water[next]
        };
        let scale =
            (f64::from(drainage.accumulation[cell]) * AREA_UNIT_KM2 / REFERENCE_CATCHMENT).sqrt();
        let depth = (0.82 + scale * 0.43).min(3.4);
        let incision = (2.8 + scale * 1.7).min(17.0);
        let height = heights[cell];
        let surface = if face != next_face {
            downstream
        } else {
            (height - incision + depth).max(downstream)
        };
        water[cell] = surface;
        heights[cell] = surface - depth;
        let raise = heights[cell] - height;
        raised += usize::from(raise > 0.0);
        largest_raise = largest_raise.max(raise);
        let from = grid_uv(x, y);
        let to = if face == next_face {
            grid_uv(nx, ny)
        } else {
            from
        };
        taper_banks(
            &mut bank_cuts,
            face,
            from,
            to,
            1.6,
            ((height - heights[cell]).max(0.0) * 0.47) as f32,
        );
    }

    for (cell, &cut) in bank_cuts.iter().enumerate() {
        if !ocean[cell] && !channels[cell] && lakes.ids[cell] == NO_LAKE {
            heights[cell] -= f64::from(cut).min((heights[cell] - WATER_SURFACE_ELEVATION).max(0.0));
        }
    }
    // Bilinear terrain across a diagonal includes two side samples. Balance the
    // unprotected portions of that cross-section against the graded bed.
    let mut diagonal_beds = Vec::new();
    for &node in &drainage.order {
        let cell = node as usize;
        if !channels[cell] {
            continue;
        }
        let next = drainage.outlets[cell] as usize;
        let (face, x, y) = decode_index(cell);
        let (next_face, nx, ny) = decode_index(next);
        if face != next_face || x == nx || y == ny {
            continue;
        }
        let sides = [index(face, x, ny), index(face, nx, y)];
        let target = if ocean[next] || lakes.ids[next] != NO_LAKE || lakes.ids[cell] != NO_LAKE {
            let end = if ocean[next] {
                WATER_SURFACE_ELEVATION
            } else {
                water[next]
            };
            water[cell] + end - 4.0
        } else {
            heights[cell] + heights[next]
        };
        let usable =
            sides.map(|side| !ocean[side] && !channels[side] && lakes.ids[side] == NO_LAKE);
        if usable[0] || usable[1] {
            diagonal_beds.push((sides, usable, target));
        }
    }
    for _ in 0..20 {
        let mut error = 0.0_f64;
        for &(sides, usable, target) in &diagonal_beds {
            let correction = target - heights[sides[0]] - heights[sides[1]];
            error = error.max(correction.abs());
            let free = f64::from(u8::from(usable[0]) + u8::from(usable[1]));
            for (side, usable) in sides.into_iter().zip(usable) {
                if usable {
                    heights[side] += correction / free;
                }
            }
        }
        if error <= 0.10 {
            break;
        }
    }
    // Preserve equal source positions across shared cube edges, including
    // channels which touch a seam before crossing it.
    for face in 0..6 {
        for t in 0..SIZE {
            for (x, y, dx, dy) in [
                (0, t, -1, 0),
                (SIZE - 1, t, 1, 0),
                (t, 0, 0, -1),
                (t, SIZE - 1, 0, 1),
            ] {
                let (nf, nx, ny) = neighbour(face, x, y, dx, dy);
                let a = index(face, x, y);
                let b = index(nf, nx, ny);
                if !ocean[a] && !ocean[b] && (channels[a] || channels[b]) {
                    let bed = heights[a].min(heights[b]);
                    heights[a] = bed;
                    heights[b] = bed;
                }
            }
        }
    }
    println!("Channel grading: {raised} raised bed samples (maximum {largest_raise:.2} m)");
    water
}

fn sample_direction(cell: usize) -> DVec3 {
    let (face, x, y) = decode_index(cell);
    let [u, v] = grid_uv(x, y);
    DVec3::from_array(sphere_direction(face, u, v))
}

fn channel_width(settings: &HydrologySettings, area: f64) -> f64 {
    (settings.river_reference_width_m * (area / REFERENCE_CATCHMENT).powf(0.39)).clamp(
        settings.river_reference_width_m * 0.5,
        settings.river_max_width_m,
    )
}

fn lake_tile(face: usize, x: usize, y: usize, height: f64) -> [Vec3; 6] {
    let last = (SIZE - 1) as f64;
    let at = |dx: f64, dy: f64| {
        let u = (x as f64 + dx).clamp(0.0, last) / last;
        let v = (y as f64 + dy).clamp(0.0, last) / last;
        let source = DVec2::new(u, v);
        let mut uv = terrain_baked_uv(source);
        if u == 0.0 || u == 1.0 {
            uv.x = u;
        }
        if v == 0.0 || v == 1.0 {
            uv.y = v;
        }
        (cube_direction(face, uv) * (RADIUS + height)).as_vec3()
    };
    let [a, b, c, d] = [at(-0.5, -0.5), at(0.5, -0.5), at(0.5, 0.5), at(-0.5, 0.5)];
    if (b - a).cross(c - a).dot(a) > 0.0 {
        [a, b, c, a, c, d]
    } else {
        [a, c, b, a, d, c]
    }
}

fn write_positions(file: &mut impl Write, positions: &[Vec3]) -> std::io::Result<()> {
    for p in positions {
        for value in p.to_array() {
            file.write_all(&value.to_le_bytes())?;
        }
    }
    Ok(())
}

fn inland_area(atlas: &RgbaImage, ocean: &[bool]) -> f64 {
    use world_core::coordinates::CUBE_CORRECTION as c;
    let last = f64::from(MASK_SIDE - 1);
    let mut weighted_area = 0.0;
    let pixel_area = (2.0 * RADIUS / last).powi(2) / 1e6;
    let source = |p| (f64::from(p) * (SIZE - 1) as f64 / last).round() as usize;
    for face in 0..6 {
        let start_x = (face as u32 % 3) * MASK_SIDE;
        let start_y = (face as u32 / 3) * MASK_SIDE;
        for y in 0..MASK_SIDE {
            let v = 2.0 * f64::from(y) / last - 1.0;
            let sy = 1.0 + c - c * v * v;
            for x in 0..MASK_SIDE {
                if ocean[index(face, source(x), source(y))]
                    || atlas.get_pixel(start_x + x, start_y + y).0[3] < 156
                {
                    continue;
                }
                let u = 2.0 * f64::from(x) / last - 1.0;
                let sx = 1.0 + c - c * u * u;
                let jacobian =
                    (1.0 + c).powi(2) / ((sx * sy) * (1.0 + u * u / sx + v * v / sy)).powf(1.5);
                let border_x = if x == 0 || x + 1 == MASK_SIDE {
                    0.5
                } else {
                    1.0
                };
                let border_y = if y == 0 || y + 1 == MASK_SIDE {
                    0.5
                } else {
                    1.0
                };
                weighted_area += jacobian * border_x * border_y;
            }
        }
    }
    weighted_area * pixel_area
}

struct BedSupport<'a> {
    heights: &'a [f64],
    lakes: &'a [u32],
    ocean: &'a [bool],
    depth_limit: f64,
}

impl BedSupport<'_> {
    fn at(&self, position: DVec3, water_height: f64) -> bool {
        let (face, uv) = surface_uv(position);
        let source = height_source_uv(uv, DVec2::splat(SIZE as f64)) * (SIZE - 1) as f64;
        let closest = index(face, source.x.round() as usize, source.y.round() as usize);
        if self.ocean[closest] || self.lakes[closest] != NO_LAKE {
            return true;
        }
        let x = source.x.floor() as usize;
        let y = source.y.floor() as usize;
        let nx = (x + 1).min(SIZE - 1);
        let ny = (y + 1).min(SIZE - 1);
        let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
        let h = |x, y| self.heights[index(face, x, y)];
        let top = lerp(h(x, y), h(nx, y), source.x.fract());
        let bottom = lerp(h(x, ny), h(nx, ny), source.x.fract());
        let ground = lerp(top, bottom, source.y.fract());
        water_height - ground <= self.depth_limit
    }
}

fn append_surface(
    vertices: &mut Vec<Vec3>,
    bed: &BedSupport,
    from: DVec3,
    to: DVec3,
    from_height: f64,
    to_height: f64,
    from_width: f64,
    to_width: f64,
) -> f64 {
    let Some(axis) = from.cross(to).try_normalize() else {
        return 0.0;
    };
    let start = from * (RADIUS + from_height);
    let end = to * (RADIUS + to_height);
    let from_width = from_width.max(4.0);
    let to_width = to_width.max(4.0);
    let mut scale = 1.0;
    for _ in 0..10 {
        let supported = (0..=8).all(|station| {
            let t = f64::from(station) / 8.0;
            let center = start.lerp(end, t);
            let height = from_height + (to_height - from_height) * t;
            let half_width = ((from_width + (to_width - from_width) * t) * scale).max(4.0);
            [-1.0, 1.0]
                .into_iter()
                .all(|side| bed.at(center + axis * half_width * side, height))
        });
        if supported || from_width.max(to_width) * scale <= 4.0 {
            break;
        }
        scale *= 0.70;
    }
    let from_width = (from_width * scale).max(4.0);
    let to_width = (to_width * scale).max(4.0);
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

    let downstream_tangent = axis.cross(from);
    let gradient = (to_height - from_height) / (RADIUS * from.angle_between(to));
    for sector in 0..8 {
        let point = |sector: i32| {
            let angle = f64::from(sector) * std::f64::consts::TAU / 8.0;
            let tangent = axis * angle.cos() - downstream_tangent * angle.sin();
            let mut offset = tangent * from_width;
            let mut height = from_height + gradient * offset.dot(downstream_tangent);
            for _ in 0..8 {
                if bed.at(start + offset, height) {
                    break;
                }
                offset *= 0.65;
                height = from_height + gradient * offset.dot(downstream_tangent);
            }
            on_surface(start + offset, height)
        };
        vertices.extend([start.as_vec3(), point(sector), point(sector + 1)]);
    }
    (from_width + to_width) * 0.5
}

fn on_surface(position: DVec3, height: f64) -> Vec3 {
    (position.normalize() * (RADIUS + height)).as_vec3()
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
    let norm = length.max(0.001);
    let red = ((dx / norm * 0.5 + 0.5) * 255.0).round() as u8;
    let green = ((dy / norm * 0.5 + 0.5) * 255.0).round() as u8;
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
            let t = ((sx * dx + sy * dy) / (norm * norm)).clamp(0.0, 1.0);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lake_tiles_have_two_outward_triangles_at_the_same_water_level() {
        for face in 0..6 {
            for (x, y) in [(0, SIZE / 2), (100, 100), (SIZE - 1, SIZE - 1)] {
                let vertices = lake_tile(face, x, y, 9.6);
                for triangle in vertices.chunks_exact(3) {
                    let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
                    assert!((b - a).cross(c - a).dot(a) > 0.0);
                    // The legacy binary stores absolute positions in f32.
                    let tolerance = RADIUS * f64::from(f32::EPSILON);
                    assert!(triangle
                        .iter()
                        .all(|p| { (p.as_dvec3().length() - RADIUS - 9.6).abs() < tolerance }));
                }
            }
        }
    }

    #[test]
    fn width_grows_with_runoff_and_caps_at_the_major_river_width() {
        let settings = HydrologySettings::default();
        assert!(
            channel_width(&settings, REFERENCE_CATCHMENT * 3.0)
                > channel_width(&settings, REFERENCE_CATCHMENT)
        );
        assert_eq!(
            channel_width(&settings, REFERENCE_CATCHMENT * 100.0),
            settings.river_max_width_m
        );
    }
}
