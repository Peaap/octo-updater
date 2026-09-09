use super::archive::{Compression, MpqArchive};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

/// One file to place inside a packaged MPQ, keyed by its in-archive path
/// (using `\` separators, e.g. `Interface\AddOns\Foo\Foo.lua`).
pub struct PackedFile {
    pub archived_name: String,
    pub bytes: Vec<u8>,
}

/// Result of packaging a mod's files into an MPQ.
#[derive(Debug)]
pub struct PackedMpq {
    pub sha256: String,
    pub file_count: usize,
}

/// Builds a new MPQ archive from `files` and atomically installs it at
/// `destination` (same-directory temp file + rename, so a crash or
/// antivirus interruption mid-write never leaves a half-written or
/// zero-byte patch MPQ where the game expects a valid one).
///
/// This is how custom mods are packaged for this updater: instead of the
/// client torrent shipping mod DLLs/files directly, each mod's fetched
/// content is packed into its own patch MPQ (via the vendored StormLib) and
/// dropped into the client's `Data` folder, verified independently of the
/// game-client sync.
pub fn pack_mpq(destination: &Path, files: &[PackedFile]) -> Result<PackedMpq, String> {
    if files.is_empty() {
        return Err("Refusing to package an MPQ with no files.".into());
    }
    let staging_path = destination.with_extension("mpq.building");
    if staging_path.exists() {
        fs::remove_file(&staging_path)
            .map_err(|error| format!("Could not clear a stale MPQ staging file: {error}"))?;
    }

    let result = (|| {
        // Reserve headroom above the exact file count: StormLib's hash table
        // size must be a power of two, and a tightly-sized table degrades
        // add/lookup performance and can reject files during table growth.
        let max_file_count = (files.len() as u32).next_power_of_two().max(4) * 2;
        let mut archive = MpqArchive::create(&staging_path, max_file_count)?;
        for file in files {
            archive.add_file_bytes(&file.archived_name, &file.bytes, Compression::Zlib)?;
        }
        drop(archive); // flush + close before hashing the bytes on disk
        let bytes = fs::read(&staging_path)
            .map_err(|error| format!("Could not read the staged MPQ: {error}"))?;
        Ok(format!("{:x}", Sha256::digest(&bytes)))
    })();

    match result {
        Ok(sha256) => {
            fs::rename(&staging_path, destination).map_err(|error| {
                format!(
                    "Could not finalize the packaged MPQ at {}: {error}",
                    destination.display()
                )
            })?;
            Ok(PackedMpq {
                sha256,
                file_count: files.len(),
            })
        }
        Err(error) => {
            let _ = fs::remove_file(&staging_path);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_files_into_a_readable_mpq() {
        let destination =
            std::env::temp_dir().join(format!("octo-mpq-pack-test-{}.mpq", std::process::id()));
        let _ = fs::remove_file(&destination);
        let files = vec![
            PackedFile {
                archived_name: "a.txt".into(),
                bytes: b"one".to_vec(),
            },
            PackedFile {
                archived_name: "dir\\b.txt".into(),
                bytes: b"two".to_vec(),
            },
        ];

        let packed = pack_mpq(&destination, &files).unwrap();
        assert_eq!(packed.file_count, 2);
        assert!(destination.is_file());
        assert!(!destination.with_extension("mpq.building").exists());

        let archive = MpqArchive::open(&destination).unwrap();
        assert_eq!(archive.read_file_bytes("a.txt").unwrap(), b"one");
        assert_eq!(archive.read_file_bytes("dir\\b.txt").unwrap(), b"two");
        drop(archive);

        let actual_sha256 = format!("{:x}", Sha256::digest(fs::read(&destination).unwrap()));
        assert_eq!(actual_sha256, packed.sha256);

        fs::remove_file(&destination).ok();
    }

    #[test]
    fn refuses_to_package_zero_files() {
        let destination =
            std::env::temp_dir().join(format!("octo-mpq-pack-empty-{}.mpq", std::process::id()));
        let error = pack_mpq(&destination, &[]).expect_err("packaging with no files must fail");
        assert!(error.contains("no files"));
        assert!(!destination.exists());
    }

    #[test]
    fn leaves_no_partial_file_when_an_add_fails() {
        // A NUL byte in the archived name is rejected by `to_mpq_cstring`
        // partway through packaging; the destination must not exist after.
        let destination =
            std::env::temp_dir().join(format!("octo-mpq-pack-fail-{}.mpq", std::process::id()));
        let files = vec![
            PackedFile {
                archived_name: "ok.txt".into(),
                bytes: b"ok".to_vec(),
            },
            PackedFile {
                archived_name: "bad\0.txt".into(),
                bytes: b"bad".to_vec(),
            },
        ];
        let error = pack_mpq(&destination, &files)
            .expect_err("packaging must fail on an invalid archived name");
        assert!(error.contains("NUL"));
        assert!(!destination.exists());
        assert!(!destination.with_extension("mpq.building").exists());
    }
}
