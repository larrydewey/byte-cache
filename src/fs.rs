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

/// The UnlockGuard is used to make sure files are cleaned up if a file-handle
/// falls out of scope for any reason.
struct UnlockGuard<'a>(&'a std::fs::File);

impl Drop for UnlockGuard<'_> {
    fn drop(&mut self) {
        let _ = FileExt::unlock(self.0);
    }
}

impl<T> FsCache<T> {
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
                let _ = UnlockGuard(&file);

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
    async fn create_dir(&self, permissions: u32) -> std::io::Result<()> {
        if self.path.exists() {
            return Ok(());
        }

        let path = self.path.clone();

        // Use spawn_blocking to avoid blocking the async runtime
        tokio::task::spawn_blocking(move || {
            let lock_path = path.join(".directory.lock");

            let dir_lock_file = std::fs::File::open(&lock_path)?;

            let _ = UnlockGuard(&dir_lock_file);

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

    pub async fn write(path: impl Into<PathBuf>, limit: usize) -> std::io::Result<Self> {
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

    pub async fn put(&self, key: &str, data: &[u8]) -> std::io::Result<()> {
        validate_key(key).await?;

        if data.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "You cannot store a key without data.",
            ));
        }

        if !self.path.exists() {
            self.create_dir(0o700).await?;
        } else if std::fs::metadata(&self.path)?.permissions().readonly() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ReadOnlyFilesystem,
                "Incorrect permissions for the disk cache.",
            ));
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
            ));
        }

        tokio::time::timeout(
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
                let _ = UnlockGuard(&key_lock_file);

                FileExt::lock_exclusive(&key_lock_file)?;

                let tmp_file = std::fs::File::open(&tmp_path)?;
                let _ = UnlockGuard(&tmp_file);

                FileExt::lock_exclusive(&tmp_file)?;

                (move || -> std::io::Result<()> {
                    let mut writer = std::io::BufWriter::new(&key_lock_file);
                    writer.write_all(&data)?;
                    writer.flush()?;
                    key_lock_file.sync_all()?;
                    Ok(())
                })()?;

                std::fs::rename(&tmp_path, &file_path)?;

                Ok(())
            }),
        )
        .await??
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
