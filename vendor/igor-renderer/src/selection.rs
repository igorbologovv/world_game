//! Continuous screen-space selection from Igor renderer's hybrid scene.

#[derive(Clone, Copy)]
/// Continuous LOD state for a stable tree placement.
pub struct Detail {
    target: usize,
    phase: f32,
}

impl Detail {
    fn advance(&mut self, elapsed: f32, seconds: [f32; 2]) {
        let goal = self.target as f32;
        let mut remaining = elapsed;
        while self.phase != goal {
            let outward = self.phase < goal;
            let stage = if outward {
                self.phase.floor()
            } else {
                self.phase.ceil() - 1.0
            };
            let end = if outward { stage + 1.0 } else { stage };
            let duration = seconds[stage as usize];
            let time = (end - self.phase).abs() * duration;
            if remaining >= time {
                self.phase = end;
                remaining -= time;
            } else {
                let step = remaining / duration;
                self.phase += if outward { step } else { -step };
                break;
            }
        }
    }
}

/// Stable rank for spreading transitions across neighboring trees.
pub fn rank(id: u32, cell: [i32; 2]) -> f32 {
    let mut bits =
        id ^ (cell[0] as u32).wrapping_mul(0x9e3779b9) ^ (cell[1] as u32).wrapping_mul(0x85ebca6b);
    bits = (bits ^ (bits >> 16)).wrapping_mul(0x7feb352d);
    bits = (bits ^ (bits >> 15)).wrapping_mul(0x846ca68b);
    ((bits ^ (bits >> 16)) >> 8) as f32 * (1.0 / 16777216.0)
}

/// Projected size thresholds and transition timing for the hybrid renderer.
#[derive(Clone, Copy)]
pub struct Selection {
    pub middle_pixels: f32,
    pub far_pixels: f32,
    pub cell_pixels: f32,
    pub hysteresis: f32,
    pub seconds: [f32; 2],
}

impl Selection {
    /// Select a target from projected tree and cell sizes, then advance toward it.
    /// The returned phase indexes the three mesh prefixes and interpolates between them.
    pub fn select(
        self,
        state: &mut Option<Detail>,
        pixels: f32,
        cell_pixels: f32,
        rank: f32,
        elapsed: f32,
    ) -> f32 {
        let band = |to| match *state {
            Some(detail) if detail.target >= to => 1.0 + self.hysteresis,
            Some(_) => 1.0 - self.hysteresis,
            None => 1.0,
        };
        let spread = 0.90 + 0.20 * rank;
        let target = if pixels >= self.middle_pixels * band(1) * spread {
            0
        } else if pixels >= self.far_pixels * band(2) * spread
            || cell_pixels >= self.cell_pixels * band(2) * spread
        {
            1
        } else {
            2
        };
        let detail = state.get_or_insert(Detail {
            target,
            phase: target as f32,
        });
        detail.target = target;
        detail.advance(elapsed, self.seconds);
        detail.phase
    }
}

#[cfg(test)]
mod tests {
    use super::Detail;

    #[test]
    fn transition_reverses_without_resetting_and_settles() {
        let mut lod = Detail {
            target: 2,
            phase: 0.0,
        };
        let seconds = [0.4, 0.65];
        lod.advance(0.5, seconds);
        assert!(lod.phase > 1.0 && lod.phase < 2.0);
        lod.target = 0;
        lod.advance(0.1, seconds);
        assert!((lod.phase - 1.0).abs() < 1e-6);
        lod.advance(0.2, seconds);
        assert!((lod.phase - 0.5).abs() < 1e-6);
        lod.advance(1.0, seconds);
        assert_eq!(lod.phase, 0.0);
        lod.target = 2;
        lod.advance(2.0, seconds);
        assert_eq!(lod.phase, 2.0);
    }

    #[test]
    fn zero_duration_can_cross_both_levels() {
        let mut lod = Detail {
            target: 2,
            phase: 0.0,
        };
        lod.advance(0.0, [0.0; 2]);
        assert_eq!(lod.phase, 2.0);
        lod.target = 0;
        lod.advance(0.0, [0.0; 2]);
        assert_eq!(lod.phase, 0.0);
    }
}
