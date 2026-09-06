use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};

use super::normalize_level;

pub fn read_master_volume_scalar() -> Option<f32> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;

        let muted = endpoint.GetMute().map(|m| m.as_bool()).unwrap_or(false);
        if muted {
            return Some(0.0);
        }

        let scalar = endpoint.GetMasterVolumeLevelScalar().ok()?;
        normalize_level(scalar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod read_master_volume_scalar {
        use super::*;

        #[test]
        fn the_default_render_endpoint_reports_a_unit_range_level_or_nothing() {
            if let Some(level) = read_master_volume_scalar() {
                assert!(level.is_finite(), "got {level}");
                assert!((0.0..=1.0).contains(&level), "got {level}");
            }
        }

        #[test]
        fn re_initializing_com_on_every_call_leaves_later_reads_working() {
            let answers = read_master_volume_scalar().is_some();
            for _ in 0..4 {
                assert_eq!(read_master_volume_scalar().is_some(), answers);
            }
        }
    }
}
