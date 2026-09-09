use std::{
    collections::VecDeque,
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const TORRENT_ROOT_NAME: &str = "client";

#[derive(Debug, Clone)]
pub struct Aria2Options {
    pub executable_path: PathBuf,
    pub staging_directory: PathBuf,
    pub game_directory: PathBuf,
    pub torrent_path: PathBuf,
    pub selected_files: Vec<usize>,
    pub check_integrity: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Aria2Progress {
    pub progress: f64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub bytes_per_second: u64,
    pub checking_integrity: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aria2DownloadLayout {
    pub aria2_directory: PathBuf,
    pub client_root: PathBuf,
    pub writes_directly_to_game: bool,
}

/// Runs aria2 against the exact local torrent stored by an immutable update
/// plan. The caller owns journal transitions and must not invoke this until a
/// plan has been explicitly approved. Returning an error never removes game
/// files; aria2's own resume metadata remains in its app-owned staging folder.
pub fn sync_client(
    options: &Aria2Options,
    mut on_progress: impl FnMut(Aria2Progress),
    mut should_cancel: impl FnMut() -> bool,
) -> Result<Aria2DownloadLayout, String> {
    validate_options(options)?;
    let layout = prepare_download_layout(&options.staging_directory, &options.game_directory)?;
    let mut arguments = vec![
        format!("--dir={}", layout.aria2_directory.display()),
        "--seed-time=0".into(),
        format!(
            "--check-integrity={}",
            options.check_integrity || !layout.writes_directly_to_game
        ),
        "--bt-save-metadata=true".into(),
        "--bt-remove-unselected-file=false".into(),
        "--continue=true".into(),
        "--allow-overwrite=true".into(),
        "--auto-file-renaming=false".into(),
        "--file-allocation=none".into(),
        "--disk-cache=128M".into(),
        "--stream-piece-selector=inorder".into(),
        "--max-tries=0".into(),
        "--retry-wait=5".into(),
        "--bt-stop-timeout=120".into(),
        "--auto-save-interval=15".into(),
        "--summary-interval=1".into(),
        "--human-readable=false".into(),
        "--truncate-console-readout=false".into(),
        "--console-log-level=warn".into(),
        format!("--stop-with-process={}", std::process::id()),
    ];
    if !options.selected_files.is_empty() {
        arguments.push(format!(
            "--select-file={}",
            options
                .selected_files
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    arguments.push(options.torrent_path.display().to_string());

    let mut child = Command::new(&options.executable_path)
        .args(arguments)
        .stdout(Stdio::piped())
        // Drain stderr concurrently: aria2 can emit useful filesystem errors
        // there, and piping without a reader could otherwise block the child.
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start aria2: {error}"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "aria2 did not provide diagnostic output.".to_string())?;
    let stderr_reader = std::thread::spawn(move || {
        let mut output = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut output);
        output
    });
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "aria2 did not provide progress output.".to_string())?;
    let reader = BufReader::new(stdout);
    let mut diagnostics = VecDeque::with_capacity(16);

    for line in reader.lines() {
        if should_cancel() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Update cancelled.".into());
        }
        let line = line.map_err(|error| format!("Could not read aria2 output: {error}"))?;
        if let Some(progress) = parse_progress(&line) {
            on_progress(progress);
        } else {
            if diagnostics.len() == 16 {
                diagnostics.pop_front();
            }
            diagnostics.push_back(line);
        }
    }
    let status = child
        .wait()
        .map_err(|error| format!("Could not wait for aria2: {error}"))?;
    let stderr = stderr_reader.join().unwrap_or_default();
    if status.success() {
        Ok(layout)
    } else {
        let details = diagnostics
            .into_iter()
            .chain(stderr.lines().map(str::to_owned))
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" | ");
        if details.is_empty() {
            Err(format!("aria2 exited with {status}."))
        } else {
            Err(format!("aria2 exited with {status}: {details}"))
        }
    }
}

pub fn parse_progress(line: &str) -> Option<Aria2Progress> {
    let percent_start = line.rfind('(')?;
    let percent_end = line[percent_start..].find("%)")? + percent_start;
    let percent = line[percent_start + 1..percent_end].parse::<f64>().ok()? / 100.0;
    let fraction = line[..percent_start].split_whitespace().last()?;
    let (done, total) = fraction.split_once('/')?;
    let bytes_done = parse_bytes(done)?;
    let bytes_total = parse_bytes(total)?;
    let bytes_per_second = line
        .split("DL:")
        .nth(1)
        .and_then(|suffix| suffix.split_whitespace().next())
        .map(|token| token.trim_end_matches(']'))
        .and_then(parse_bytes)
        .unwrap_or(0);
    Some(Aria2Progress {
        progress: percent.clamp(0.0, 1.0),
        bytes_done,
        bytes_total,
        bytes_per_second,
        checking_integrity: line.contains("Checksum:#"),
    })
}

fn validate_options(options: &Aria2Options) -> Result<(), String> {
    for (label, path) in [
        ("aria2 executable", &options.executable_path),
        ("persisted torrent manifest", &options.torrent_path),
    ] {
        if !path.is_file() {
            return Err(format!("The {label} is missing: {}", path.display()));
        }
    }
    if !options.game_directory.is_dir() {
        return Err(format!(
            "The game directory is unavailable: {}",
            options.game_directory.display()
        ));
    }
    Ok(())
}

fn prepare_download_layout(
    staging_directory: &Path,
    game_directory: &Path,
) -> Result<Aria2DownloadLayout, String> {
    fs::create_dir_all(staging_directory)
        .map_err(|error| format!("Could not create aria2 staging directory: {error}"))?;
    let client_root = staging_directory.join(TORRENT_ROOT_NAME);
    let target = game_directory
        .canonicalize()
        .map_err(|error| format!("Could not resolve game directory: {error}"))?;

    match fs::symlink_metadata(&client_root) {
        Ok(_) => match client_root.canonicalize() {
            Ok(current) if current == target => {
                return Ok(Aria2DownloadLayout {
                    aria2_directory: staging_directory.into(),
                    client_root: target,
                    writes_directly_to_game: true,
                });
            }
            Ok(_) if client_root.is_dir() => {
                // A prior no-link update left its verified payload here. This
                // path is under the updater-owned staging directory; aria2 can
                // safely resume it without writing to the game directory.
                return Ok(Aria2DownloadLayout {
                    aria2_directory: staging_directory.into(),
                    client_root,
                    writes_directly_to_game: false,
                });
            }
            Ok(_) => {
                return Err(format!(
                    "Refusing to replace an unexpected aria2 staging entry {}.",
                    client_root.display()
                ));
            }
            Err(error) => {
                fs::remove_dir(&client_root).map_err(|remove_error| {
                    format!(
                        "Could not resolve or remove stale aria2 staging entry {}: {error}; {remove_error}",
                        client_root.display()
                    )
                })?;
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "Could not inspect aria2 staging entry {}: {error}",
                client_root.display()
            ));
        }
    }

    let junction_error = create_windows_link("/J", &client_root, &target).err();
    if junction_error.is_none() && client_root.is_dir() {
        return Ok(Aria2DownloadLayout {
            aria2_directory: staging_directory.into(),
            client_root: target,
            writes_directly_to_game: true,
        });
    }
    let _ = fs::remove_dir(&client_root);

    // Junctions require a local NTFS target. A directory symlink also works
    // for supported network/virtual filesystems, but Windows may require
    // Developer Mode or the Create symbolic links privilege for this fallback.
    let symlink_error = create_windows_link("/D", &client_root, &target).err();
    if symlink_error.is_none() && client_root.is_dir() {
        return Ok(Aria2DownloadLayout {
            aria2_directory: staging_directory.into(),
            client_root: target,
            writes_directly_to_game: true,
        });
    }
    let _ = fs::remove_dir(&client_root);

    // The game filesystem cannot be linked. Keep aria2's fixed `client`
    // torrent root as ordinary, updater-owned staging instead. The executor
    // verifies every selected file here before atomically applying it to the
    // game directory, so an interrupted download never alters the client.
    fs::create_dir_all(&client_root).map_err(|error| {
        format!(
            "Could not create private aria2 download staging at {} after link attempts failed (junction: {}; symlink: {}): {error}",
            client_root.display(),
            junction_error.unwrap_or_else(|| "the link was not usable".into()),
            symlink_error.unwrap_or_else(|| "the link was not usable".into()),
        )
    })?;
    Ok(Aria2DownloadLayout {
        aria2_directory: staging_directory.into(),
        client_root,
        writes_directly_to_game: false,
    })
}

fn create_windows_link(kind: &str, link: &Path, target: &Path) -> Result<(), String> {
    let command = format!(
        "mklink {kind} \"{}\" \"{}\"",
        link.display(),
        target.display()
    );
    let output = Command::new("cmd")
        .args(["/D", "/S", "/C", &command])
        .output()
        .map_err(|error| format!("could not run mklink: {error}"))?;
    if output.status.success() {
        return Ok(());
    }

    let details = [
        String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    ]
    .into_iter()
    .filter(|message| !message.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    Err(if details.is_empty() {
        format!("mklink {kind} exited with {}", output.status)
    } else {
        details
    })
}

fn parse_bytes(value: &str) -> Option<u64> {
    const UNITS: [(&str, f64); 5] = [
        ("TiB", 1_099_511_627_776.0),
        ("GiB", 1_073_741_824.0),
        ("MiB", 1_048_576.0),
        ("KiB", 1_024.0),
        ("B", 1.0),
    ];
    let value = value.trim();
    let (number, multiplier) = UNITS.iter().find_map(|(unit, multiplier)| {
        value
            .strip_suffix(unit)
            .and_then(|number| number.parse::<f64>().ok())
            .map(|number| (number, multiplier))
    })?;
    if number.is_sign_negative() || !number.is_finite() {
        return None;
    }
    let bytes = number * multiplier;
    (bytes <= u64::MAX as f64).then_some(bytes.round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_normal_progress_line() {
        let line = "[#1a2b3c 1.2GiB/8.8GiB(13%) CN:4 DL:5.0MiB]";
        let progress = parse_progress(line).expect("line should parse");
        assert!((progress.progress - 0.13).abs() < 1e-9);
        assert_eq!(
            progress.bytes_done,
            (1.2_f64 * 1_073_741_824.0).round() as u64
        );
        assert_eq!(
            progress.bytes_total,
            (8.8_f64 * 1_073_741_824.0).round() as u64
        );
        assert_eq!(
            progress.bytes_per_second,
            (5.0_f64 * 1_048_576.0).round() as u64
        );
        assert!(!progress.checking_integrity);
    }

    #[test]
    fn parses_a_checksum_progress_line() {
        let line = "[#f66a3e 236MiB/1.5GiB(15%) Checksum:#f66a3e 236MiB/1.5GiB(15%)]";
        let progress = parse_progress(line).expect("checksum line should parse");
        assert!(progress.checking_integrity);
    }

    #[test]
    fn ignores_lines_without_a_progress_fraction() {
        assert!(parse_progress("some unrelated aria2 log line").is_none());
    }

    #[test]
    fn parse_bytes_rejects_negative_and_non_finite_values() {
        assert_eq!(parse_bytes("-1.0MiB"), None);
        assert_eq!(parse_bytes("NaNMiB"), None);
        assert_eq!(parse_bytes("1.0MiB"), Some(1_048_576));
    }

    #[test]
    fn validate_options_rejects_missing_executable() {
        let options = Aria2Options {
            executable_path: PathBuf::from("does-not-exist.exe"),
            staging_directory: PathBuf::from("."),
            game_directory: PathBuf::from("."),
            torrent_path: PathBuf::from("does-not-exist.torrent"),
            selected_files: vec![],
            check_integrity: false,
        };
        let error =
            validate_options(&options).expect_err("missing executable should fail validation");
        assert!(error.contains("aria2 executable"));
    }
}
