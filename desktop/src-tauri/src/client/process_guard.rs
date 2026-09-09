use std::path::Path;

/// Checks whether a given executable's process name currently appears in the
/// Windows process list. Ported from the legacy Python updater's
/// `isGameRunning`/`tasklist` probe (via OctoLauncher's equivalent). Returns
/// an error rather than silently assuming "not running" when the probe itself
/// fails, so a caller can decide whether to block a write as a precaution.
pub fn is_process_running(executable_path: &Path) -> Result<bool, String> {
    let Some(file_name) = executable_path.file_name().and_then(|name| name.to_str()) else {
        return Err("Could not determine an executable name to check.".into());
    };
    if !cfg!(windows) {
        // Outside Windows (Proton/Wine) tasklist doesn't apply; the caller is
        // expected to accept the small residual risk rather than block
        // every operation with no reliable check available.
        return Ok(false);
    }
    let filter = format!("IMAGENAME eq {file_name}");
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &filter, "/FO", "CSV", "/NH"])
        .output()
        .map_err(|error| format!("Could not check whether {file_name} is running: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not check whether {file_name} is running; tasklist exited with {}.",
            output.status
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    Ok(stdout.contains(&format!("\"{}\"", file_name.to_ascii_lowercase())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_path_with_no_file_name() {
        let error = is_process_running(Path::new("")).expect_err("empty path must fail");
        assert!(error.contains("executable name"));
    }
}
