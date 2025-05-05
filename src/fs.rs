//! # OmneCache File System Operations
//!
//! This module handles file system interactions for the OmneCache system,
//! providing interfaces for reading from and writing to cache storage on disk.
//!
//! It implements two primary access patterns:
//!
//! * **Read-only access**: Used for sideloaded content that should not be modified
//! * **Read-write access**: Used for the persistent disk cache
//!
//! The module uses a marker type pattern to distinguish between these access modes
//! at compile time, ensuring that operations are only performed when appropriate.

use fs2::FileExt;
use std::{
    io::Write,
    os::unix::fs::DirBuilderExt,
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use rand::{Rng, rng};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

// Constants for file operations
const BASE_RETRY_DELAY_MS: u64 = 50;
const MAX_RETRY_DELAY_MS: u64 = 1000; // 1 second max
const DIR_SYNC_RETRY_COUNT: u8 = 3;

/// Marker type for read-only filesystem operations.
///
/// This is used for cache layers that should only read pre-existing data,
/// such as the sideload cache.
pub struct Read(());

/// Marker type for read-write filesystem operations with capacity limit.
///
/// This is used for cache layers that need to both read and write data,
/// such as the disk cache. It includes a limit on the number of items
/// to enforce cache size constraints.
pub struct ReadWrite {
    /// Maximum number of items to store in this cache
    _limit: usize,
    /// Semaphore to control concurrent access and enforce size limits.
    _semaphore: Arc<Semaphore>,
}

// Future work for FsCache<Key, Value, Kind = Read>:
//
// Option A: Custom String type Similar to OSString
// Option B: Bytes type and Hex encode the bytes.

/// File system cache representation.
///
/// This struct represents a directory-based cache on the filesystem,
/// with operations determined by the generic type parameter T.
/// T can be either `Read` (read-only) or `ReadWrite` (read-write).
#[derive(Clone)]
pub struct FsCache<T> {
    /// Path to the directory containing cached items
    path: PathBuf,
    /// Type marker that determines available operations
    _kind: T,
}

impl<T> FsCache<T> {
    pub async fn get(&self, key: &str) -> Option<Vec<u8>> {
        // Validate key first
        if validate_key(key).await.is_err() {
            return None;
        }

        let file_path = self.path.join(key);
        if !file_path.exists() {
            return None;
        }

        // Use blocking task with timeout to ensure we don't block the async runtime indefinitely
        match tokio::time::timeout(
            std::time::Duration::from_secs(5), // 5 second timeout
            tokio::task::spawn_blocking(move || {
                let file = match std::fs::File::open(&file_path) {
                    Ok(f) => f,
                    Err(_) => return None,
                };

                // Use shared lock for reading to prevent reading during writes
                if FileExt::lock_shared(&file).is_err() {
                    return None;
                }

                // Ensure lock is released with a guard pattern
                struct UnlockGuard<'a>(&'a std::fs::File);
                impl Drop for UnlockGuard<'_> {
                    fn drop(&mut self) {
                        let _ = FileExt::unlock(self.0);
                    }
                }
                let _guard = UnlockGuard(&file);

                std::fs::read(&file_path).ok()
            }),
        )
        .await
        {
            Ok(result) => result.unwrap_or(None),
            Err(_) => {
                // Timeout occurred, log the issue but don't propagate the error
                eprintln!("Warning: Read operation timed out for key: {}", key);
                None
            }
        }
    }
}

impl FsCache<Read> {
    /// This method validates that the path exists before creating the cache,
    /// as a read-only cache cannot create its own directory.
    ///
    /// # Parameters
    /// * `path`: Path to the directory containing cached items
    ///
    /// # Returns
    /// * `Ok(FsCache<Read>)`: The created cache instance
    /// * `Err(std::io::Error)`: If the path doesn't exist
    pub async fn read(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path: PathBuf = path.into();

        if path.exists() {
            let fh = std::fs::File::open(&path)?;

            let permission = fh.metadata()?.permissions();

            if permission.readonly() {
                Ok(Self {
                    path,
                    _kind: Read(()),
                })
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Sideload cache must have read-only permissions",
                ))
            }
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Path does not exist",
            ))
        }
    }
}

impl FsCache<ReadWrite> {
    /// Removes stale temporary and lock files from the cache directory.
    ///
    /// This method is called during cache initialization to clean up any files
    /// that might have been left over from interrupted operations:
    /// - `.tmp` files are always removed as they represent incomplete writes
    /// - `.lock` files are removed only if they're considered stale (determined by `is_lock_file_stale`)
    ///
    /// # Parameters
    /// * `path`: The directory path to clean
    ///
    /// # Returns
    /// * `Ok(())`: If cleanup was successful or no cleanup was needed
    /// * `Err(std::io::Error)`: If an error occurred during the cleanup process
    async fn cleanup_temp_files(path: &Path) -> std::io::Result<()> {
        if !path.exists() {
            return Ok(());
        }

        let dir = match path.read_dir() {
            Ok(dir) => dir,
            Err(_) => return Ok(()),
        };

        for entry in dir.filter_map(Result::ok) {
            let file_path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();

            if file_name.ends_with(".tmp") {
                // Always remove temporary files
                if let Err(e) = std::fs::remove_file(&file_path) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            format!("Failed to remove temp file: {}", e),
                        ));
                    }
                }
            } else if file_name.ends_with(".lock") {
                // Only remove stale lock files
                if Self::is_lock_file_stale(&file_path).await {
                    if let Err(e) = std::fs::remove_file(&file_path) {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                format!("Failed to remove stale lock file: {}", e),
                            ));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Determines if a lock file is stale based on its modification time.
    ///
    /// Lock files are considered stale if they're older than 5 minutes,
    /// as this suggests an interrupted or hung operation.
    ///
    /// # Parameters
    /// * `lock_path`: Path to the lock file to check
    ///
    /// # Returns
    /// * `true`: If the lock file is considered stale
    /// * `false`: If the lock file is still valid or cannot be checked
    async fn is_lock_file_stale(lock_path: &Path) -> bool {
        match lock_path.metadata() {
            Ok(metadata) => {
                if let Ok(modified_time) = metadata.modified() {
                    if let Ok(elapsed) = modified_time.elapsed() {
                        // Consider lock files older than 5 minutes as stale
                        return elapsed > std::time::Duration::from_secs(300);
                    }
                }
                false
            }
            Err(_) => false,
        }
    }

    async fn create_dir(&self, permissions: u32) -> std::io::Result<()> {
        // First check if directory exists to avoid unnecessary work
        if self.path.exists() {
            return Ok(());
        }

        let path = self.path.clone();

        // Use spawn_blocking to avoid blocking the async runtime
        tokio::task::spawn_blocking(move || {
            let lock_path = path.join(".directory.lock");

            // Use a more robust locking approach with proper cleanup
            let lock_file = match std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&lock_path)
            {
                Ok(file) => file,
                Err(e) => {
                    return Err(std::io::Error::new(
                        e.kind(),
                        format!("Failed to create lock file: {}", e),
                    ));
                }
            };

            // Try to acquire an exclusive lock, cleaning up properly on failure
            if let Err(e) = lock_file.try_lock_exclusive() {
                // Another process has the lock
                let _ = std::fs::remove_file(&lock_path);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    format!("Directory creation in progress by another process: {}", e),
                ));
            }

            // Use a scoped cleanup to ensure lock file is removed
            let result = {
                let result = std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(permissions)
                    .create(&path);

                // If creation failed because directory now exists (concurrent creation),
                // that's still a success for our purposes
                match result {
                    Ok(_) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                    Err(e) => Err(e),
                }
            };

            // Always unlock and remove the lock file
            let _ = FileExt::unlock(&lock_file);
            let _ = std::fs::remove_file(&lock_path);

            result
        })
        .await?
    }

    pub async fn write(path: impl Into<PathBuf>, limit: usize) -> std::io::Result<Self> {
        let path = path.into();
        let semaphore = Arc::new(Semaphore::new(limit));

        let cache = Self {
            path: path.clone(),
            _kind: ReadWrite {
                _limit: limit,
                _semaphore: semaphore.clone(),
            },
        };

        if !path.exists() {
            cache.create_dir(0o700).await?;
        } else {
            // Clean up any orphaned temp files from previous runs
            Self::cleanup_temp_files(&path).await?;

            if limit > 0 {
                let entries_count = path.read_dir()?.filter_map(Result::ok).count();
                let permits = std::cmp::min(entries_count, limit) as u32;
                if permits > 0 {
                    if let Ok(permit) = semaphore.try_acquire_many_owned(permits) {
                        permit.forget();
                    }
                }
            }
        }

        Ok(cache)
    }

    pub async fn put(&self, key: &str, data: &[u8]) -> std::io::Result<()> {
        // Validate key first
        validate_key(key).await?;

        if !self.path.exists() {
            self.create_dir(0o700).await?;
        } else if tokio::fs::metadata(&self.path)
            .await?
            .permissions()
            .readonly()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ReadOnlyFilesystem,
                "Incorrect permissions for the disk cache.",
            ));
        }

        let file_path = self.path.join(key);
        let semaphore = self._kind._semaphore.clone();
        let limit = self._kind._limit;

        // Execute file IO in blocking task
        let data = data.to_vec();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::task::spawn_blocking(move || {
                let key_lock_path = file_path.with_extension(".lock");
                let check_path = file_path.with_extension(".tmp");

                if check_path.exists() {
                    match std::fs::symlink_metadata(&check_path) {
                        Ok(metadata) => {
                            if metadata.file_type().is_symlink() {
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::InvalidInput,
                                    "Potential symlink attack detected",
                                ));
                            }
                        },
                        Err(_) => {} // If we can't check, we'll create normally and overwrite
                    }
                }

                struct CleanupGuard {
                    lock_path: PathBuf,
                    temp_path: PathBuf,
                    permit: Option<OwnedSemaphorePermit>,
                }

                impl Drop for CleanupGuard {
                    fn drop(&mut self) {
                        let _ = std::fs::remove_file(&self.lock_path);
                        let _ = std::fs::remove_file(&self.temp_path);
                    }
                }

                let mut guard = CleanupGuard {
                    lock_path: key_lock_path.clone(),
                    temp_path: check_path.clone(),
                    permit: None,
                };

                let _ = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&key_lock_path)?;

                const MAX_RETRIES: u8 = 3;
                const RETRY_DELAY_MS: u64 = 50;
                let temp_path = file_path.with_extension(".tmp");

                let need_permit = {
                    let file_exists = std::fs::metadata(&file_path).is_ok();
                    !file_exists && limit > 0
                };

                let permit = if need_permit {
                    match semaphore.try_acquire_owned() {
                        Ok(permit) => Some(permit),
                        Err(_) => {
                            std::fs::remove_file(&key_lock_path)?;

                            return Err(std::io::Error::new(
                                std::io::ErrorKind::StorageFull,
                                format!("Cannot write more than {} items", limit),
                            ));
                        }
                    }
                } else {
                    None
                };

                if !data.is_empty() {
                    // Use statvfs to check available disk space
                    if let Some(parent_dir) = file_path.parent() {
                        if let Ok(stats) = nix::sys::statvfs::statvfs(
                            parent_dir.to_str().unwrap_or(".")
                        ) {
                            let available_space = stats.block_size() * stats.blocks_available();
                            let required_space = data.len() as u64 + 4096; // Data size plus some margin

                            if required_space > available_space {
                                // Clean up resources before returning
                                if let Some(p) = permit {
                                    p.forget();
                                }
                                std::fs::remove_file(&key_lock_path)?;

                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::StorageFull,
                                    format!("Not enough disk space: need {} bytes, have {} bytes", 
                                        required_space, available_space),
                                ));
                            }
                        }
                    }
                }

                for attempt in 1..=MAX_RETRIES {
                    let result = (|| {
                        let file = match std::fs::OpenOptions::new()
                            .write(true)
                            .create(true)
                            .truncate(true)
                            .open(&temp_path)
                            {
                                Ok(file) => file,
                                Err(e) if e.raw_os_error() == Some(24) /* EMFILE: Too many open files */ => {
                                    // Wait briefly and retry once for this specific error
                                    std::thread::sleep(std::time::Duration::from_millis(100));
                                    std::fs::OpenOptions::new()
                                        .write(true)
                                        .create(true)
                                        .truncate(true)
                                        .open(&temp_path)?
                                },
                                Err(e) => return Err(e),
                            };

                        // Set the same permissions as the directory
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let mut perms = file.metadata()?.permissions();
                            perms.set_mode(0o600); // rw for owner only
                            file.set_permissions(perms)?;
                        }

                        // Use a scoped guard pattern for locks too
                        if let Err(e) = file.lock_exclusive() {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::WouldBlock,
                                format!("Failed to lock file: {}", e),
                            ));
                        }

                        (|| -> std::io::Result<()> {
                            let mut writer = std::io::BufWriter::new(&file);
                            writer.write_all(&data)?;
                            writer.flush()?;
                            file.sync_all()?;
                            Ok(())
                        })()?;

                        let unlock_result = FileExt::unlock(&file);

                        if let Err(e) = unlock_result {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                format!("Failed to unlock file: {}", e),
                            ));
                        }

                        file.sync_all()?;

                        // Cross-platform file integrity verification
                        let written_data = std::fs::read(&temp_path)?;
                        if written_data.len() != data.len() {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "Data length mismatch after write",
                            ));
                        }

                        // Verify content hash for data integrity
                        let mut original_hasher = DefaultHasher::new();
                        data.hash(&mut original_hasher);
                        let original_hash = original_hasher.finish();

                        let mut written_hasher = DefaultHasher::new();
                        written_data.hash(&mut written_hasher);
                        let written_hash = written_hasher.finish();

                        if original_hash != written_hash {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "Data integrity check failed after write",
                            ));
                        }

                        match std::fs::rename(&temp_path, &file_path) {
                            Ok(_) => {},
                            Err(e) if e.raw_os_error() == Some(18) /* EXDEV: Cross-device link */ => {
                                // Fallback to copy + unlink for cross-filesystem moves
                                std::fs::copy(&temp_path, &file_path)?;
                                std::fs::remove_file(&temp_path)?;
                            },
                            Err(e) => return Err(e),
                        }

                        // Sync parent directory with exponential backoff for better reliability
                        if let Some(parent_dir) = file_path.parent() {
                            // Calculate backoff with jitter function for retries
                            let calc_backoff_ms = |attempt: u8| -> u64 {
                                let exp_factor = 2_u64.pow(attempt as u32);
                                let base = BASE_RETRY_DELAY_MS * exp_factor;
                                let max = MAX_RETRY_DELAY_MS;

                                let backoff = std::cmp::min(base, max);

                                // Add jitter (±25%)
                                let mut rng = rng();
                                let jitter_factor = 0.75 + (rng.random::<f64>() * 0.5); // 0.75-1.25
                                (backoff as f64 * jitter_factor) as u64
                            };

                            // Try to sync directory multiple times with backoff
                            let mut success = false;
                            for attempt in 1..=DIR_SYNC_RETRY_COUNT {
                                if let Ok(dir) = std::fs::File::open(parent_dir) {
                                    if dir.sync_all().is_ok() {
                                        success = true;
                                        break;
                                    }
                                }

                                if attempt < DIR_SYNC_RETRY_COUNT {
                                    let delay_ms = calc_backoff_ms(attempt);
                                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                                }
                            }

                            if !success {
                                // Log warning but don't fail the operation
                                // Directory sync is a best-effort operation
                                eprintln!("Warning: Could not sync parent directory for {}", file_path.display());
                            }
                        }

                        Ok(())
                    })();

                    if result.is_ok() {
                        guard.permit = permit;
                        std::mem::forget(guard);
                        std::fs::remove_file(&key_lock_path)?;

                        return Ok(());
                    }

                    if let Err(e) = &result {
                        match e.kind() {
                            std::io::ErrorKind::PermissionDenied |
                            std::io::ErrorKind::NotFound |
                            std::io::ErrorKind::InvalidInput |
                            std::io::ErrorKind::AlreadyExists => {
                                // Don't retry on permanent errors
                                std::fs::remove_file(&key_lock_path)?;
                                if let Some(p) = permit {
                                    p.forget();
                                }
                                return result;
                            },
                            _ => {}
                        }
                    }

                    if attempt == MAX_RETRIES {
                        let _ = std::fs::remove_file(&temp_path);

                        if result.is_err() {
                            if let Some(p) = permit {
                                p.forget();
                            }
                        }

                        std::fs::remove_file(&key_lock_path)?;

                        return result;
                    }

                    std::thread::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS));
                }

                Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Failed to write to file",
                ))
            }),
        )
        .await;

        match result {
            Ok(spawn_result) => match spawn_result {
                Ok(io_result) => io_result,
                Err(join_error) => Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Task join error: {}", join_error),
                )),
            },
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "File operation timed out",
            )),
        }
    }
}

/// Validates that a key is safe to use as a filename.
/// Prevents path traversal attacks and invalid filenames.
///
/// This function performs several security checks to ensure the key:
/// - Is not empty
/// - Does not contain path traversal components (like '..' or '/')
/// - Does not contain null characters
/// - Is not excessively long for filesystems
/// - Does not start with a dot (which would make it a hidden file)
///
/// # Parameters
/// * `key`: The string key to validate
///
/// # Returns
/// * `Ok(())`: If the key passes all validation checks
/// * `Err(std::io::Error)`: If any validation check fails, with a descriptive error message
async fn validate_key(key: &str) -> std::io::Result<()> {
    if key.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Empty cache key not allowed",
        ));
    }

    // Check if key length is reasonable for most filesystems
    if key.len() > 255 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Cache key too long, must be fewer than 256 characters",
        ));
    }

    let path = PathBuf::from(key);
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Invalid characters in cache key",
                ));
            }
        }
    }

    if key.contains('/') || key.contains('\0') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid characters in cache key",
        ));
    }

    if key.starts_with('.') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Cache key cannot start with dot",
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fs_cache_read() {
        let cache = FsCache::<Read>::read("test_cache_ro").await.unwrap();
        assert_eq!(cache.path, PathBuf::from("test_cache_ro"));
    }

    #[tokio::test]
    async fn test_fs_cache_read_write() {
        let cache = FsCache::<ReadWrite>::write("test_cache_rw", 100)
            .await
            .unwrap();
        assert_eq!(cache.path, PathBuf::from("test_cache_rw"));
        assert_eq!(cache._kind._limit, 100);
    }

    #[tokio::test]
    async fn test_fs_cache_get() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("test_cache_ro");
        let cache = FsCache::<Read>::read(path).await.unwrap();
        let result = cache.get("key1").await;
        assert!(
            result.is_some(),
            "Expected to find item in cache, but got None"
        );
    }

    #[tokio::test]
    async fn test_fs_cache_put() {
        let cache = FsCache::<ReadWrite>::write("test_cache_rw", 100)
            .await
            .unwrap();
        let result = cache.put("key1", b"Hello, world!").await;
        dbg!(&result);
        assert!(result.is_ok());
    }
}
