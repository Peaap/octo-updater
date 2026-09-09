//! MPQ archive support, backed by a vendored copy of StormLib (MIT license,
//! pinned at tag v9.40; see `vendor/stormlib`). This module exists because
//! custom mods are packaged as standalone MPQ patch archives rather than
//! being bundled with the game-client torrent source.
#[allow(dead_code)]
mod archive;
#[allow(dead_code)]
mod ffi;
#[allow(dead_code)]
mod pack;

#[allow(unused_imports)]
pub use archive::{Compression, MpqArchive};
#[allow(unused_imports)]
pub use pack::{pack_mpq, PackedFile, PackedMpq};

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("octo-mpq-test-{name}-{}.mpq", std::process::id()))
    }

    #[test]
    fn creates_writes_and_reads_back_a_file() {
        let path = temp_path("round-trip");
        let _ = std::fs::remove_file(&path);
        {
            let mut archive = MpqArchive::create(&path, 16).unwrap();
            archive
                .add_file_bytes("Interface\\Test.txt", b"hello mpq", Compression::Zlib)
                .unwrap();
        }

        {
            let archive = MpqArchive::open(&path).unwrap();
            assert!(archive.has_file("Interface\\Test.txt").unwrap());
            let bytes = archive.read_file_bytes("Interface\\Test.txt").unwrap();
            assert_eq!(bytes, b"hello mpq");
        }

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn refuses_to_overwrite_an_existing_file() {
        let path = temp_path("no-overwrite");
        std::fs::write(&path, b"not an mpq").unwrap();
        let error = match MpqArchive::create(&path, 4) {
            Ok(_) => panic!("create must refuse an existing path"),
            Err(error) => error,
        };
        assert!(error.contains("Refusing to overwrite"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn lists_added_files() {
        let path = temp_path("list-files");
        let _ = std::fs::remove_file(&path);
        {
            let mut archive = MpqArchive::create(&path, 16).unwrap();
            archive
                .add_file_bytes("a.txt", b"a", Compression::None)
                .unwrap();
            archive
                .add_file_bytes("sub\\b.txt", b"b", Compression::Zlib)
                .unwrap();
        }
        {
            let archive = MpqArchive::open(&path).unwrap();
            let mut names = archive.list_files().unwrap();
            names.sort();
            assert_eq!(names, vec!["a.txt".to_string(), "sub\\b.txt".to_string()]);
        }
        std::fs::remove_file(&path).ok();
    }
}
