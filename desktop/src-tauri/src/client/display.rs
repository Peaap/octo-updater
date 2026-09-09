/// Current display resolution and refresh rate, used to seed `Config.wtf`'s
/// `gxResolution`/`gxRefresh`. Ported from the legacy Python updater's
/// `_get_display_info_safe` (a direct `EnumDisplaySettingsW` call). Falls back
/// to a safe default off Windows or if the call fails, matching the Python
/// updater's own `except Exception` fallback behavior.
pub fn current_display_info() -> (u32, u32, u32) {
    #[cfg(windows)]
    {
        if let Some(info) = win32::query_display_info() {
            return info;
        }
    }
    (1920, 1080, 60)
}

#[cfg(windows)]
mod win32 {
    use std::mem::size_of;

    const ENUM_CURRENT_SETTINGS: u32 = 0xFFFFFFFF; // -1 as u32

    // Only the trailing fields this call actually reads are declared, laid
    // out to match DEVMODEW's tail exactly (verified against the Windows SDK
    // header); the fixed-size name fields before them are included only for
    // correct offsets, not read.
    #[repr(C)]
    struct DevModeW {
        dm_device_name: [u16; 32],
        dm_spec_version: u16,
        dm_driver_version: u16,
        dm_size: u16,
        dm_driver_extra: u16,
        dm_fields: u32,
        dm_position_x: i32,
        dm_position_y: i32,
        dm_display_orientation: u32,
        dm_display_fixed_output: u32,
        dm_color: i16,
        dm_duplex: i16,
        dm_y_resolution: i16,
        dm_tt_option: i16,
        dm_collate: i16,
        dm_form_name: [u16; 32],
        dm_log_pixels: u16,
        dm_bits_per_pel: u32,
        dm_pels_width: u32,
        dm_pels_height: u32,
        dm_display_flags: u32,
        dm_display_frequency: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumDisplaySettingsW(
            device_name: *const u16,
            mode_num: u32,
            dev_mode: *mut DevModeW,
        ) -> i32;
    }

    pub fn query_display_info() -> Option<(u32, u32, u32)> {
        let mut dev_mode: DevModeW = unsafe { std::mem::zeroed() };
        dev_mode.dm_size = size_of::<DevModeW>() as u16;
        let ok =
            unsafe { EnumDisplaySettingsW(std::ptr::null(), ENUM_CURRENT_SETTINGS, &mut dev_mode) };
        if ok == 0 {
            return None;
        }
        Some((
            dev_mode.dm_pels_width,
            dev_mode.dm_pels_height,
            dev_mode.dm_display_frequency,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_display_info_never_panics_and_returns_positive_values() {
        let (width, height, refresh) = current_display_info();
        assert!(width > 0);
        assert!(height > 0);
        assert!(refresh > 0);
    }
}
