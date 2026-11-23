use std::{env, io, path::PathBuf, time::Instant};

fn main() -> io::Result<()> {
    let mut args = env::args_os().skip(1);
    let assets = args.next().map_or_else(|| "assets".into(), PathBuf::from);
    let output = args
        .next()
        .map_or_else(|| assets.join("world/planet-canopy-v1.gz"), PathBuf::from);
    if args.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: bake_canopy [ASSETS_DIRECTORY] [OUTPUT_FILE]",
        ));
    }
    let start = Instant::now();
    world_vegetation::bake_planet_canopy(&assets, &output)?;
    println!("Wrote {} in {:.1?}", output.display(), start.elapsed());
    Ok(())
}
