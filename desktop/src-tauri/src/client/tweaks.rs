use serde::{Deserialize, Serialize};

/// User-configurable client tweaks. Field defaults match the legacy Python
/// updater's `TWEAKS_DEFAULTS`. `field_of_view` has no static default in the
/// original either — it is seeded per-display at first use (see
/// `fov_default_for_display`) and only falls back to 110 here when display
/// enumeration is unavailable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TweaksConfig {
    pub locale: String,
    pub always_auto_loot: bool,
    pub nameplate_range: f64,
    pub field_of_view: f64,
    pub far_clip: f64,
    pub frill_distance: f64,
    pub camera_distance: f64,
    pub sound_in_background: bool,
}

pub const TWEAKS_DEFAULTS: TweaksConfig = TweaksConfig {
    locale: String::new(), // placeholder; use `TweaksConfig::default_for_display` instead of this const directly
    always_auto_loot: true,
    nameplate_range: 41.0,
    field_of_view: 110.0,
    far_clip: 777.0,
    frill_distance: 120.0,
    camera_distance: 50.0,
    sound_in_background: true,
};

// Numeric clamp ranges, ported from `TWEAKS_LIMITS` (min, max).
pub const NAMEPLATE_RANGE_LIMITS: (f64, f64) = (0.0, 41.0);
pub const FIELD_OF_VIEW_LIMITS: (f64, f64) = (90.0, 180.0);
pub const CAMERA_DISTANCE_LIMITS: (f64, f64) = (50.0, 100.0);
pub const FAR_CLIP_LIMITS: (f64, f64) = (100.0, 10_000.0);
pub const FRILL_DISTANCE_LIMITS: (f64, f64) = (0.0, 300.0);

impl TweaksConfig {
    /// Aspect-ratio-appropriate field-of-view default, matching the Python
    /// updater's `fov_default_for_display` piecewise-linear interpolation
    /// between reference (aspect ratio, FOV) points, rounded to the nearest 5.
    pub fn default_field_of_view(aspect_ratio: f64) -> f64 {
        const REFERENCE_POINTS: [(f64, f64); 4] = [
            (4.0 / 3.0, 90.0),
            (16.0 / 9.0, 110.0),
            (21.0 / 9.0, 150.0),
            (32.0 / 9.0, 180.0),
        ];
        if aspect_ratio <= REFERENCE_POINTS[0].0 {
            return REFERENCE_POINTS[0].1;
        }
        if aspect_ratio >= REFERENCE_POINTS[3].0 {
            return REFERENCE_POINTS[3].1;
        }
        for window in REFERENCE_POINTS.windows(2) {
            let (ratio_low, fov_low) = window[0];
            let (ratio_high, fov_high) = window[1];
            if ratio_low <= aspect_ratio && aspect_ratio <= ratio_high {
                let t = (aspect_ratio - ratio_low) / (ratio_high - ratio_low);
                let raw = fov_low + t * (fov_high - fov_low);
                return (raw / 5.0).round() * 5.0;
            }
        }
        110.0
    }

    pub fn default_for_aspect_ratio(aspect_ratio: f64) -> Self {
        Self {
            locale: super::locale::DEFAULT_LOCALE.into(),
            field_of_view: Self::default_field_of_view(aspect_ratio),
            ..TWEAKS_DEFAULTS
        }
    }

    /// Clamps every numeric field to its documented range, so a value loaded
    /// from disk or entered by a user can never exceed what the exe patch or
    /// Config.wtf writer expects.
    pub fn clamped(mut self) -> Self {
        self.nameplate_range = self
            .nameplate_range
            .clamp(NAMEPLATE_RANGE_LIMITS.0, NAMEPLATE_RANGE_LIMITS.1);
        self.field_of_view = self
            .field_of_view
            .clamp(FIELD_OF_VIEW_LIMITS.0, FIELD_OF_VIEW_LIMITS.1);
        self.camera_distance = self
            .camera_distance
            .clamp(CAMERA_DISTANCE_LIMITS.0, CAMERA_DISTANCE_LIMITS.1);
        self.far_clip = self.far_clip.clamp(FAR_CLIP_LIMITS.0, FAR_CLIP_LIMITS.1);
        self.frill_distance = self
            .frill_distance
            .clamp(FRILL_DISTANCE_LIMITS.0, FRILL_DISTANCE_LIMITS.1);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fov_default_matches_known_reference_points() {
        assert_eq!(TweaksConfig::default_field_of_view(4.0 / 3.0), 90.0);
        assert_eq!(TweaksConfig::default_field_of_view(16.0 / 9.0), 110.0);
        assert_eq!(TweaksConfig::default_field_of_view(21.0 / 9.0), 150.0);
        assert_eq!(TweaksConfig::default_field_of_view(32.0 / 9.0), 180.0);
    }

    #[test]
    fn fov_default_clamps_extreme_ratios() {
        assert_eq!(TweaksConfig::default_field_of_view(1.0), 90.0);
        assert_eq!(TweaksConfig::default_field_of_view(10.0), 180.0);
    }

    #[test]
    fn clamped_rejects_out_of_range_values() {
        let mut config = TweaksConfig::default_for_aspect_ratio(16.0 / 9.0);
        config.far_clip = 999_999.0;
        config.nameplate_range = -5.0;
        let clamped = config.clamped();
        assert_eq!(clamped.far_clip, FAR_CLIP_LIMITS.1);
        assert_eq!(clamped.nameplate_range, NAMEPLATE_RANGE_LIMITS.0);
    }
}
