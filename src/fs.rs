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

use crate::{error::CacheableError, result::Result};
use fs2::FileExt;
use nix::sys::resource::{getrlimit, Resource};
use std::{
    io::Write,
    os::unix::fs::DirBuilderExt,
    path::{Component, PathBuf},
};

// Constants for file operations
const LOCK_RETRY_TIMEOUT: u64 = 5;
const WRITE_LOCK_COUNT: usize = 2;

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
}

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

/// The UnlockGuard ensures files are unlocked when they fall out of scope.
///
/// This guard uses RAII (Resource Acquisition Is Initialization) pattern to guarantee
/// that file locks are properly released even when errors occur or execution leaves
/// the current scope. It's a safety mechanism that prevents lock leaks.
///
/// # Example
/// ```rust,no_run
/// use fs2::FileExt;
/// use std::fs::File;
///
/// let file = File::open("some_file").unwrap();
/// let _guard = UnlockGuard(&file);  // Will unlock the file when it goes out of scope
/// FileExt::lock_exclusive(&file).unwrap();
/// // Operations on the locked file
/// // No need to call unlock - the guard will handle it
/// ```
struct UnlockGuard<'a>(&'a std::fs::File);

impl Drop for UnlockGuard<'_> {
    fn drop(&mut self) {
        let _ = FileExt::unlock(self.0);
    }
}

impl<T> FsCache<T> {
    /// Retrieves data from the filesystem cache for the specified key.
    ///
    /// This method first validates the key's format and then attempts to read
    /// the associated file from disk. It uses a shared lock to ensure thread safety
    /// during reads and includes a timeout to prevent blocking the async runtime.
    ///
    /// # Parameters
    /// * `key`: The unique identifier for the data to retrieve
    ///
    /// # Returns
    /// * `Some(Vec<u8>)`: The cached data if found
    /// * `None`: If the key is invalid, the file doesn't exist, or an error occurs during reading
    ///
    /// # Note
    /// This method handles errors internally and returns `None` instead of propagating
    /// them, preferring graceful degradation over error propagation for cache misses.
    pub async fn get(&self, key: &str) -> Option<Vec<u8>> {
        if validate_key(key).await.is_err() {
            return None;
        }

        let file_path = self.path.join(key);

        if !file_path.exists() {
            return None;
        }

        // Use blocking task with timeout to ensure we don't block the async runtime indefinitely
        match tokio::time::timeout(
            std::time::Duration::from_secs(LOCK_RETRY_TIMEOUT), // 5 second timeout
            tokio::task::spawn_blocking(move || {
                let file = match std::fs::File::open(&file_path) {
                    Ok(f) => f,
                    Err(_) => return None,
                };

                // Create the lock guard for the file-handle to protect
                // against a failed lock.
                let _file_guard = UnlockGuard(&file);

                // Use shared lock for reading to prevent reading during writes
                if FileExt::lock_shared(&file).is_err() {
                    return None;
                }

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
    /// Creates a new read-only filesystem cache.
    ///
    /// This method validates that the path exists before creating the cache,
    /// as a read-only cache cannot create its own directory. It also verifies
    /// that the directory has read-only permissions, and acquires a shared file
    /// lock to ensure thread safety during validation.
    ///
    /// # Parameters
    /// * `path`: Path to the directory containing cached items
    ///
    /// # Returns
    /// * `Ok(FsCache<Read>)`: The created cache instance
    /// * `Err(std::io::Error)`: If the path doesn't exist, isn't read-only, or lock acquisition fails
    pub async fn new_read(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path: PathBuf = path.into();

        // Use blocking task with timeout to ensure we don't block the async runtime indefinitely
        tokio::time::timeout(
            std::time::Duration::from_secs(LOCK_RETRY_TIMEOUT),
            tokio::task::spawn_blocking(move || {
                if path.exists() {
                    let fh = std::fs::File::open(&path)?;

                    let _guard = UnlockGuard(&fh);

                    if FileExt::lock_shared(&fh).is_err() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::ResourceBusy,
                            "File could not obtain a lock.",
                        ));
                    }

                    if fh.metadata()?.permissions().readonly() {
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
            }),
        )
        .await??
    }
}

impl FsCache<ReadWrite> {
    /// Creates a directory for the cache with the specified permissions.
    ///
    /// This method ensures the cache directory exists, creating it if necessary.
    /// It uses a lock file to ensure thread safety during directory creation.
    ///
    /// # Parameters
    /// * `permissions`: The Unix permission mode to apply to the created directory
    ///
    /// # Returns
    /// * `Ok(())`: If the directory exists or was successfully created
    /// * `Err(std::io::Error)`: If directory creation failed or if lock acquisition fails
    ///
    /// # Safety
    /// This method creates a lock file, acquires an exclusive lock, creates the directory
    /// if needed, then releases the lock and removes the lock file to clean up.
    async fn create_dir(&self, permissions: u32) -> std::io::Result<()> {
        if self.path.exists() {
            return Ok(());
        }

        let path = self.path.clone();

        // Use spawn_blocking to avoid blocking the async runtime
        tokio::task::spawn_blocking(move || {
            let lock_path = path.join(".directory.lock");

            let dir_lock_file = std::fs::File::open(&lock_path)?;

            let _dir_lock_file_guard = UnlockGuard(&dir_lock_file);

            FileExt::lock_exclusive(&dir_lock_file)?;

            let _ = match std::fs::DirBuilder::new()
                .recursive(true)
                .mode(permissions)
                .create(&path)
            {
                Ok(_) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                Err(e) => Err(e),
            };

            FileExt::unlock(&dir_lock_file)?;
            std::fs::remove_file(&lock_path)?;

            Ok(())
        })
        .await?
    }

    /// Creates a new read-write filesystem cache with the specified capacity limit.
    ///
    /// This constructor initializes a filesystem-based cache that can both read and write data.
    /// It creates the cache directory if it doesn't already exist.
    ///
    /// # Parameters
    /// * `path`: Path to the directory that will contain cached items
    /// * `limit`: Maximum number of items that can be stored in the cache
    ///
    /// # Returns
    /// * `Ok(FsCache<ReadWrite>)`: The created cache instance
    /// * `Err(std::io::Error)`: If directory creation or verification failed
    ///
    /// # Example
    /// ```rust,no_run
    /// use omnecache::fs::{FsCache, ReadWrite};
    ///
    /// async fn create_cache() -> std::io::Result<FsCache<ReadWrite>> {
    ///     FsCache::new_write("cache_dir", 1000).await
    /// }
    /// ```
    pub async fn new_write(path: impl Into<PathBuf>, limit: usize) -> std::io::Result<Self> {
        let path = path.into();

        let cache = Self {
            path: path.clone(),
            _kind: ReadWrite { _limit: limit },
        };

        if !path.exists() {
            cache.create_dir(0o700).await?;
        }

        Ok(cache)
    }

    /// Stores data in the filesystem cache with the provided key.
    ///
    /// This method takes a key and data, validates them, and stores the data
    /// in the filesystem cache. It ensures thread safety during write operations
    /// using file locks and handles file operations safely.
    ///
    /// The method follows these steps:
    /// 1. Validate the key format and check that data is not empty
    /// 2. Check resource limits including file descriptor count on Linux
    /// 3. Ensure write permissions and check cache capacity limits
    /// 4. Create temporary and lock files for thread-safe operations
    /// 5. Use exclusive file locks to protect concurrent operations
    /// 6. Write data to a temporary file and use atomic rename for durability
    ///
    /// # Parameters
    /// * `key`: The unique identifier for the data (must be a valid filename)
    /// * `data`: The byte data to store (must not be empty)
    ///
    /// # Returns
    /// * `Ok(())`: If the data was successfully stored
    /// * `Err(std::io::Error)`: If validation failed or storage operations failed
    ///
    /// # Errors
    /// This method returns an error in the following cases:
    /// - Key validation fails
    /// - The data is empty
    /// - The cache is at capacity
    /// - File descriptor limit is approaching (Linux only)
    /// - Filesystem has read-only permissions
    /// - File locking fails
    /// - File I/O operations fail
    ///
    /// # Security
    /// This method includes protections against symlink attacks and path traversal.
    /// It also uses file locks to prevent race conditions during writes.
    pub async fn put(&self, key: &str, data: &[u8]) -> Result<()> {
        validate_key(key).await?;
        // On Linux check the file-descriptor limit to make sure that
        #[cfg(target_os = "linux")]
        {
            const FD_LIMIT_BUFFER: u64 = 10;
            let (soft_limit, _) = getrlimit(Resource::RLIMIT_NOFILE)?;

            let open_fds = std::fs::read_dir("/proc/self/fd")
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
                .count() as u64;

            if open_fds > soft_limit - FD_LIMIT_BUFFER {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Approaching file descriptor limit",
                ))?;
            }
        }

        if data.is_empty() {
            return Err(CacheableError::EmptyBuffer);
        }

        if !self.path.exists() {
            self.create_dir(0o700).await?;
        } else if std::fs::metadata(&self.path)?.permissions().readonly() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ReadOnlyFilesystem,
                "Incorrect permissions for the disk cache.",
            ))?;
        }

        let file_path = self.path.join(key);
        let data = data.to_vec();

        // Make sure limit is enforced before we create the files.
        if tokio::fs::read_dir(&self.path).await.iter().count()
            >= self._kind._limit - WRITE_LOCK_COUNT
            && self.get(&key).await.is_none()
        {
            // If not, error out. No space left.
            return Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "Cannot exceed cache limit",
            ))?;
        }

        Ok(tokio::time::timeout(
            std::time::Duration::from_secs(LOCK_RETRY_TIMEOUT),
            tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                let key_lock_path = file_path.with_extension(".lock");
                let tmp_path = file_path.with_extension(".tmp");

                if tmp_path.exists() {
                    let metadata = tmp_path.metadata()?;

                    if metadata.is_symlink() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "Potential symlink attack detected",
                        ));
                    }

                    if metadata.permissions().readonly() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "Cannot write to the path provided. Invalid permissions.",
                        ));
                    }
                }

                let key_lock_file = std::fs::File::open(&key_lock_path)?;
                let _key_lock_file_guard = UnlockGuard(&key_lock_file);

                FileExt::lock_exclusive(&key_lock_file)?;

                let tmp_file = std::fs::File::open(&tmp_path)?;
                let _tmp_file_guard = UnlockGuard(&tmp_file);

                FileExt::lock_exclusive(&tmp_file)?;

                (|| -> std::io::Result<()> {
                    let mut writer = std::io::BufWriter::new(&tmp_file);
                    writer.write_all(&data)?;
                    writer.flush()?;
                    tmp_file.sync_all()?;
                    Ok(())
                })()?;

                std::fs::rename(&tmp_path, &file_path)?;

                Ok(())
            }),
        )
        .await???)
    }
}

/// Validates that a key is safe to use as a filename.
/// Prevents path traversal attacks and invalid filenames.
///
/// This function performs several security checks to ensure the key:
/// - Is not empty
/// - Does not contain path traversal components (like '..' or '/')
/// - Does not contain special path components (such as root or parent directory references)
/// - Is not excessively long for filesystems (max 255 characters)
///
/// # Parameters
/// * `key`: The string key to validate
///
/// # Returns
/// * `Ok(())`: If the key passes all validation checks
/// * `Err(std::io::Error)`: If any validation check fails, with a descriptive error message
async fn validate_key(key: &str) -> Result<()> {
    if key.is_empty() {
        return Err(CacheableError::EmptyKey);
    }

    // Check if key length is reasonable for most filesystems
    if key.len() > 255 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Cache key too long, must be fewer than 256 characters",
        ))?;
    }

    let path = PathBuf::from(key);
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Invalid characters in cache key",
                ))?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fs_cache_read() {
        let cache = FsCache::<Read>::new_read("test_cache_ro").await.unwrap();
        assert_eq!(cache.path, PathBuf::from("test_cache_ro"));
    }

    #[tokio::test]
    async fn test_fs_cache_read_write() {
        let cache = FsCache::<ReadWrite>::new_write("test_cache_rw", 100)
            .await
            .unwrap();
        assert_eq!(cache.path, PathBuf::from("test_cache_rw"));
        assert_eq!(cache._kind._limit, 100);
    }

    #[tokio::test]
    async fn test_fs_cache_get() {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("test_cache_ro");
        let cache = FsCache::<Read>::new_read(path).await.unwrap();
        let result = cache.get("key1").await;
        assert!(
            result.is_some(),
            "Expected to find item in cache, but got None"
        );
    }

    #[tokio::test]
    async fn test_fs_cache_put() {
        let cache = FsCache::<ReadWrite>::new_write("test_cache_rw", 100)
            .await
            .unwrap();
        let result = cache.put("key1", b"Hello, world!").await;
        dbg!(&result);
        assert!(result.is_ok());
    }
}
