//! # OmneCache Configuration
//!
//! Configuration components for building and customizing OmneCache instances.
//!
//! This module provides a flexible builder pattern for constructing OmneCache instances
//! with various configuration options. It enables precise control over:
//!
//! * Memory cache settings (capacity, enabled/disabled)
//! * Disk cache settings (capacity, storage path)
//! * Sideload cache settings (capacity, content path)
//!
//! ## Configuration Components
//!
//! * [`OmneCacheCfg`]: Main structure representing the configuration for a OmneCache.
//! * [`MemoryCfg`]: Settings for the in-memory LRU cache
//! * [`DiskCfg`]: Settings for the persistent disk cache
//! * [`SideloadCfg`]: Settings for the sideloaded content cache
//!
//! ## Serialization Support
//!
//! All configuration components implement the [`Configurable`] trait, which provides
//! methods for loading and saving configurations to TOML files. This enables easy
//! persistence and sharing of cache configurations.
//!
//! ## Example
//!
//! ```rust,no_run,ignore
//! use byte_cache::OmneCache;
//! use byte_cache::configuration::{OmneCacheCfg, Configurable, MemoryCfg, DiskCfg, SideloadCfg};
//! use std::path::PathBuf;
//!
//! // Create a new configuration
//! let cfg = OmneCacheCfg {
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
//!
//! // Save the configuration
//! cfg.write_cfg(None).await.expect("Failed to save configuration");
//!
//! // Load a saved configuration
//! let loaded_config = OmneCacheCfg::load_cfg(None).await.expect("Failed to load configuration");
//!
//! // Alternative: build the cache directly
//! // Note: In the real implementation, you would use OmneCache::from_config() or similar
//! let cache = OmneCache::try_from(OmneCacheCfg {
//!     memory: Some(MemoryCfg {
//!         disabled: false,
//!         items: Some(2000),
//!     }),
//!     disk: Some(DiskCfg {
//!         disabled: false,
//!         path: Some("/var/cache/SomeOmneCacheApp/evidence".into()),
//!        items: Some(10000),
//!   }),
//!   sideload: Some(SideloadCfg {
//!        disabled: false,
//!        path: Some("/var/sideload/SomeOmneCacheApp/evidence".into()),
//!       items: Some(5000),
//!   }),
//! }).unwrap();
//! ```

/// Configuration modules for the OmneCache system
mod disk_conf;
mod memory_conf;
mod sideload_conf;

use const_default::ConstDefault;
pub use disk_conf::*;
pub use memory_conf::*;
use serde::{Deserialize, Serialize};
pub use sideload_conf::*;

/// Builder pattern implementation for constructing a OmneCache with custom configuration.
///
/// This struct allows for flexible configuration of memory caching, disk caching,
/// and sideloaded content caching. Each component can be enabled or disabled
/// and configured independently.
#[derive(ConstDefault, Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OmneCacheCfg {
    /// Configuration for the in-memory LRU cache
    pub memory: Option<MemoryCfg>,
    /// Configuration for sideloaded content cache
    pub sideload: Option<SideloadCfg>,
    /// Configuration for persistent disk storage
    pub disk: Option<DiskCfg>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte_cache_builder() {
        let builder = OmneCacheCfg {
            memory: Some(MemoryCfg {
                disabled: false,
                items: Some(2000),
            }),
            disk: Some(DiskCfg {
                disabled: false,
                path: Some("/var/cache/SomeOmneCacheApp/evidence".into()),
                items: Some(10000),
            }),
            sideload: Some(SideloadCfg {
                disabled: false,
                path: Some("/var/sideload/SomeOmneCacheApp/evidence".into()),
                items: Some(5000),
            }),
        };

        assert_eq!(builder.memory.is_some(), true);
        assert_eq!(builder.disk.is_some(), true);
        assert_eq!(builder.sideload.is_some(), true);
    }

    #[tokio::test]
    async fn test_serialize_to_toml() {
        let cfg = OmneCacheCfg {
            memory: Some(MemoryCfg {
                disabled: true,
                items: Some(2000),
            }),
            disk: Some(DiskCfg {
                disabled: true,
                path: Some("/var/cache/SomeOmneCacheApp/evidence".into()),
                items: Some(10000),
            }),
            sideload: Some(SideloadCfg {
                disabled: true,
                path: Some("/var/sideload/SomeOmneCacheApp/evidence".into()),
                items: Some(5000),
            }),
        };

        let toml_str = toml::to_string(&cfg).unwrap();
        assert!(toml_str.contains("memory"));
        assert!(toml_str.contains("disk"));
        assert!(toml_str.contains("sideload"));
    }

    #[tokio::test]
    async fn test_deserialize_from_toml() {
        let toml_str = r#"
            memory = { items = 2000 }
            disk = { path = "/var/cache/app", items = 10000 }
            sideload = { path = "/var/sideload", items = 5000 }
        "#;

        let cfg: OmneCacheCfg = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.memory.unwrap().disabled, false);
        assert_eq!(cfg.disk.unwrap().path, Some(String::from("/var/cache/app")));
        assert_eq!(
            cfg.sideload.unwrap().path,
            Some(String::from("/var/sideload"))
        );
    }

    #[tokio::test]
    async fn test_desrialize_from_toml() {
        let toml_str = r#"[memory]
items = 2000

[disk]
path = "/var/cache/app"
items = 10000

[sideload]
path = "/var/sideload"
"#;

        let cfg: OmneCacheCfg = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.memory.unwrap().disabled, false);
        assert_eq!(cfg.memory.unwrap().items, Some(2000));
        assert_eq!(
            cfg.disk.clone().unwrap().path,
            Some(String::from("/var/cache/app"))
        );
        assert_eq!(cfg.disk.clone().unwrap().items, Some(10000));
        assert_eq!(
            cfg.sideload.clone().unwrap().path,
            Some(String::from("/var/sideload"))
        );
    }

    #[tokio::test]
    async fn read_memory_only_cfg_from_file() {
        let mem_cfg = include_str!("../../examples/mem_only_cache.toml");

        let cfg: OmneCacheCfg = toml::from_str(mem_cfg).unwrap();
        assert_eq!(cfg.memory.unwrap().disabled, false);
        assert_eq!(cfg.memory.unwrap().items, Some(100));
    }

    #[tokio::test]
    async fn read_disk_only_cfg_from_file() {
        let disk_cfg = include_str!("../../examples/disk_only_cache.toml");

        let cfg: OmneCacheCfg = toml::from_str(disk_cfg).unwrap();
        assert_eq!(
            cfg.disk.clone().unwrap().path,
            Some(String::from("/var/cache/SomeOmneCacheApp/evidence"))
        );
        assert_eq!(cfg.disk.unwrap().items, Some(1000));
    }

    #[tokio::test]
    async fn read_sideload_only_cfg_from_file() {
        let sideload_cfg = include_str!("../../examples/sideload_only_cache.toml");

        let cfg: OmneCacheCfg = toml::from_str(sideload_cfg).unwrap();
        assert_eq!(
            cfg.sideload.clone().unwrap().path,
            Some(String::from("/var/lib/SomeOmneCacheApp/evidence"))
        );
        assert_eq!(cfg.sideload.unwrap().items, None);
    }

    #[tokio::test]
    async fn read_default_cfg_from_file() {
        let default_cfg = include_str!("../../examples/default.toml");

        let cfg: OmneCacheCfg = toml::from_str(default_cfg).unwrap();
        assert_eq!(cfg.memory.unwrap().disabled, false);
        assert_eq!(cfg.memory.unwrap().items, Some(2000));
        assert_eq!(
            cfg.disk.clone().unwrap().path,
            Some(String::from("/var/cache/SomeOmneCacheApp/evidence"))
        );
        assert_eq!(cfg.disk.unwrap().items, Some(10000));
        assert_eq!(
            cfg.sideload.clone().unwrap().path,
            Some(String::from("/var/lib/SomeOmneCacheApp/evidence"))
        );
        assert_eq!(cfg.sideload.unwrap().items, None);
    }
}
