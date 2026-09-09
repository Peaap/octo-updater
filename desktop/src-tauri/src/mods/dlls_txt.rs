use std::{fs, path::Path};

/// Ported from the legacy Python updater's `add_dll`/`remove_dll`: a
/// case-insensitive, deduplicated, atomically-written list of DLL names for
/// VanillaFixes' loader to inject. Never exposed to the user as free-text —
/// every mutation goes through `add`/`remove` so the file always reflects
/// exactly the mods this updater has installed, never an orphaned or
/// hand-edited entry it can't account for during uninstall/update.
fn dlls_txt_path(game_directory: &str) -> std::path::PathBuf {
    Path::new(game_directory).join("dlls.txt")
}

/// Registers `dll_name` in `dlls.txt` if it isn't already present
/// (case-insensitive). Writes via a same-directory temp file + rename so a
/// crash or antivirus interruption mid-write can't corrupt the list other
/// mods depend on.
pub fn add_dll(game_directory: &str, dll_name: &str) -> Result<(), String> {
    let path = dlls_txt_path(game_directory);
    let mut lines = read_lines(&path)?;
    if lines.iter().any(|line| line.eq_ignore_ascii_case(dll_name)) {
        return Ok(());
    }
    lines.push(dll_name.to_string());
    write_lines(&path, &lines)
}

/// Removes `dll_name` from `dlls.txt` (case-insensitive), deleting the file
/// entirely once it would otherwise be empty — matching the legacy updater's
/// behavior of never leaving a zero-mod empty `dlls.txt` around.
pub fn remove_dll(game_directory: &str, dll_name: &str) -> Result<(), String> {
    let path = dlls_txt_path(game_directory);
    if !path.exists() {
        return Ok(());
    }
    let lines: Vec<String> = read_lines(&path)?
        .into_iter()
        .filter(|line| !line.eq_ignore_ascii_case(dll_name))
        .collect();
    if lines.is_empty() {
        fs::remove_file(&path)
            .map_err(|error| format!("Could not remove {}: {error}", path.display()))?;
        return Ok(());
    }
    write_lines(&path, &lines)
}

fn read_lines(path: &Path) -> Result<Vec<String>, String> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("Could not read {}: {error}", path.display())),
    }
}

fn write_lines(path: &Path, lines: &[String]) -> Result<(), String> {
    let mut contents = lines.join("\n");
    contents.push('\n');
    let tmp = path.with_extension("txt.tmp");
    fs::write(&tmp, &contents)
        .map_err(|error| format!("Could not stage {}: {error}", path.display()))?;
    fs::rename(&tmp, path)
        .map_err(|error| format!("Could not finalize {}: {error}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("octo-dlls-txt-test-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn add_dll_creates_the_file_when_missing() {
        let dir = temp_dir("create");
        add_dll(dir.to_str().unwrap(), "Foo.dll").unwrap();
        let contents = fs::read_to_string(dir.join("dlls.txt")).unwrap();
        assert_eq!(contents, "Foo.dll\n");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn add_dll_is_idempotent_and_case_insensitive() {
        let dir = temp_dir("idempotent");
        add_dll(dir.to_str().unwrap(), "Foo.dll").unwrap();
        add_dll(dir.to_str().unwrap(), "foo.dll").unwrap();
        add_dll(dir.to_str().unwrap(), "Bar.dll").unwrap();
        let contents = fs::read_to_string(dir.join("dlls.txt")).unwrap();
        assert_eq!(contents, "Foo.dll\nBar.dll\n");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn remove_dll_deletes_the_file_once_empty() {
        let dir = temp_dir("remove-last");
        add_dll(dir.to_str().unwrap(), "Foo.dll").unwrap();
        remove_dll(dir.to_str().unwrap(), "foo.dll").unwrap();
        assert!(!dir.join("dlls.txt").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn remove_dll_keeps_other_entries() {
        let dir = temp_dir("remove-one-of-two");
        add_dll(dir.to_str().unwrap(), "Foo.dll").unwrap();
        add_dll(dir.to_str().unwrap(), "Bar.dll").unwrap();
        remove_dll(dir.to_str().unwrap(), "Foo.dll").unwrap();
        let contents = fs::read_to_string(dir.join("dlls.txt")).unwrap();
        assert_eq!(contents, "Bar.dll\n");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn remove_dll_on_a_missing_file_is_a_no_op() {
        let dir = temp_dir("remove-missing");
        remove_dll(dir.to_str().unwrap(), "Foo.dll").unwrap();
        assert!(!dir.join("dlls.txt").exists());
        fs::remove_dir_all(&dir).ok();
    }
}
