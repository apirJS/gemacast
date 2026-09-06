#[cfg(target_os = "windows")]
pub mod wasapi_endpoint;

#[cfg(target_os = "linux")]
pub mod wpctl;

#[cfg(target_os = "macos")]
pub mod osascript;

pub use crate::ports::output_volume::OutputVolumeReader;

#[cfg(not(target_os = "android"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct PlatformOutputVolumeReader;

#[cfg(not(target_os = "android"))]
impl PlatformOutputVolumeReader {
    pub fn new() -> Self {
        Self
    }

    pub fn is_supported(&self) -> bool {
        self.read().is_some()
    }
}

#[cfg(not(target_os = "android"))]
impl OutputVolumeReader for PlatformOutputVolumeReader {
    fn read(&self) -> Option<f32> {
        #[cfg(target_os = "windows")]
        return wasapi_endpoint::read_master_volume_scalar();

        #[cfg(target_os = "linux")]
        return wpctl::read_default_sink_volume();

        #[cfg(target_os = "macos")]
        return osascript::read_output_volume();

        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        return None;
    }
}

pub fn normalize_level(raw: f32) -> Option<f32> {
    if !raw.is_finite() {
        return None;
    }
    Some(raw.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    mod normalize_level {
        use super::*;

        #[test]
        fn a_level_inside_the_unit_range_is_returned_unchanged() {
            assert_eq!(normalize_level(0.42), Some(0.42));
            assert_eq!(normalize_level(0.0), Some(0.0));
            assert_eq!(normalize_level(1.0), Some(1.0));
        }

        #[test]
        fn a_level_above_unity_is_clamped_rather_than_rejected() {
            assert_eq!(normalize_level(1.4), Some(1.0));
        }

        #[test]
        fn a_negative_level_is_clamped_to_silence() {
            assert_eq!(normalize_level(-0.3), Some(0.0));
        }

        #[test]
        fn a_non_finite_level_is_rejected_so_it_never_reaches_the_audio_path() {
            assert_eq!(normalize_level(f32::NAN), None);
            assert_eq!(normalize_level(f32::INFINITY), None);
            assert_eq!(normalize_level(f32::NEG_INFINITY), None);
        }
    }

    #[cfg(not(target_os = "android"))]
    mod platform_reader {
        use super::*;

        fn an_output_device_is_promised() -> bool {
            std::env::var("GEMACAST_EXPECT_OS_VOLUME").as_deref() == Ok("1")
        }

        #[test]
        #[cfg_attr(target_os = "linux", serial_test::serial(pipewire))]
        fn a_level_read_from_this_machine_lands_inside_the_unit_range_or_is_absent() {
            let level = PlatformOutputVolumeReader::new().read();
            println!("platform output volume: {level:?}");
            if let Some(level) = level {
                assert!(level.is_finite(), "got {level}");
                assert!((0.0..=1.0).contains(&level), "got {level}");
            }
        }

        #[test]
        #[cfg_attr(target_os = "linux", serial_test::serial(pipewire))]
        fn a_level_is_readable_wherever_the_environment_promises_an_output_device() {
            if !an_output_device_is_promised() {
                return;
            }
            let level = PlatformOutputVolumeReader::new().read();
            assert!(
                level.is_some(),
                "GEMACAST_EXPECT_OS_VOLUME=1 but the platform reader answered nothing"
            );
        }

        #[test]
        #[cfg_attr(target_os = "linux", serial_test::serial(pipewire))]
        fn support_is_claimed_only_when_the_platform_actually_answers() {
            let reader = PlatformOutputVolumeReader::new();
            assert_eq!(reader.is_supported(), reader.read().is_some());
        }

        #[test]
        #[cfg_attr(target_os = "linux", serial_test::serial(pipewire))]
        fn a_read_stays_answerable_when_repeated_because_the_watcher_polls_forever() {
            let reader = PlatformOutputVolumeReader::new();
            let answers = reader.read().is_some();
            for _ in 0..4 {
                assert_eq!(reader.read().is_some(), answers);
            }
        }
    }
}
