use super::locale::locale_patches;
use super::pristine::read_pristine_or_current;
use super::tweaks::TweaksConfig;
use std::{f64::consts::PI, fs, io::Write, path::Path};

// Fixed WoW.exe offsets patched on top of the pristine base. Ported
// byte-for-byte from the legacy Python updater's `UpdateWorker.build_tweaks`.
// These assume the same 1.12.1-family client the torrent ships; a client
// build/layout change would make these invalid (tracked as a follow-up:
// validate expected pre-patch bytes before writing).
const LARGE_ADDRESS_OFFSET: usize = 0x126;
const FIELD_OF_VIEW_OFFSET: usize = 0x4089b4;
const CAMERA_DISTANCE_OFFSET: usize = 0x4089a4;
const FAR_CLIP_OFFSET: usize = 0x40fed8;
const FRILL_DISTANCE_OFFSET: usize = 0x467958;
const NAMEPLATE_RANGE_OFFSET: usize = 0x40c448;
const SOUND_IN_BACKGROUND_OFFSET: usize = 0x3a4869;
const AUTO_LOOT_OFFSETS: [usize; 2] = [0x0c1ecf, 0x0c2b25];
const OCTOWOW_URL_ALLOWLIST_OFFSET: usize = 0x45ccd8;
const SKILL_UI_GATE_HIJACK_OFFSET: usize = 0x002ddf90;

const OCTOWOW_URL_ALLOWLIST_BYTES: [u8; 16] = [
    0x6f, 0x63, 0x74, 0x6f, 0x77, 0x6f, 0x77, 0x2e, 0x73, 0x74, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

// The official launcher still applies this patch even though it's normally
// baked into the torrent's WoW.exe already; mirrored here in case a future
// build drops it. Opaque machine code — not decoded, only reproduced.
const SKILL_UI_GATE_HIJACK_BYTES: [u8; 176] = [
    0x55, 0x8b, 0xec, 0x83, 0xec, 0x08, 0x53, 0x56, 0x57, 0x8b, 0x3d, 0x60, 0xab, 0xce, 0x00, 0x83,
    0xff, 0xff, 0x89, 0x55, 0xfc, 0x89, 0x4d, 0xf8, 0x74, 0x79, 0x8b, 0x75, 0x08, 0x8b, 0x15, 0x58,
    0xab, 0xce, 0x00, 0x8b, 0xc7, 0x23, 0xc6, 0x8d, 0x04, 0x40, 0x8b, 0x4c, 0x82, 0x08, 0xf6, 0xc1,
    0x01, 0x8d, 0x44, 0x82, 0x04, 0x75, 0x04, 0x85, 0xc9, 0x75, 0x05, 0x33, 0xc9, 0x8d, 0x49, 0x00,
    0xf6, 0xc1, 0x01, 0x75, 0x4e, 0x85, 0xc9, 0x74, 0x4a, 0x39, 0x31, 0x74, 0x13, 0x8b, 0xc7, 0x23,
    0xc6, 0x8d, 0x04, 0x40, 0x8d, 0x04, 0x82, 0x8b, 0x00, 0x03, 0xc1, 0x8b, 0x48, 0x04, 0xeb, 0xe0,
    0x8b, 0x59, 0x1c, 0x8b, 0x71, 0x18, 0x33, 0xff, 0x85, 0xdb, 0x7e, 0x27, 0x8d, 0x64, 0x24, 0x00,
    0x8b, 0x4e, 0x0c, 0x8b, 0x56, 0x08, 0x6a, 0x00, 0x6a, 0x00, 0x51, 0x8b, 0x4d, 0xf8, 0x52, 0x8b,
    0x55, 0xfc, 0xe8, 0xb9, 0xfd, 0xff, 0xff, 0x84, 0xc0, 0x75, 0x13, 0x47, 0x83, 0xc6, 0x20, 0x3b,
    0xfb, 0x7c, 0xdd, 0x5f, 0x5e, 0x33, 0xc0, 0x5b, 0x8b, 0xe5, 0x5d, 0xc2, 0x04, 0x00, 0x5f, 0x8b,
    0xc6, 0x5e, 0x5b, 0x8b, 0xe5, 0x5d, 0xc2, 0x04, 0x00, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90,
];

/// Patches `WoW.exe` for `game_directory` from its cached/current pristine
/// base (see `pristine::read_pristine_or_current`) and writes the result via
/// a same-directory temporary file, so a crash or antivirus interruption
/// mid-write can never leave a truncated executable in place. Returns an
/// error rather than raising past a half-written destination.
pub fn patch_wow_exe(
    app_data_dir: &Path,
    game_directory: &str,
    manifest_sha256: &str,
    tweaks: &TweaksConfig,
) -> Result<(), String> {
    let mut buffer = read_pristine_or_current(app_data_dir, game_directory, manifest_sha256)?;
    apply_tweak_patches(&mut buffer, tweaks)?;

    let exe_path = Path::new(game_directory).join("WoW.exe");
    let tmp_path = exe_path.with_extension("exe.tmp");
    let mut file = fs::File::create(&tmp_path)
        .map_err(|error| format!("Could not stage patched WoW.exe: {error}"))?;
    file.write_all(&buffer)
        .map_err(|error| format!("Could not write patched WoW.exe: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("Could not flush patched WoW.exe: {error}"))?;
    drop(file);
    fs::rename(&tmp_path, &exe_path)
        .map_err(|error| format!("Could not finalize patched WoW.exe: {error}"))?;
    Ok(())
}

fn apply_tweak_patches(buffer: &mut [u8], tweaks: &TweaksConfig) -> Result<(), String> {
    let write_f32 = |buffer: &mut [u8], offset: usize, value: f32| -> Result<(), String> {
        let bytes = value.to_le_bytes();
        buffer
            .get_mut(offset..offset + 4)
            .ok_or_else(|| out_of_range(offset))?
            .copy_from_slice(&bytes);
        Ok(())
    };
    let write_u16 = |buffer: &mut [u8], offset: usize, value: u16| -> Result<(), String> {
        let bytes = value.to_le_bytes();
        buffer
            .get_mut(offset..offset + 2)
            .ok_or_else(|| out_of_range(offset))?
            .copy_from_slice(&bytes);
        Ok(())
    };
    let write_i8 = |buffer: &mut [u8], offset: usize, value: i8| -> Result<(), String> {
        *buffer.get_mut(offset).ok_or_else(|| out_of_range(offset))? = value as u8;
        Ok(())
    };
    let write_bytes = |buffer: &mut [u8], offset: usize, data: &[u8]| -> Result<(), String> {
        buffer
            .get_mut(offset..offset + data.len())
            .ok_or_else(|| out_of_range(offset))?
            .copy_from_slice(data);
        Ok(())
    };

    for (offset, data) in locale_patches(&tweaks.locale) {
        write_bytes(buffer, offset, &data)?;
    }

    let large_address_flags = u16::from_le_bytes([
        *buffer
            .get(LARGE_ADDRESS_OFFSET)
            .ok_or_else(|| out_of_range(LARGE_ADDRESS_OFFSET))?,
        *buffer
            .get(LARGE_ADDRESS_OFFSET + 1)
            .ok_or_else(|| out_of_range(LARGE_ADDRESS_OFFSET))?,
    ]) | 0x20;
    write_u16(buffer, LARGE_ADDRESS_OFFSET, large_address_flags)?;

    write_f32(
        buffer,
        FIELD_OF_VIEW_OFFSET,
        (tweaks.field_of_view * (PI / 180.0)) as f32,
    )?;
    write_f32(
        buffer,
        CAMERA_DISTANCE_OFFSET,
        tweaks.camera_distance as f32,
    )?;
    write_f32(buffer, FAR_CLIP_OFFSET, tweaks.far_clip as f32)?;
    write_f32(buffer, FRILL_DISTANCE_OFFSET, tweaks.frill_distance as f32)?;
    write_f32(
        buffer,
        NAMEPLATE_RANGE_OFFSET,
        tweaks.nameplate_range as f32,
    )?;
    write_i8(
        buffer,
        SOUND_IN_BACKGROUND_OFFSET,
        if tweaks.sound_in_background {
            0x27
        } else {
            0x14
        },
    )?;

    let auto_loot_byte: u8 = if tweaks.always_auto_loot { 0x75 } else { 0x74 };
    for offset in AUTO_LOOT_OFFSETS {
        write_bytes(buffer, offset, &[auto_loot_byte])?;
    }

    write_bytes(
        buffer,
        OCTOWOW_URL_ALLOWLIST_OFFSET,
        &OCTOWOW_URL_ALLOWLIST_BYTES,
    )?;
    write_bytes(
        buffer,
        SKILL_UI_GATE_HIJACK_OFFSET,
        &SKILL_UI_GATE_HIJACK_BYTES,
    )?;
    Ok(())
}

fn out_of_range(offset: usize) -> String {
    format!("WoW.exe is smaller than expected for a client patch at offset 0x{offset:x}. Refusing to patch an unexpected executable.")
}

/// Writes a fresh `Config.wtf`, matching the field set and formatting of the
/// legacy Python updater's `write_config_wtf`. Uses a same-directory
/// temporary file plus atomic rename so a crash or lock mid-write cannot
/// leave a truncated config behind.
pub fn write_config_wtf(
    game_directory: &str,
    tweaks: &TweaksConfig,
    resolution: (u32, u32),
    refresh_rate: u32,
) -> Result<(), String> {
    let fov_radians = round6(tweaks.field_of_view * PI / 180.0);
    let background_sound = if tweaks.sound_in_background { 1 } else { 0 };
    let lines = [
        format!("SET realmList \"octowow.st\""),
        format!("SET patchList \"octowow.st\""),
        format!("SET readTOS \"1\""),
        format!("SET readEULA \"1\""),
        format!("SET profanityFilter \"0\""),
        format!("SET gxResolution \"{}x{}\"", resolution.0, resolution.1),
        format!("SET gxWindow \"1\""),
        format!("SET gxMaximize \"1\""),
        format!("SET gxVSync \"0\""),
        format!("SET gxColorBits \"24\""),
        format!("SET gxDepthBits \"24\""),
        format!("SET gxRefresh \"{refresh_rate}\""),
        format!("SET gxMultisampleQuality \"0\""),
        format!("SET gxMultisample \"2\""),
        format!("SET hwDetect \"0\""),
        format!("SET pixelShaders \"1\""),
        format!("SET M2UsePixelShaders \"1\""),
        format!("SET specular \"1\""),
        format!("SET anisotropic \"16\""),
        format!("SET trilinear \"1\""),
        format!("SET lod \"0\""),
        format!("SET lodDist \"100\""),
        format!("SET texLodBias \"0\""),
        format!("SET shadowLevel \"0\""),
        format!("SET particleDensity \"1\""),
        format!("SET fullAlpha \"1\""),
        format!("SET SmallCull \"0.01\""),
        format!("SET farClip \"{}\"", tweaks.far_clip),
        format!("SET DistCull \"888.8\""),
        format!("SET frillDensity \"128\""),
        format!("SET unitDrawDist \"300\""),
        format!("SET weatherDensity \"3\""),
        format!("SET FoV \"{fov_radians}\""),
        format!("SET NameplateRange \"{}\"", tweaks.nameplate_range),
        format!("SET CameraDistanceMax \"{}\"", tweaks.camera_distance),
        format!("SET cameraDistanceMaxFactor \"1\""),
        format!("SET scriptMemory \"512000\""),
        format!("SET uiScale \"1\""),
        format!("SET mouseSpeed \"1\""),
        format!("SET autoSelfCast \"1\""),
        format!("SET movie \"0\""),
        format!("SET movieSubtitle \"1\""),
        format!("SET checkAddonVersion \"0\""),
        format!("SET minimapZoom \"0\""),
        format!("SET minimapInsideZoom \"0\""),
        format!("SET EnableErrorSpeech \"0\""),
        format!("SET SoundZoneMusicNoDelay \"1\""),
        format!("SET SoundMaxHardwareChannels \"64\""),
        format!("SET SoundSoftwareChannels \"64\""),
        format!("SET UncapSounds \"1\""),
        format!("SET BackgroundSound \"{background_sound}\""),
        format!("SET NP_NameplateDistance \"{}\"", tweaks.nameplate_range),
        format!("SET NP_SpellQueueWindowMs \"150\""),
        format!("SET NP_EnableAuraCastEvents \"1\""),
        format!("SET NP_EnableAutoAttackEvents \"1\""),
        format!("SET NP_EnableSpellStartEvents \"1\""),
        format!("SET NP_EnableSpellGoEvents \"1\""),
        format!("SET NP_EnableSpellHealEvents \"1\""),
        format!("SET NP_QueueCastTimeSpells \"0\""),
        format!("SET NP_QueueInstantSpells \"0\""),
        format!("SET NP_QueueChannelingSpells \"0\""),
        format!("SET NP_QueueTargetingSpells \"0\""),
        format!("SET NP_QueueSpellsOnCooldown \"0\""),
        format!("SET NP_ChatBubbleDistance \"60\""),
        format!("SET NP_ChatBubblesWhisper \"1\""),
        format!("SET NP_ChatBubblesRaid \"1\""),
        format!("SET NP_ChatBubblesBattleground \"1\""),
        format!("SET ChatBubblesParty \"1\""),
    ];

    let wtf_dir = Path::new(game_directory).join("WTF");
    fs::create_dir_all(&wtf_dir)
        .map_err(|error| format!("Could not create WTF directory: {error}"))?;
    let config_path = wtf_dir.join("Config.wtf");
    let tmp_path = config_path.with_extension("wtf.tmp");
    let mut file = fs::File::create(&tmp_path)
        .map_err(|error| format!("Could not stage Config.wtf: {error}"))?;
    for line in &lines {
        writeln!(file, "{line}").map_err(|error| format!("Could not write Config.wtf: {error}"))?;
    }
    file.sync_all()
        .map_err(|error| format!("Could not flush Config.wtf: {error}"))?;
    drop(file);
    fs::rename(&tmp_path, &config_path)
        .map_err(|error| format!("Could not finalize Config.wtf: {error}"))?;
    Ok(())
}

fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::locale::DEFAULT_LOCALE;

    fn sample_tweaks() -> TweaksConfig {
        TweaksConfig {
            locale: DEFAULT_LOCALE.into(),
            always_auto_loot: true,
            nameplate_range: 41.0,
            field_of_view: 110.0,
            far_clip: 777.0,
            frill_distance: 120.0,
            camera_distance: 50.0,
            sound_in_background: true,
        }
    }

    #[test]
    fn apply_tweak_patches_rejects_an_undersized_buffer() {
        let mut buffer = vec![0_u8; 16];
        let error = apply_tweak_patches(&mut buffer, &sample_tweaks())
            .expect_err("undersized buffer must fail");
        assert!(error.contains("Refusing to patch"));
    }

    #[test]
    fn apply_tweak_patches_writes_expected_field_of_view_bytes() {
        // Large enough to cover every fixed offset this function writes,
        // comfortably above the highest one (frillDistance at 0x467958).
        let mut buffer = vec![0_u8; 0x470000];
        apply_tweak_patches(&mut buffer, &sample_tweaks()).unwrap();
        let fov_bytes: [u8; 4] = buffer[FIELD_OF_VIEW_OFFSET..FIELD_OF_VIEW_OFFSET + 4]
            .try_into()
            .unwrap();
        let fov = f32::from_le_bytes(fov_bytes);
        assert!((fov - (110.0_f32 * (std::f32::consts::PI / 180.0))).abs() < 1e-4);
    }

    #[test]
    fn write_config_wtf_round_trips_via_temp_file() {
        let dir = std::env::temp_dir().join(format!("octo-config-wtf-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        write_config_wtf(dir.to_str().unwrap(), &sample_tweaks(), (1920, 1080), 60).unwrap();
        let contents = fs::read_to_string(dir.join("WTF").join("Config.wtf")).unwrap();
        assert!(contents.contains("SET gxResolution \"1920x1080\""));
        assert!(contents.contains("SET farClip \"777\""));
        assert!(!dir.join("WTF").join("Config.wtf.tmp").exists());
        fs::remove_dir_all(&dir).ok();
    }
}
