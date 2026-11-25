use std::io;

/// Controls for the deterministic offline inland-water bake.
#[derive(Clone, Copy, Debug)]
pub struct HydrologySettings {
    /// Upstream contributing area needed to start an ordinary river, in km².
    pub river_catchment_km2: f64,
    /// Minimum undrained lake footprint eligible for lowering its spillway, in km².
    pub spillway_min_area_km2: f64,
    /// Maximum incision of a lake outlet below its original sill, in meters.
    pub spillway_drawdown_m: f64,
    /// Maximum number of terrain samples cut along one spillway.
    pub spillway_max_cells: usize,
    /// Minimum final lake footprint, in km², for basins without a major throughflow.
    pub lake_min_area_km2: f64,
    /// Minimum maximum depth, in meters, for an ordinary retained lake.
    pub lake_min_depth_m: f64,
    /// Nominal full river width at the 90,000 km² reference catchment, in meters.
    pub river_reference_width_m: f64,
    /// Maximum full width of major rivers, in meters.
    pub river_max_width_m: f64,
    /// Maximum water depth at dry-terrain ribbon banks before narrowing the reach.
    pub river_bank_depth_limit_m: f64,
}

impl Default for HydrologySettings {
    fn default() -> Self {
        Self {
            river_catchment_km2: 90_000.0,
            spillway_min_area_km2: 1_200.0,
            spillway_drawdown_m: 210.0,
            spillway_max_cells: 192,
            lake_min_area_km2: 5_700.0,
            lake_min_depth_m: 8.0,
            river_reference_width_m: 82.0,
            river_max_width_m: 275.0,
            river_bank_depth_limit_m: 8.0,
        }
    }
}

impl HydrologySettings {
    pub(super) fn validate(self) -> io::Result<()> {
        let invalid = |name| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid hydrology setting {name}"),
            )
        };
        for (name, value, min, max) in [
            (
                "river_catchment_km2",
                self.river_catchment_km2,
                0.1,
                5_000_000.0,
            ),
            (
                "spillway_min_area_km2",
                self.spillway_min_area_km2,
                0.0,
                5_000_000.0,
            ),
            ("spillway_drawdown_m", self.spillway_drawdown_m, 0.0, 350.0),
            (
                "lake_min_area_km2",
                self.lake_min_area_km2,
                0.0,
                5_000_000.0,
            ),
            ("lake_min_depth_m", self.lake_min_depth_m, 0.0, 1_000.0),
            (
                "river_reference_width_m",
                self.river_reference_width_m,
                10.0,
                500.0,
            ),
            (
                "river_max_width_m",
                self.river_max_width_m,
                self.river_reference_width_m,
                650.0,
            ),
            (
                "river_bank_depth_limit_m",
                self.river_bank_depth_limit_m,
                4.0,
                30.0,
            ),
        ] {
            if !(min..=max).contains(&value) {
                return Err(invalid(name));
            }
        }
        if self.spillway_max_cells > 1_024 {
            return Err(invalid("spillway_max_cells"));
        }
        Ok(())
    }
}
