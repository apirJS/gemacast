use super::normalize_level;

const SCRIPT: &str = "set s to (get volume settings)
if output muted of s then
  return 0
else
  return output volume of s
end if";

pub fn read_output_volume() -> Option<f32> {
    let output = crate::process::quiet_command("osascript")
        .args(["-e", SCRIPT])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_output_volume(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_output_volume(stdout: &str) -> Option<f32> {
    let percent: f32 = stdout.trim().parse().ok()?;
    normalize_level(percent / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parse_output_volume {
        use super::*;

        #[test]
        fn a_percentage_is_converted_to_a_linear_scalar() {
            assert_eq!(parse_output_volume("50\n"), Some(0.5));
            assert_eq!(parse_output_volume("100"), Some(1.0));
            assert_eq!(parse_output_volume("0\n"), Some(0.0));
        }

        #[test]
        fn surrounding_whitespace_is_tolerated() {
            assert_eq!(parse_output_volume("  75  \n"), Some(0.75));
        }

        #[test]
        fn a_fractional_percentage_is_accepted() {
            let level = parse_output_volume("12.5").expect("should parse");
            assert!((level - 0.125).abs() < 1e-6, "got {level}");
        }

        #[test]
        fn a_percentage_above_one_hundred_is_clamped_to_unity() {
            assert_eq!(parse_output_volume("140"), Some(1.0));
        }

        #[test]
        fn unparseable_output_is_rejected_rather_than_defaulted() {
            assert_eq!(parse_output_volume(""), None);
            assert_eq!(parse_output_volume("missing value"), None);
            assert_eq!(parse_output_volume("50%"), None);
        }
    }

    mod read_output_volume {
        use super::*;

        #[test]
        fn this_mac_reports_a_unit_range_level_or_nothing() {
            if let Some(level) = read_output_volume() {
                assert!(level.is_finite(), "got {level}");
                assert!((0.0..=1.0).contains(&level), "got {level}");
            }
        }

        #[test]
        fn the_script_keeps_returning_a_parseable_number_across_repeated_polls() {
            let answers = read_output_volume().is_some();
            for _ in 0..4 {
                assert_eq!(read_output_volume().is_some(), answers);
            }
        }
    }
}
