//! # OmneCache
//!
//! A flexible multi-layer caching system for byte-oriented data with configurable storage.
//!
//! OmneCache provides a hierarchical caching system with three optional layers:
//!
//! * **Memory**: Fast in-memory LRU cache for recently accessed items
//! * **Sideload**: Pre-loaded content that can be used without downloading
//! * **Disk**: Persistent storage in the filesystem
//!
//! ## Architecture
//!
//! When retrieving data, OmneCache checks each enabled cache layer in order
//! (memory → sideload → disk). If the data is not found
//! in any cache, a `NotFound` error is returned.
//!
//! ## Configuration
//!
//! OmneCache offers a flexible configuration system through the [`configuration`] module,
//! allowing you to:
//!
//! * Enable or disable specific cache layers
//! * Set capacity limits for each layer
//! * Define custom paths for disk and sideload caches
//! * Load and save configurations from/to TOML files
//!
//! ## Example Usage
//!
//! To use OmneCache, implement the [`Cacheable`] trait for your data type and
//! configure the cache layers according to your needs.
//!
//! ```rust,no_run,ignore
//! use byte_cache::{OmneCache, Cacheable, configuration::{OmneCacheCfg, MemoryCfg, DiskCfg, SideloadCfg}};
//! use std::path::PathBuf;
//!
//! // Configure and build a OmneCache instance
//! let cache = OmneCacheCfg {
//!     memory: Some(MemoryCfg {
//!         disabled: false,
//!         items: Some(2000),
//!     }),
//!     disk: Some(DiskCfg {
//!         disabled: false,
//!         path: Some("/var/cache/SomeOmneCacheApp/evidence".into()),
//!         items: Some(10000),
//!     }),
//!     sideload: Some(SideloadCfg {
//!         disabled: false,
//!         path: Some("/var/sideload/SomeOmneCacheApp/evidence".into()),
//!         items: Some(5000),
//!     }),
//! };
//! ```

/// Configuration components for OmneCache's storage layers
pub mod configuration;
/// Error types for OmneCache operations
pub mod error;
/// File system operations for OmneCache
pub mod fs;

use crate::error::*;
use configuration::OmneCacheCfg;
use fs::{FsCache, Read, ReadWrite};
use lru::LruCache;

/// Trait for types which can be retrieved from an external source and stored in a [`OmneCache`].
///
/// This trait is used to define the interface for retrieving data from an external source
/// and storing it in the cache. It allows for custom error handling and
/// deserialization of the cached data.
///
/// Implementors of this trait should provide a method to obtain the data
/// from its original source, as well as a method to deserialize the data
/// from the cache.
///
/// **Note**: This trait inherits its types from [`Cacheable`]. It is not automatically
/// used by `OmneCache.get` - you would need to manually call `fetch()` when handling cache misses.
///
pub trait Request: Cacheable {
    /// Downloads the data from its original external source.
    ///
    /// This method is called when there is a cache miss in all layers
    /// and the data needs to be fetched from its authoritative source.
    fn fetch(&self) -> impl std::future::Future<Output = Result<Vec<u8>, Self::Error>>;
}

/// Trait for types that can be cached by OmneCache.
///
/// Implementors of this trait can be fetched, cached, and retrieved
/// from the OmneCache system. The trait allows for specialized
/// error handling and provides the interface for retrieving data
/// from external sources when cache misses occur.
///
pub trait Cacheable: Clone {
    /// This prefix is used to differentiate between different
    /// cacheable types in the cache system, and should be
    /// unique to the type implementing this trait.
    const PREFIX: &'static str;

    /// The error type returned by operations on this cacheable type
    type Error: From<CacheableError> + From<std::io::Error>;

    /// The value type produced by deserializing cached data
    type Value: TryFrom<Vec<u8>, Error = Self::Error>;

    /// Returns a unique key that identifies the content to be cached.
    ///
    /// This key is used as an identifier in all cache layers.
    /// It's typically structured as a path or identifier string
    /// (e.g., "chain:$vendor:...").
    ///
    /// Note: The key should be a plain string without quotes or special formatting.
    /// When used in the OmneCache system, it will be combined with the PREFIX constant
    /// to form the complete cache key in the format "PREFIX_key".
    fn key(&self) -> impl std::future::Future<Output = String>;
}

/// Multi-layer caching system for byte-oriented data.
///
/// OmneCache provides a hierarchical caching system with three optional layers:
/// 1. Memory: Fast in-memory LRU cache for recently accessed items
/// 2. Sideload: Pre-loaded content that can be used without downloading
/// 3. Disk: Persistent storage in the filesystem
///
/// When retrieving data, OmneCache checks each enabled cache layer in order
/// from fastest to slowest. If the data is not found in any cache, an error
/// is returned.
pub struct OmneCache {
    /// In-memory LRU cache for fast access to recently used items
    memory: Option<LruCache<String, Vec<u8>>>,

    /// Path to the sideloaded content directory
    sideload: Option<FsCache<Read>>,

    /// Path to the disk cache directory
    disk: Option<FsCache<ReadWrite>>,
}

impl OmneCache {
    pub async fn try_from(cfg: OmneCacheCfg) -> Result<Self, ConfigurationError> {
        // Memory cache initialization
        let memory = match cfg.memory {
            Some(memory) if !memory.disabled => Some(memory.lru_cache().await?),
            _ => None,
        };

        // Sideload cache initialization
        let sideload = match cfg.sideload {
            Some(s) => Some(s.as_fs_cache().await?),
            _ => None,
        };

        // Disk cache initialization
        let disk = match cfg.disk {
            Some(d) => Some(d.as_fs_cache().await?),
            _ => None,
        };

        Ok(Self {
            memory,
            disk,
            sideload,
        })
    }
}

impl OmneCache {
    async fn build_key<C: Cacheable>(&self, entry: C) -> String {
        format!("{}_{}", C::PREFIX, entry.key().await)
    }
    /// Attempts to retrieve the requested data from the cache.
    ///
    /// This is the main method of OmneCache, which follows this retrieval sequence:
    /// 1. Check memory cache (if enabled)
    /// 2. Check sideload cache (if enabled)
    /// 3. Check disk cache (if enabled)
    ///
    /// Retrieved data is stored in the appropriate cache layers for future access.
    ///
    /// # Parameters
    /// * `entry`: The Cacheable object that identifies the needed data
    ///
    /// # Returns
    /// * `Ok(C::Value)`: The successfully retrieved and deserialized value
    /// * `Err(C::Error)`: If retrieval or deserialization failed, including when data is not found in any cache
    pub async fn get<C: Cacheable>(&mut self, entry: C) -> Result<C::Value, C::Error> {
        let key: String = self.build_key(entry).await;

        // Check if the memory cache was enabled during construction. If so, check if the data is in memory.
        if let Some(memory) = &mut self.memory {
            if let Some(data) = memory.get(&key) {
                return C::Value::try_from(data.clone());
            }
        }

        // Check if the sideload cache was enabled during construction. If so, check if the data is in the sideload cache.
        if let Some(sideload) = &self.sideload {
            if let Some(data) = sideload.get(&key).await {
                // If the data is found in the sideload cache, but it wasn't found in memory, and the memory cache is enabled, write it to memory.
                if let Some(memory) = &mut self.memory {
                    memory.put(key.clone(), data.clone());
                }

                return C::Value::try_from(data);
            }
        }

        // Check if the disk cache was enabled during construction. If so, check if the data is in the disk cache.
        if let Some(disk) = &self.disk {
            if let Some(data) = disk.get(&key).await {
                // If the data is found in the disk cache, but it wasn't found in memory, and the memory cache is enabled, write it to memory.
                if let Some(memory) = &mut self.memory {
                    memory.put(key.clone(), data.clone());
                }

                return C::Value::try_from(data);
            }
        }

        Err(C::Error::from(CacheableError::NotFound))
    }

    pub async fn put<C: Cacheable>(&mut self, entry: C, value: &[u8]) -> Result<(), C::Error> {
        let key: String = self.build_key(entry).await;

        // Use a sequential approach that prioritizes memory cache first

        // Check if the memory cache was enabled during construction. If so, write to the memory cache.
        if let Some(memory) = &mut self.memory {
            memory.put(key.clone(), value.to_vec());

            // If disk cache is also enabled, asynchronously update it without waiting
            if let Some(disk) = &self.disk {
                // Clone needed data for the async task
                let key_clone = key.clone();
                let value_vec = value.to_vec();
                let disk_clone = disk;

                disk_clone.put(&key_clone, &value_vec).await?
            }

            return Ok(());
        }

        // If memory cache is disabled, write to the disk cache.
        if let Some(disk) = &self.disk {
            return Ok(disk.put(&key, value).await?);
        }

        Err(CacheableError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Unable to write to memory or disk",
        )))?
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::string::String;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    pub struct Bytes(pub Vec<u8>);

    impl TryFrom<Vec<u8>> for Bytes {
        type Error = CacheableError;

        fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
            Ok(Bytes(value))
        }
    }

    impl ToString for Bytes {
        fn to_string(&self) -> String {
            String::from_utf8_lossy(&self.0).to_string()
        }
    }

    impl Cacheable for String {
        const PREFIX: &'static str = "CustomString";

        type Error = CacheableError;
        type Value = Bytes;

        fn key(&self) -> impl std::future::Future<Output = String> {
            // Return the string directly without debug formatting
            // This ensures the key doesn't have quotes when formatted in the cache
            async move { self.clone() }
        }
    }

    impl Request for String {
        fn fetch(&self) -> impl std::future::Future<Output = Result<Vec<u8>, Self::Error>> {
            async move { Ok(self.as_bytes().to_vec()) }
        }
    }

    // Generate a test which will test key collisions.
    #[tokio::test]
    async fn test_key_collision() {
        let mut cache = OmneCache {
            memory: Some(LruCache::new(NonZeroUsize::new(100).unwrap())),
            disk: None,
            sideload: None,
        };

        let key1 = "key1".to_string();
        let key2 = "key2".to_string();

        let something: Result<Bytes, CacheableError> = cache.get(key1.clone()).await;
        assert!(something.is_err());

        cache
            .put("key1".to_string(), "hello world!".as_bytes())
            .await
            .unwrap();

        let memory_len = cache.memory.as_ref().map(|m| m.len()).unwrap_or(0);
        assert!(memory_len == 1);

        assert!(
            cache
                .memory
                .as_ref()
                .unwrap()
                .iter()
                .filter(|(k, _)| **k == format!("{}_{}", String::PREFIX, key1))
                .count()
                == 1
        );

        assert_eq!(
            *cache
                .memory
                .as_mut()
                .unwrap()
                .get(&format!("{}_{}", String::PREFIX, key1))
                .unwrap(),
            b"hello world!".as_slice()
        );

        let something_else = cache.get(key2.clone()).await;
        assert!(something_else.is_err());

        cache
            .put("key2".to_string(), "hello world 2!".as_bytes())
            .await
            .unwrap();

        let something = cache.get(key1.clone()).await.unwrap();
        let something_else = cache.get(key2.clone()).await.unwrap();

        assert_eq!(something, Bytes(b"hello world!".to_vec()));
        assert_eq!(something_else, Bytes(b"hello world 2!".to_vec()));
    }

    #[tokio::test]
    async fn test_insert_duplicate_key() {
        let mut cache = OmneCache {
            memory: Some(LruCache::new(NonZeroUsize::new(100).unwrap())),
            disk: None,
            sideload: None,
        };

        let key = "key".to_string();

        let test = cache.get(key.clone()).await;

        assert!(test.is_err());
        cache.put("key".to_string(), b"hello world!").await.unwrap();

        let memory_len = cache.memory.as_ref().map(|m| m.len()).unwrap_or(0);
        assert_eq!(memory_len, 1);
        assert_eq!(cache.memory.iter().len(), 1);

        let _ = cache.get(key.clone()).await;

        cache
            .put("key".to_string(), b"hello world 2!")
            .await
            .unwrap();

        let memory_len = cache.memory.as_ref().map(|m| m.len()).unwrap_or(0);
        assert_eq!(memory_len, 1);

        assert_eq!(cache.memory.iter().len(), 1);

        assert_eq!(
            cache.get("key".to_string()).await.unwrap(),
            Bytes(b"hello world 2!".to_vec())
        );
    }
}
