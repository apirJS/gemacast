#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolumeMix {
    pub user_gain: f32,
    pub match_pc: bool,
    pub pc_level: Option<f32>,
}

impl Default for VolumeMix {
    fn default() -> Self {
        Self {
            user_gain: 1.0,
            match_pc: false,
            pc_level: None,
        }
    }
}

impl VolumeMix {
    pub fn effective(&self) -> f32 {
        if !self.match_pc {
            return self.user_gain;
        }
        match self.pc_level {
            Some(level) => self.user_gain * level.clamp(0.0, 1.0),
            None => self.user_gain,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_mix_is_unity_and_unmatched() {
        let mix = VolumeMix::default();
        assert_eq!(mix.user_gain, 1.0);
        assert!(!mix.match_pc);
        assert_eq!(mix.pc_level, None);
        assert_eq!(mix.effective(), 1.0);
    }

    #[test]
    fn the_user_gain_alone_decides_the_output_while_matching_is_off() {
        let mix = VolumeMix {
            user_gain: 2.0,
            match_pc: false,
            pc_level: Some(0.25),
        };
        assert_eq!(mix.effective(), 2.0);
    }

    #[test]
    fn matching_scales_the_user_gain_by_the_pc_level() {
        let mix = VolumeMix {
            user_gain: 2.0,
            match_pc: true,
            pc_level: Some(0.25),
        };
        assert_eq!(mix.effective(), 0.5);
    }

    #[test]
    fn a_muted_pc_silences_the_output_however_high_the_user_gain_is() {
        let mix = VolumeMix {
            user_gain: 4.0,
            match_pc: true,
            pc_level: Some(0.0),
        };
        assert_eq!(mix.effective(), 0.0);
    }

    #[test]
    fn matching_without_a_known_pc_level_leaves_the_user_gain_untouched() {
        let mix = VolumeMix {
            user_gain: 0.5,
            match_pc: true,
            pc_level: None,
        };
        assert_eq!(mix.effective(), 0.5);
    }

    #[test]
    fn a_pc_level_outside_the_unit_range_is_clamped_before_it_scales_anything() {
        let boosted = VolumeMix {
            user_gain: 1.0,
            match_pc: true,
            pc_level: Some(1.5),
        };
        assert_eq!(boosted.effective(), 1.0);

        let negative = VolumeMix {
            user_gain: 1.0,
            match_pc: true,
            pc_level: Some(-0.2),
        };
        assert_eq!(negative.effective(), 0.0);
    }

    #[test]
    fn turning_matching_off_restores_the_full_user_gain() {
        let mut mix = VolumeMix {
            user_gain: 1.0,
            match_pc: true,
            pc_level: Some(0.1),
        };
        assert_eq!(mix.effective(), 0.1);

        mix.match_pc = false;
        assert_eq!(mix.effective(), 1.0);
    }
}
