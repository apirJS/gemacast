use super::normalize_level;

pub fn read_default_sink_volume() -> Option<f32> {
    let output = crate::process::quiet_command("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_get_volume(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_get_volume(stdout: &str) -> Option<f32> {
    let line = stdout.trim();
    let rest = line.strip_prefix("Volume:")?.trim();
    if rest.split_whitespace().any(|token| token == "[MUTED]") {
        return Some(0.0);
    }
    let value: f32 = rest.split_whitespace().next()?.parse().ok()?;
    normalize_level(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parse_get_volume {
        use super::*;

        #[test]
        fn a_plain_volume_line_yields_its_linear_scalar() {
            assert_eq!(parse_get_volume("Volume: 0.65\n"), Some(0.65));
        }

        #[test]
        fn unity_and_silence_both_parse() {
            assert_eq!(parse_get_volume("Volume: 1.00\n"), Some(1.0));
            assert_eq!(parse_get_volume("Volume: 0.00\n"), Some(0.0));
        }

        #[test]
        fn a_muted_sink_reads_as_silence_regardless_of_its_stored_level() {
            assert_eq!(parse_get_volume("Volume: 0.80 [MUTED]\n"), Some(0.0));
        }

        #[test]
        fn a_level_above_unity_is_clamped_because_pipewire_allows_boost() {
            assert_eq!(parse_get_volume("Volume: 1.50\n"), Some(1.0));
        }

        #[test]
        fn output_without_the_volume_prefix_is_rejected() {
            assert_eq!(parse_get_volume("Node 42 not found\n"), None);
            assert_eq!(parse_get_volume(""), None);
        }

        #[test]
        fn a_non_numeric_level_is_rejected_rather_than_defaulted() {
            assert_eq!(parse_get_volume("Volume: high\n"), None);
        }
    }

    mod read_default_sink_volume {
        use super::*;
        use serial_test::serial;

        #[test]
        #[serial(pipewire)]
        fn the_default_sink_reports_a_unit_range_level_or_nothing_without_wireplumber() {
            if let Some(level) = read_default_sink_volume() {
                assert!(level.is_finite(), "got {level}");
                assert!((0.0..=1.0).contains(&level), "got {level}");
            }
        }

        #[test]
        #[serial(pipewire)]
        fn wpctl_output_stays_parseable_across_repeated_polls() {
            let answers = read_default_sink_volume().is_some();
            for _ in 0..4 {
                assert_eq!(read_default_sink_volume().is_some(), answers);
            }
        }
    }
}
