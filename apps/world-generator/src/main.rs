use std::{env, error::Error, io, path::Path};

use world_generation::{generate_world_with_hydrology, HydrologySettings};

fn main() -> Result<(), Box<dyn Error>> {
    let mut settings = HydrologySettings::default();
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        if flag == "--help" {
            println!(
                "generate_world [OPTION VALUE...]\n\
                 --river-catchment-km2\n--spillway-min-area-km2\n--spillway-drawdown-m\n\
                 --spillway-max-cells\n--lake-min-area-km2\n--lake-min-depth-m\n\
                 --river-reference-width-m\n--river-max-width-m\n--river-bank-depth-limit-m\n\nWrites generated files under ./assets"
            );
            return Ok(());
        }
        let value = args.next().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("missing value for {flag}"),
            )
        })?;
        match flag.as_str() {
            "--river-catchment-km2" => settings.river_catchment_km2 = value.parse()?,
            "--spillway-min-area-km2" => settings.spillway_min_area_km2 = value.parse()?,
            "--spillway-drawdown-m" => settings.spillway_drawdown_m = value.parse()?,
            "--spillway-max-cells" => settings.spillway_max_cells = value.parse()?,
            "--lake-min-area-km2" => settings.lake_min_area_km2 = value.parse()?,
            "--lake-min-depth-m" => settings.lake_min_depth_m = value.parse()?,
            "--river-reference-width-m" => settings.river_reference_width_m = value.parse()?,
            "--river-max-width-m" => settings.river_max_width_m = value.parse()?,
            "--river-bank-depth-limit-m" => settings.river_bank_depth_limit_m = value.parse()?,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown option {flag}"),
                )
                .into())
            }
        }
    }
    generate_world_with_hydrology(Path::new("assets"), settings)
}
