use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::atomic::{AtomicU8, Ordering as AtomicOrdering};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

use super::error::StorageError;
use super::{migrations, snapshot};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct StoragePaths {
    metadata_root: PathBuf,
    database: PathBuf,
    ownership_lock: PathBuf,
    legacy_root: PathBuf,
    snapshot_root: PathBuf,
    content_root: PathBuf,
}

impl StoragePaths {
    pub fn new(
        metadata_root: impl AsRef<Path>,
        app_data_root: impl AsRef<Path>,
    ) -> Result<Self, StorageError> {
        let metadata_root = normalize_absolute(metadata_root.as_ref())?;
        let app_data_root = normalize_absolute(app_data_root.as_ref())?;
        create_private_directory(&metadata_root)?;
        create_private_directory(&app_data_root)?;

        let metadata_root = metadata_root.canonicalize()?;
        let app_data_root = app_data_root.canonicalize()?;
        let legacy_root = create_contained_directory(&metadata_root, Path::new("legacy"))?;
        let snapshot_root = create_contained_directory(&metadata_root, Path::new("snapshots"))?;
        let content_root =
            create_contained_directory(&app_data_root, Path::new("farm3d-content/v1"))?;
        let database = validate_database_path(&metadata_root)?;

        if [&legacy_root, &snapshot_root, &content_root]
            .iter()
            .any(|tree| database.starts_with(tree))
            || trees_overlap(&legacy_root, &snapshot_root)
            || trees_overlap(&legacy_root, &content_root)
            || trees_overlap(&snapshot_root, &content_root)
        {
            return Err(StorageError::PathCollision);
        }

        Ok(Self {
            database,
            ownership_lock: metadata_root.join("farm3d.lock"),
            metadata_root,
            legacy_root,
            snapshot_root,
            content_root,
        })
    }

    pub fn metadata_root(&self) -> &Path {
        &self.metadata_root
    }

    pub fn database(&self) -> &Path {
        &self.database
    }

    pub fn snapshot_root(&self) -> &Path {
        &self.snapshot_root
    }

    pub fn content_root(&self) -> &Path {
        &self.content_root
    }

    pub fn legacy_root(&self) -> &Path {
        &self.legacy_root
    }
}

#[derive(Debug)]
pub struct MetadataRootLease {
    metadata_root: PathBuf,
    _file: File,
}

impl MetadataRootLease {
    pub fn acquire(paths: &StoragePaths) -> Result<Self, StorageError> {
        let file = open_private_lock_file(&paths.ownership_lock)?;
        match file.try_lock() {
            Ok(()) => Ok(Self {
                metadata_root: paths.metadata_root.clone(),
                _file: file,
            }),
            Err(error) => Err(classify_lock_error(error)),
        }
    }
}

pub struct Storage {
    pub(super) paths: StoragePaths,
    writer: Mutex<Connection>,
    snapshot: Mutex<()>,
    failure_point: AtomicU8,
    #[cfg(test)]
    fail_retention_deletion: AtomicBool,
}

impl Storage {
    pub fn open(paths: StoragePaths, lease: &MetadataRootLease) -> Result<Self, StorageError> {
        if paths.metadata_root != lease.metadata_root {
            return Err(StorageError::PersistenceUnavailable);
        }
        snapshot::cleanup_restore_staging(&paths)?;
        if validate_database_path(&paths.metadata_root)? != paths.database {
            return Err(StorageError::PathCollision);
        }
        create_private_file(&paths.database)?;
        let mut writer = Connection::open_with_flags(
            &paths.database,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        configure_connection(&writer)?;
        let journal_mode: String =
            writer.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            return Err(StorageError::PersistenceUnavailable);
        }
        writer.pragma_update(None, "synchronous", "FULL")?;
        let synchronous: i64 = writer.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
        if synchronous != 2 {
            return Err(StorageError::PersistenceUnavailable);
        }
        migrations::apply(&mut writer)?;

        Ok(Self {
            paths,
            writer: Mutex::new(writer),
            snapshot: Mutex::new(()),
            failure_point: AtomicU8::new(0),
            #[cfg(test)]
            fail_retention_deletion: AtomicBool::new(false),
        })
    }

    #[doc(hidden)]
    pub fn inject_failure_once(&self, point: FailurePoint) {
        self.failure_point
            .store(point as u8, AtomicOrdering::SeqCst);
    }

    pub(super) fn take_failure(&self, point: FailurePoint) -> bool {
        self.failure_point
            .compare_exchange(
                point as u8,
                0,
                AtomicOrdering::SeqCst,
                AtomicOrdering::SeqCst,
            )
            .is_ok()
    }

    pub fn read<T>(
        &self,
        operation: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, StorageError> {
        let connection = self.open_reader()?;
        operation(&connection).map_err(StorageError::from)
    }

    pub fn read_transaction<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>,
    ) -> Result<T, StorageError> {
        let mut connection = self.open_reader()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let result = operation(&transaction)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn write<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| StorageError::PersistenceUnavailable)?;
        let transaction = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        match operation(&transaction) {
            Ok(result) => {
                transaction.commit()?;
                Ok(result)
            }
            Err(operation_error) => match transaction.rollback() {
                Ok(()) => Err(operation_error),
                Err(rollback_error) => Err(rollback_error.into()),
            },
        }
    }

    pub(super) fn open_reader(&self) -> Result<Connection, StorageError> {
        let connection = Connection::open_with_flags(
            &self.paths.database,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        configure_connection(&connection)?;
        Ok(connection)
    }

    pub(super) fn lock_snapshot(&self) -> Result<std::sync::MutexGuard<'_, ()>, StorageError> {
        self.snapshot
            .lock()
            .map_err(|_| StorageError::PersistenceUnavailable)
    }

    pub(super) fn remove_retained_snapshot(&self, path: &Path) -> Result<(), StorageError> {
        #[cfg(test)]
        if self.fail_retention_deletion.load(Ordering::SeqCst) {
            return Err(StorageError::Filesystem);
        }
        fs::remove_file(path)?;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn fail_retention_deletion(&self) {
        self.fail_retention_deletion.store(true, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FailurePoint {
    Snapshot = 1,
}

fn configure_connection(connection: &Connection) -> Result<(), StorageError> {
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.busy_timeout(BUSY_TIMEOUT)?;
    Ok(())
}

fn normalize_absolute(path: &Path) -> Result<PathBuf, StorageError> {
    if !path.is_absolute() {
        return Err(StorageError::PathCollision);
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return Err(StorageError::PathCollision);
                }
            }
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
            std::path::Component::Normal(part) => normalized.push(part),
        }
    }
    Ok(normalized)
}

pub(super) fn classify_lock_error(error: std::fs::TryLockError) -> StorageError {
    match error {
        std::fs::TryLockError::Error(error) if error.kind() == std::io::ErrorKind::Unsupported => {
            StorageError::UnsupportedLocking
        }
        std::fs::TryLockError::WouldBlock | std::fs::TryLockError::Error(_) => {
            StorageError::PersistenceUnavailable
        }
    }
}

fn create_contained_directory(base: &Path, relative: &Path) -> Result<PathBuf, StorageError> {
    let mut candidate = base.to_path_buf();
    for component in relative.components() {
        candidate.push(component);
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(StorageError::PathCollision);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&candidate)?;
            }
            Err(error) => return Err(error.into()),
        }
        set_private_directory_permissions(&candidate)?;
    }
    let canonical = candidate.canonicalize()?;
    if canonical.starts_with(base) {
        Ok(canonical)
    } else {
        Err(StorageError::PathCollision)
    }
}

fn validate_database_path(metadata_root: &Path) -> Result<PathBuf, StorageError> {
    let database = metadata_root.join("farm3d.sqlite3");
    match fs::symlink_metadata(&database) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || has_multiple_links(&metadata) =>
        {
            Err(StorageError::PathCollision)
        }
        Ok(_) => Ok(database),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(database),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn has_multiple_links(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink() > 1
}

#[cfg(windows)]
fn has_multiple_links(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.number_of_links().is_some_and(|links| links > 1)
}

#[cfg(not(any(unix, windows)))]
fn has_multiple_links(_: &fs::Metadata) -> bool {
    false
}

fn trees_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

pub(super) fn create_private_directory(path: &Path) -> Result<(), StorageError> {
    fs::create_dir_all(path)?;
    set_private_directory_permissions(path)
}

fn open_private_lock_file(path: &Path) -> Result<File, StorageError> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || has_multiple_links(&metadata) =>
        {
            return Err(StorageError::PersistenceUnavailable);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(StorageError::PersistenceUnavailable),
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options
        .open(path)
        .map_err(|_| StorageError::PersistenceUnavailable)?;
    let metadata = file
        .metadata()
        .map_err(|_| StorageError::PersistenceUnavailable)?;
    if !metadata.is_file() || has_multiple_links(&metadata) {
        return Err(StorageError::PersistenceUnavailable);
    }
    set_private_handle_permissions(&file).map_err(|_| StorageError::PersistenceUnavailable)?;
    Ok(file)
}

pub(super) fn set_private_handle_permissions(file: &File) -> Result<(), StorageError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub(super) fn create_private_file(path: &Path) -> Result<(), StorageError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || has_multiple_links(&metadata) {
        return Err(StorageError::PathCollision);
    }
    set_private_handle_permissions(&file)
}

fn set_private_directory_permissions(path: &Path) -> Result<(), StorageError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StorageError::PathCollision);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::fs::PermissionsExt;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(path)?;
        if !directory.metadata()?.is_dir() {
            return Err(StorageError::PathCollision);
        }
        directory.set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
