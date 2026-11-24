//! The generated tree manifest and its mapping into the forest's biome families.

use super::Species;

#[derive(Clone, Copy)]
pub(super) struct TreeAsset {
    pub path: &'static str,
    pub species: Species,
    pub height: f32,
    pub width: f32,
    pub color: [f32; 3],
    pub snow: bool,
}

pub(super) const ASSETS: &[TreeAsset] = include!("tree_catalog.rs");
pub(super) const ZENITH_AREAS: [f32; ASSETS.len()] = include!("tree_coverage.rs");

const fn ranges() -> [(usize, usize); Species::ALL.len()] {
    let mut result = [(0, 0); Species::ALL.len()];
    let mut index = 0;
    while index < ASSETS.len() {
        let range = &mut result[ASSETS[index].species as usize];
        if range.1 == 0 {
            range.0 = index;
        }
        range.1 += 1;
        index += 1;
    }
    result
}

const RANGES: [(usize, usize); Species::ALL.len()] = ranges();

pub(super) const fn count(species: Species) -> usize {
    RANGES[species as usize].1
}

pub(super) fn index(species: Species, variant: usize) -> Option<usize> {
    let (base, count) = RANGES[species as usize];
    let local = variant.checked_sub(species.stock_variants())?;
    (local < count).then_some(base + local)
}

pub(super) fn variant(asset_index: usize) -> usize {
    let species = ASSETS[asset_index].species;
    species.stock_variants() + asset_index - RANGES[species as usize].0
}

pub(super) fn asset(species: Species, variant: usize) -> Option<&'static TreeAsset> {
    index(species, variant).map(|i| &ASSETS[i])
}

pub(super) fn choose(species: Species, roll: usize, latitude: f64, elevation: f64) -> usize {
    let selected = roll % species.variants();
    let Some(index) = index(species, selected) else {
        return selected;
    };
    let cold = latitude > 0.77 || (latitude > 0.59 && elevation > 1_350.0);
    let suitable = |entry: &TreeAsset| {
        (species != Species::Pine || entry.snow == cold)
            && (elevation <= 55.0 || !entry.path.starts_with("library/red-mangrove/"))
    };
    if suitable(&ASSETS[index]) {
        return selected;
    }
    let (start, len) = RANGES[species as usize];
    // Walk the same habitat's catalog before using a stock fallback.
    (1..len)
        .map(|offset| start + (index - start + offset) % len)
        .find(|&index| suitable(&ASSETS[index]))
        .map_or(roll % species.stock_variants(), variant)
}

pub(super) fn leaf_color(species: Species, variant: usize) -> [f32; 3] {
    asset(species, variant).map_or_else(|| species.colors().1, |entry| entry.color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_variants_round_trip_and_obey_climate_limits() {
        for (i, entry) in ASSETS.iter().enumerate() {
            assert_eq!(index(entry.species, variant(i)), Some(i));
        }
        let count_kind =
            |latitude, elevation, species: Species, condition: fn(&TreeAsset) -> bool| {
                (0..species.variants())
                    .filter_map(|roll| asset(species, choose(species, roll, latitude, elevation)))
                    .filter(|entry| condition(entry))
                    .count()
            };
        let snow = |entry: &TreeAsset| entry.snow;
        let mangrove = |entry: &TreeAsset| entry.path.starts_with("library/red-mangrove/");
        assert!(count_kind(0.85, 250.0, Species::Pine, snow) > 0);
        assert_eq!(count_kind(0.53, 250.0, Species::Pine, snow), 0);
        assert!(count_kind(0.65, 1450.0, Species::Pine, snow) > 0);
        assert!(count_kind(0.19, 25.0, Species::Jungle, mangrove) > 0);
        assert_eq!(count_kind(0.19, 200.0, Species::Jungle, mangrove), 0);
    }
}
