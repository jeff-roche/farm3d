//! Hard-link checks for the private files farm3d opens (the database, its
//! lock file, legacy imports, and the credential fallback). A file with more
//! than one link could be another path's content, so these callers refuse it.

use std::fs::{File, Metadata};

/// Whether open `file` has more than one hard link. Fails closed: a file
/// whose link count can't be read counts as linked.
pub(crate) fn file_has_multiple_links(file: &File) -> bool {
    imp::file_has_multiple_links(file)
}

/// Whether the file `metadata` describes has more than one hard link, from
/// path metadata alone. Only Unix reports a link count this way; Windows
/// needs an open handle, so there it returns false and the caller's
/// handle check ([`file_has_multiple_links`]) is the guard.
pub(crate) fn metadata_has_multiple_links(metadata: &Metadata) -> bool {
    imp::metadata_has_multiple_links(metadata)
}

#[cfg(unix)]
mod imp {
    use std::fs::{File, Metadata};
    use std::os::unix::fs::MetadataExt;

    pub(super) fn file_has_multiple_links(file: &File) -> bool {
        file.metadata()
            .map_or(true, |metadata| metadata_has_multiple_links(&metadata))
    }

    pub(super) fn metadata_has_multiple_links(metadata: &Metadata) -> bool {
        metadata.nlink() > 1
    }
}

#[cfg(windows)]
mod imp {
    use std::fs::{File, Metadata};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    // `std`'s `MetadataExt::number_of_links` is unstable
    // (`windows_by_handle`), so ask the handle directly.
    pub(super) fn file_has_multiple_links(file: &File) -> bool {
        // SAFETY: an all-zero BY_HANDLE_FILE_INFORMATION is a valid value.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the handle stays open for the borrow of `file`, and `info`
        // is a valid, writable out-pointer.
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
        ok == 0 || info.nNumberOfLinks > 1
    }

    pub(super) fn metadata_has_multiple_links(_: &Metadata) -> bool {
        false
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::fs::{File, Metadata};

    pub(super) fn file_has_multiple_links(_: &File) -> bool {
        false
    }

    pub(super) fn metadata_has_multiple_links(_: &Metadata) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_single_link_file_is_not_linked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("only");
        fs::write(&path, b"x").unwrap();
        let file = File::open(&path).unwrap();
        assert!(!file_has_multiple_links(&file));
        assert!(!metadata_has_multiple_links(
            &fs::symlink_metadata(&path).unwrap()
        ));
    }

    #[test]
    fn a_hard_linked_file_is_linked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("first");
        fs::write(&path, b"x").unwrap();
        fs::hard_link(&path, dir.path().join("second")).unwrap();
        let file = File::open(&path).unwrap();
        assert!(file_has_multiple_links(&file));
        #[cfg(unix)]
        assert!(metadata_has_multiple_links(
            &fs::symlink_metadata(&path).unwrap()
        ));
    }
}
