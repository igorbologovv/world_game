//! Canonical orbital coverage, its on-disk cache, and solid-angle mipmaps.

use std::{
    fs::{self, File},
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use bevy::{math::DVec3, prelude::*};
use flate2::{bufread::GzDecoder, write::GzEncoder, Compression};
use world_core::{
    planet::{
        HEIGHT_SOURCE_PATH, LAKE_MASK_PATH, LAKE_SURFACE_PATH, OCEAN_ATLAS_PATH, RIVER_ATLAS_PATH,
        RIVER_SURFACE_PATH,
    },
    SurfaceHeights,
};

use super::{crown, encode};
use crate::forest::placement::PatchKey;

pub(super) const SIDE: u32 = 1024;
pub(super) const MIPS: u32 = SIDE.ilog2() + 1;
pub(super) const PREVIEW_MIP: u32 = 4;
pub(super) const FACES: u32 = 6;
pub(super) const AUTHORED_COLOR_RESPONSE: f32 = 2.65;
pub(super) const ZENITH_LAYER_COUNT: f32 = 2.0;
const BAKED_FILE: &str = "world/planet-canopy-v2.gz";
// Increase the version when tree placement, world sampling, or bake inputs change.
const MAGIC: &[u8; 8] = b"WLCNPY04";
type Fingerprint = [u8; 32];

pub(super) struct Mip {
    pub level: u32,
    // WebGPU cube order: +X, -X, +Y, -Y, +Z, -Z. RGBA8 linear, premultiplied.
    pub bytes: Vec<u8>,
}

impl Mip {
    pub fn side(&self) -> u32 {
        SIDE >> self.level
    }
}

/// Bake the planet's 1024²-per-face forest canopy to a compressed, terrain-keyed file.
///
/// Use the same asset directory as the client. The output can be packaged at
/// `assets/world/planet-canopy-v2.gz`. Camera-local clearings are applied at runtime.
pub fn bake_planet_canopy(assets: &Path, output: &Path) -> io::Result<()> {
    let key = fingerprint(assets)?;
    let heights = SurfaceHeights::load(assets);
    if heights.sample_base(DVec3::X).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing planet heights",
        ));
    }
    store(output, key, &raster(&heights, 0, true))
}

pub(super) fn load_or_bake(
    assets: &Path,
    heights: &SurfaceHeights,
    mut publish: impl FnMut(Vec<Mip>),
) -> io::Result<()> {
    let key = fingerprint(assets)?;
    let cache = cache_file(key);
    let baked = load(&assets.join(BAKED_FILE), key).or_else(|_| {
        cache.as_ref().map_or_else(
            || {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "no cache directory",
                ))
            },
            |file| load(file, key),
        )
    });
    if let Ok(base) = baked {
        publish(mip_chain(base));
        return Ok(());
    }
    if heights.sample_base(DVec3::X).is_none() {
        return Ok(());
    }

    // A separate background worker does the expensive miss path. Keep the preview
    // mips resident during refinement so a terrain cache miss never stalls streaming.
    publish(mip_chain(raster(heights, PREVIEW_MIP, false)));
    let base = raster(heights, 0, false);
    // Prepare only finer levels. The preview levels already cover larger footprints.
    let mut fine = mip_chain(base);
    let base = fine.pop().expect("the base mip is last");
    fine.retain(|mip| mip.level < PREVIEW_MIP);
    let cache_result = cache.map_or(Ok(()), |file| store(&file, key, &base));
    fine.push(base);
    publish(fine);
    cache_result
}

fn cache_file(key: Fingerprint) -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_CACHE_HOME")
        .filter(|path| !path.is_empty())
        .or_else(|| std::env::var_os("LOCALAPPDATA"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|path| PathBuf::from(path).join(".cache")))?;
    let key = blake3::Hash::from_bytes(key);
    Some(dir.join("world-client/canopy").join(format!("{key}.gz")))
}

fn fingerprint(assets: &Path) -> io::Result<Fingerprint> {
    let mut hash = blake3::Hasher::new();
    hash.update(MAGIC);
    hash.update(include_bytes!("../tree_catalog.rs"));
    hash.update(include_bytes!("../tree_coverage.rs"));
    hash.update(&AUTHORED_COLOR_RESPONSE.to_le_bytes());
    hash.update(&ZENITH_LAYER_COUNT.to_le_bytes());
    for relative in (0..FACES)
        .map(|face| Path::new(HEIGHT_SOURCE_PATH).join(format!("face{face}.tif")))
        .chain(
            [
                RIVER_ATLAS_PATH,
                RIVER_SURFACE_PATH,
                LAKE_SURFACE_PATH,
                OCEAN_ATLAS_PATH,
            ]
            .map(PathBuf::from),
        )
        .chain((0..FACES).map(|face| Path::new(LAKE_MASK_PATH).join(format!("face{face}.png"))))
    {
        hash.update(relative.to_str().expect("canonical asset path").as_bytes());
        match File::open(assets.join(relative)) {
            Ok(file) => {
                hash.update(&[1]);
                hash.update(&file.metadata()?.len().to_le_bytes());
                hash.update_reader(file)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                hash.update(&[0]);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(*hash.finalize().as_bytes())
}

fn load(path: &Path, key: Fingerprint) -> io::Result<Mip> {
    let mut reader = GzDecoder::new(BufReader::new(File::open(path)?));
    let mut header = [0; 8];
    let mut actual = [0; 32];
    reader.read_exact(&mut header)?;
    reader.read_exact(&mut actual)?;
    if &header != MAGIC || actual != key {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "stale canopy bake",
        ));
    }
    let mut bytes = vec![0; (SIDE * SIDE * FACES * 4) as usize];
    reader.read_exact(&mut bytes)?;
    if reader.read(&mut [0])? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized canopy bake",
        ));
    }
    Ok(Mip { level: 0, bytes })
}

fn store(path: &Path, key: Fingerprint, mip: &Mip) -> io::Result<()> {
    let parent = path.parent().filter(|dir| !dir.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)?;
    }
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = path.with_extension(format!("{}-{time}.tmp", std::process::id()));
    let file = File::options()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let write = || {
        let mut writer = GzEncoder::new(BufWriter::new(file), Compression::default());
        writer.write_all(MAGIC)?;
        writer.write_all(&key)?;
        writer.write_all(&mip.bytes)?;
        writer.finish()?.flush()?;
        fs::rename(&temporary, path)
    };
    let result = write();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn raster(heights: &SurfaceHeights, level: u32, parallel: bool) -> Mip {
    let side = SIDE >> level;
    let mut bytes = vec![0; (side * side * FACES * 4) as usize];
    let faces = bytes
        .chunks_exact_mut((side * side * 4) as usize)
        .enumerate();
    if parallel {
        thread::scope(|scope| {
            for (face, pixels) in faces {
                scope.spawn(move || raster_face(face, side, pixels, heights));
            }
        });
    } else {
        for (face, pixels) in faces {
            raster_face(face, side, pixels, heights);
        }
    }
    Mip { level, bytes }
}

fn raster_face(face: usize, side: u32, pixels: &mut [u8], heights: &SurfaceHeights) {
    // Four spatial strata per texel, each with four actual placement candidates.
    // Weight crown area by the patch area and convert to fractional cover only
    // after accumulating a representative stand. Directions use native cube
    // faces so hardware filtering can cross seams at every mip.
    let scale = 2.0 / f64::from(side);
    let stride = 28;
    for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
        let x = (index % side as usize) as f64;
        let y = (index / side as usize) as f64;
        let mut area_ratio = Vec4::ZERO;
        for (dx, dy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
            let up = cube_ray(face, (x + dx) * scale - 1.0, (y + dy) * scale - 1.0);
            if up.y.abs() <= 0.9601 {
                let patch = PatchKey::containing(up);
                let area = patch.area().max(1.0) as f32;
                area_ratio += patch
                    .sample_sites(heights, None, stride)
                    .map(crown)
                    .sum::<Vec4>()
                    * (stride as f32 / area);
            }
        }
        pixel.copy_from_slice(&encode(super::cover(area_ratio * 0.25)));
    }
}

fn cube_ray(face: usize, u: f64, v: f64) -> DVec3 {
    match face {
        0 => DVec3::new(1.0, -v, -u),
        1 => DVec3::new(-1.0, -v, u),
        2 => DVec3::new(u, 1.0, v),
        3 => DVec3::new(u, -1.0, -v),
        4 => DVec3::new(u, -v, 1.0),
        _ => DVec3::new(-u, -v, -1.0),
    }
    .normalize()
}

fn mip_chain(base: Mip) -> Vec<Mip> {
    let mut chain = vec![base];
    while chain.last().expect("mip chain has a base").level < MIPS - 1 {
        let source = chain.last().unwrap();
        let side = source.side() as usize;
        let mut bytes = Vec::with_capacity(source.bytes.len() / 4);
        // Center solid angles vary with position, even at equal texel areas.
        let weights: Vec<_> = (0..side)
            .flat_map(|y| {
                (0..side).map(move |x| {
                    let axis = |index| (index as f32 + 0.5) * 2.0 / side as f32 - 1.0;
                    let r2 = 1.0 + axis(x).powi(2) + axis(y).powi(2);
                    1.0 / (r2 * r2.sqrt())
                })
            })
            .collect();
        for face in source.bytes.chunks_exact(side * side * 4) {
            for y in (0..side).step_by(2) {
                for x in (0..side).step_by(2) {
                    let first = y * side + x;
                    let mut rgba = Vec4::ZERO;
                    let mut total = 0.0;
                    for index in [first, first + 1, first + side, first + side + 1] {
                        let color = Vec4::from_array(std::array::from_fn(|channel| {
                            f32::from(face[index * 4 + channel])
                        }));
                        rgba += color * weights[index];
                        total += weights[index];
                    }
                    bytes.extend((rgba / total).round().to_array().map(|value| value as u8));
                }
            }
        }
        chain.push(Mip {
            level: source.level + 1,
            bytes,
        });
    }
    chain.reverse();
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_faces_share_edges_and_use_gpu_orientation() {
        for t in [-1.0, -0.25, 0.0, 0.34, 1.0] {
            assert_eq!(cube_ray(0, -1.0, t), cube_ray(4, 1.0, t));
            assert_eq!(cube_ray(0, 1.0, t), cube_ray(5, -1.0, t));
            assert_eq!(cube_ray(1, -1.0, t), cube_ray(5, 1.0, t));
            assert_eq!(cube_ray(1, 1.0, t), cube_ray(4, -1.0, t));
            assert_eq!(cube_ray(2, t, 1.0), cube_ray(4, t, -1.0));
            assert_eq!(cube_ray(3, t, 1.0), cube_ray(5, -t, 1.0));
        }
    }

    #[test]
    fn packaged_bake_matches_terrain_and_the_canonical_tree_line() {
        let assets = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
        let path = assets.join(BAKED_FILE);
        let key = fingerprint(assets).unwrap();
        let base = load(&path, key).unwrap();
        let alpha = |face, x, y| base.bytes[((face * SIDE + y) * SIDE + x) as usize * 4 + 3];
        assert_eq!(alpha(2, SIDE / 2, SIDE / 2), 0);
        // Boreal river valley used by the client camera sweep.
        assert!(alpha(2, 286, 780) > 40);
        let mut other_key = key;
        other_key[0] ^= 1;
        assert_eq!(
            load(&path, other_key).err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn mipmaps_preserve_premultiplied_coverage_per_face() {
        let bytes = [[0; 4], [24, 40, 32, 160], [0; 4], [0; 4]]
            .into_iter()
            .flatten()
            .cycle()
            .take(FACES as usize * 16)
            .collect();
        let mips = mip_chain(Mip {
            level: MIPS - 2,
            bytes,
        });
        assert_eq!(mips[0].level, MIPS - 1);
        assert_eq!(mips[0].bytes, [6, 10, 8, 40].repeat(FACES as usize));
    }
}
