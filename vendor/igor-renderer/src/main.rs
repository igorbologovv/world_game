use glam::Vec3;
use igor_renderer::{shade, trace, Scene, Stats, DEFAULT_DENSITY, DEFAULT_SIDE};
use std::{
    env,
    fs::File,
    io::{BufWriter, Write},
    time::Instant,
};
mod frame_counter;
mod viewer;
mod splat_viewer;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help") {
        println!("igor_renderer [--side {DEFAULT_SIDE}] [--density {DEFAULT_DENSITY}] [--headless] [--width PX] [--height PX] [--out forest.ppm] [--compare] [--aerial] [--ray]");
        return Ok(());
    }
    let value = |name: &str, default: &str| -> String {
        args.windows(2)
            .find(|w| w[0] == name)
            .map(|w| w[1].clone())
            .unwrap_or_else(|| default.to_owned())
    };
    let flag = |name: &str| args.iter().any(|a| a == name);
    let side: u32 = value("--side", &DEFAULT_SIDE.to_string()).parse()?;
    let density: u32 = value("--density", &DEFAULT_DENSITY.to_string()).parse()?;
    if !(1..=1024).contains(&side) || density > 100 {
        return Err("side must be 1..=1024; density 0..=100".into());
    }
    let headless = flag("--headless") || flag("--compare");
    let w: u32 = value("--width", if headless { "640" } else { "960" }).parse()?;
    let h: u32 = value("--height", if headless { "360" } else { "540" }).parse()?;
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return Err("invalid dimensions".into());
    }
    let start = Instant::now();
    let ray=headless||flag("--ray");
    let scene=if ray {Scene::try_forest(side,density)?} else {Scene::try_forest_splats(side,density)?};
    println!("{} trees at {} sites, {} models, {} unique surfels, {} instanced surfels; {:.2} MiB scene buffers; built in {:.2?}",
        scene.trees, scene.occupied_sites, scene.geometry.batches.len(), scene.geometry.surfels.len(), scene.instanced_surfels,
        scene.bytes() as f64/1048576.0, start.elapsed());
    if !headless {
        return if ray {viewer::run(scene,flag("--aerial"),w,h)} else {splat_viewer::run(scene,flag("--aerial"),w,h)};
    }
    let camera = viewer::Camera::new(&scene, flag("--aerial"));
    let render = |skip: bool| {
        let start = Instant::now();
        let mut stats = Stats::default();
        let mut pixels = Vec::with_capacity((w * h * 3) as usize);
        let (forward, right, up) = camera.basis();
        let scale = 2.0 * (60f32.to_radians() / 2.0).tan() / h as f32;
        for y in 0..h {
            for x in 0..w {
                let d = (forward
                    + right * ((x as f32 + 0.5 - w as f32 * 0.5) * scale)
                    + up * ((h as f32 * 0.5 - y as f32 - 0.5) * scale))
                    .normalize();
                let hit = trace(&scene, camera.position, d, scale, skip, &mut stats);
                let c = shade(hit, d).clamp(Vec3::ZERO, Vec3::ONE);
                pixels.extend(
                    c.to_array()
                        .map(|v| (v.powf(1.0 / 2.2) * 255.0 + 0.5) as u8),
                );
            }
        }
        println!(
            "skip={skip}: {:?}; blocks={} jumps={} surfel_tests={}",
            start.elapsed(),
            stats.blocks,
            stats.jumps,
            stats.surfels
        );
        pixels
    };
    let pixels = render(true);
    if flag("--compare") {
        let reference = render(false);
        let mismatches = pixels
            .chunks_exact(3)
            .zip(reference.chunks_exact(3))
            .filter(|(a, b)| a != b)
            .count();
        println!("different pixels: {mismatches}/{}", w * h);
        if mismatches != 0 {
            return Err("skip/reference mismatch".into());
        }
    }
    let out = value("--out", "forest.ppm");
    let mut file = BufWriter::new(File::create(&out)?);
    write!(file, "P6\n{w} {h}\n255\n")?;
    file.write_all(&pixels)?;
    println!("saved {out}");
    Ok(())
}
