use super::ffi::{self, HANDLE};
use std::{ffi::CString, path::Path};

/// A safe handle over a StormLib MPQ archive opened for read or write. The
/// archive is always closed (and, for a writer, flushed) when this value is
/// dropped, so a panic or early return can never leave a StormLib handle
/// leaked or an archive unflushed on disk.
pub struct MpqArchive {
    handle: HANDLE,
}

/// Compression to apply to files added via [`MpqArchive::add_file_bytes`].
/// Only ZLIB is exposed for now; StormLib supports several others, but ZLIB
/// is the same compression the legacy Python updater's game-content MPQs use.
#[derive(Debug, Clone, Copy)]
pub enum Compression {
    None,
    Zlib,
}

impl MpqArchive {
    /// Opens an existing MPQ archive for reading.
    pub fn open(path: &Path) -> Result<Self, String> {
        let mpq_path = path_to_cstring(path)?;
        let mut handle: HANDLE = std::ptr::null_mut();
        // Both dwPriority and dwFlags are 0: StormLib's normal read/write
        // default. `SFILE_OPEN_HARD_DISK_FILE` is a legacy dwPriority value,
        // not a dwFlags bit — passing it as flags previously corrupted the
        // open call and crashed inside StormLib.
        let ok = unsafe { ffi::SFileOpenArchive(mpq_path.as_ptr(), 0, 0, &mut handle) };
        if !ok {
            return Err(format!(
                "Could not open MPQ archive {}: {}",
                path.display(),
                last_error()
            ));
        }
        Ok(Self { handle })
    }

    /// Creates a new, empty MPQ v1 archive at `path`, failing if a file
    /// already exists there (the caller is expected to stage into a
    /// temporary path and atomically rename it into place afterward).
    pub fn create(path: &Path, max_file_count: u32) -> Result<Self, String> {
        if path.exists() {
            return Err(format!(
                "Refusing to overwrite an existing file at {}.",
                path.display()
            ));
        }
        let mpq_path = path_to_cstring(path)?;
        let mut handle: HANDLE = std::ptr::null_mut();
        let flags =
            ffi::MPQ_CREATE_ARCHIVE_V1 | ffi::MPQ_CREATE_LISTFILE | ffi::MPQ_CREATE_ATTRIBUTES;
        let ok = unsafe {
            ffi::SFileCreateArchive(mpq_path.as_ptr(), flags, max_file_count, &mut handle)
        };
        if !ok {
            return Err(format!(
                "Could not create MPQ archive {}: {}",
                path.display(),
                last_error()
            ));
        }
        Ok(Self { handle })
    }

    /// Adds `bytes` to the archive under `archived_name` (using MPQ's `\`
    /// path separators). Always sets `MPQ_FILE_REPLACEEXISTING` so re-adding
    /// the same logical file (e.g. rebuilding a patch MPQ) does not require a
    /// separate remove step.
    pub fn add_file_bytes(
        &mut self,
        archived_name: &str,
        bytes: &[u8],
        compression: Compression,
    ) -> Result<(), String> {
        let source_path = write_temp_source(bytes)?;
        let result = (|| {
            let source_cstring = path_to_cstring(&source_path)?;
            let archived_cstring = to_mpq_cstring(archived_name)?;
            let dw_compression = match compression {
                Compression::None => 0,
                Compression::Zlib => ffi::MPQ_COMPRESSION_ZLIB,
            };
            let flags = if matches!(compression, Compression::None) {
                0
            } else {
                ffi::MPQ_FILE_COMPRESS
            } | ffi::MPQ_FILE_REPLACEEXISTING;
            let ok = unsafe {
                ffi::SFileAddFileEx(
                    self.handle,
                    source_cstring.as_ptr(),
                    archived_cstring.as_ptr(),
                    flags,
                    dw_compression,
                    dw_compression,
                )
            };
            if !ok {
                return Err(format!(
                    "Could not add {archived_name} to MPQ archive: {}",
                    last_error()
                ));
            }
            Ok(())
        })();
        let _ = std::fs::remove_file(&source_path);
        result
    }

    pub fn has_file(&self, archived_name: &str) -> Result<bool, String> {
        let archived_cstring = to_mpq_cstring(archived_name)?;
        Ok(unsafe { ffi::SFileHasFile(self.handle, archived_cstring.as_ptr()) })
    }

    /// Reads a file's full contents out of the archive.
    pub fn read_file_bytes(&self, archived_name: &str) -> Result<Vec<u8>, String> {
        let archived_cstring = to_mpq_cstring(archived_name)?;
        let mut file_handle: HANDLE = std::ptr::null_mut();
        let opened = unsafe {
            ffi::SFileOpenFileEx(
                self.handle,
                archived_cstring.as_ptr(),
                ffi::SFILE_OPEN_FROM_MPQ,
                &mut file_handle,
            )
        };
        if !opened {
            return Err(format!(
                "Could not open {archived_name} inside the MPQ archive: {}",
                last_error()
            ));
        }
        let result = (|| {
            let size = unsafe { ffi::SFileGetFileSize(file_handle, std::ptr::null_mut()) };
            if size == ffi::SFILE_INVALID_SIZE {
                return Err(format!(
                    "Could not determine the size of {archived_name}: {}",
                    last_error()
                ));
            }
            let mut buffer = vec![0_u8; size as usize];
            let mut bytes_read: u32 = 0;
            let read_ok = unsafe {
                ffi::SFileReadFile(
                    file_handle,
                    buffer.as_mut_ptr().cast(),
                    size,
                    &mut bytes_read,
                    std::ptr::null_mut(),
                )
            };
            if !read_ok || bytes_read != size {
                return Err(format!(
                    "Could not read {archived_name} from the MPQ archive: {}",
                    last_error()
                ));
            }
            Ok(buffer)
        })();
        unsafe { ffi::SFileCloseFile(file_handle) };
        result
    }

    /// Lists every real file entry in the archive (skips the `(listfile)`
    /// and `(attributes)` internal bookkeeping entries StormLib may create).
    pub fn list_files(&self) -> Result<Vec<String>, String> {
        let mut names = Vec::new();
        let mask = CString::new("*").unwrap();
        let mut find_data = ffi::SFILE_FIND_DATA::default();
        let find_handle = unsafe {
            ffi::SFileFindFirstFile(self.handle, mask.as_ptr(), &mut find_data, std::ptr::null())
        };
        if find_handle.is_null() {
            // An empty archive (or one with no matching entries) is not an error.
            return Ok(names);
        }
        loop {
            let name = c_char_array_to_string(&find_data.cFileName);
            if !name.starts_with('(') {
                names.push(name);
            }
            let has_more = unsafe { ffi::SFileFindNextFile(find_handle, &mut find_data) };
            if !has_more {
                break;
            }
        }
        unsafe { ffi::SFileFindClose(find_handle) };
        Ok(names)
    }
}

impl Drop for MpqArchive {
    fn drop(&mut self) {
        unsafe {
            ffi::SFileFlushArchive(self.handle);
            ffi::SFileCloseArchive(self.handle);
        }
    }
}

fn last_error() -> String {
    format!("StormLib error code {}", unsafe { ffi::SErrGetLastError() })
}

fn path_to_cstring(path: &Path) -> Result<CString, String> {
    let text = path
        .to_str()
        .ok_or_else(|| format!("Path {} is not valid UTF-8.", path.display()))?;
    CString::new(text)
        .map_err(|_| format!("Path {} contains an embedded NUL byte.", path.display()))
}

fn to_mpq_cstring(name: &str) -> Result<CString, String> {
    if name.contains('\0') {
        return Err("Archived file name contains an embedded NUL byte.".into());
    }
    Ok(CString::new(name.replace('/', "\\")).unwrap())
}

/// StormLib's file-adding API (`SFileAddFileEx`) requires an on-disk source
/// file. In-memory bytes are staged to a short-lived temporary file (deleted
/// immediately after the add call, success or failure) rather than requiring
/// every caller to already have on-disk content to add.
fn write_temp_source(bytes: &[u8]) -> Result<std::path::PathBuf, String> {
    let path = std::env::temp_dir().join(format!(
        "octo-mpq-add-{}-{}.tmp",
        std::process::id(),
        fastrand_like()
    ));
    std::fs::write(&path, bytes)
        .map_err(|error| format!("Could not stage a temporary file for MPQ add: {error}"))?;
    Ok(path)
}

/// A tiny, dependency-free per-call disambiguator (not cryptographically
/// random; only needs to avoid same-process temp-file collisions).
fn fastrand_like() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn c_char_array_to_string(chars: &[std::os::raw::c_char]) -> String {
    let bytes: Vec<u8> = chars
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}
