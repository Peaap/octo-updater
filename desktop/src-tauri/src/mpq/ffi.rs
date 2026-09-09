//! Minimal FFI surface over the subset of `StormLib.h` this crate uses. Only
//! the functions and constants actually called by `mpq::archive` are
//! declared here; StormLib's full C API is much larger.
#![allow(non_snake_case, non_camel_case_types)]

use std::os::raw::{c_char, c_void};

pub type HANDLE = *mut c_void;
pub type DWORD = u32;
pub type ULONGLONG = u64;
pub type LCID = u32;

// SFileOpenArchive flags
pub const SFILE_OPEN_HARD_DISK_FILE: DWORD = 2;
pub const MPQ_OPEN_FORCE_LISTFILE: DWORD = 0x00400000;

// SFileCreateArchive flags
pub const MPQ_CREATE_LISTFILE: DWORD = 0x00100000;
pub const MPQ_CREATE_ATTRIBUTES: DWORD = 0x00200000;
pub const MPQ_CREATE_ARCHIVE_V1: DWORD = 0x00000000;

// SFileAddFileEx flags
pub const MPQ_FILE_COMPRESS: DWORD = 0x00000200;
pub const MPQ_FILE_REPLACEEXISTING: DWORD = 0x80000000;

// Compression mask values for SFileAddFileEx (dwCompression)
pub const MPQ_COMPRESSION_ZLIB: DWORD = 0x02;

pub const SFILE_OPEN_FROM_MPQ: DWORD = 0x00000000;

#[repr(C)]
pub struct SFILE_FIND_DATA {
    pub cFileName: [c_char; 260], // MAX_PATH on Windows
    pub szPlainName: *mut c_char,
    pub dwHashIndex: DWORD,
    pub dwBlockIndex: DWORD,
    pub dwFileSize: DWORD,
    pub dwFileFlags: DWORD,
    pub dwCompSize: DWORD,
    pub dwFileTimeLo: DWORD,
    pub dwFileTimeHi: DWORD,
    pub lcLocale: LCID,
}

impl Default for SFILE_FIND_DATA {
    fn default() -> Self {
        // SAFETY: every field is a plain integer or a fixed-size byte array;
        // zeroed bytes are a valid (if meaningless) value for all of them,
        // and StormLib fully populates this struct before it is read.
        unsafe { std::mem::zeroed() }
    }
}

extern "C" {
    pub fn SFileOpenArchive(
        szMpqName: *const c_char,
        dwPriority: DWORD,
        dwFlags: DWORD,
        phMpq: *mut HANDLE,
    ) -> bool;
    pub fn SFileCreateArchive(
        szMpqName: *const c_char,
        dwCreateFlags: DWORD,
        dwMaxFileCount: DWORD,
        phMpq: *mut HANDLE,
    ) -> bool;
    pub fn SFileCloseArchive(hMpq: HANDLE) -> bool;
    pub fn SFileFlushArchive(hMpq: HANDLE) -> bool;

    pub fn SFileAddFileEx(
        hMpq: HANDLE,
        szFileName: *const c_char,
        szArchivedName: *const c_char,
        dwFlags: DWORD,
        dwCompression: DWORD,
        dwCompressionNext: DWORD,
    ) -> bool;
    pub fn SFileRemoveFile(hMpq: HANDLE, szFileName: *const c_char, dwSearchScope: DWORD) -> bool;

    pub fn SFileHasFile(hMpq: HANDLE, szFileName: *const c_char) -> bool;
    pub fn SFileOpenFileEx(
        hMpq: HANDLE,
        szFileName: *const c_char,
        dwSearchScope: DWORD,
        phFile: *mut HANDLE,
    ) -> bool;
    pub fn SFileGetFileSize(hFile: HANDLE, pdwFileSizeHigh: *mut DWORD) -> DWORD;
    pub fn SFileReadFile(
        hFile: HANDLE,
        lpBuffer: *mut c_void,
        dwToRead: DWORD,
        pdwRead: *mut DWORD,
        lpOverlapped: *mut c_void,
    ) -> bool;
    pub fn SFileCloseFile(hFile: HANDLE) -> bool;

    pub fn SFileFindFirstFile(
        hMpq: HANDLE,
        szMask: *const c_char,
        lpFindFileData: *mut SFILE_FIND_DATA,
        szListFile: *const c_char,
    ) -> HANDLE;
    pub fn SFileFindNextFile(hFind: HANDLE, lpFindFileData: *mut SFILE_FIND_DATA) -> bool;
    pub fn SFileFindClose(hFind: HANDLE) -> bool;

    pub fn SErrGetLastError() -> DWORD;
}

pub const SFILE_INVALID_SIZE: DWORD = 0xFFFFFFFF;
