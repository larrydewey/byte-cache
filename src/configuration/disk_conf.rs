use std::path::PathBuf;

use const_default::ConstDefault;

use crate::fs::{FsCache, ReadWrite};

use super::*;

/// Configuration for the disk-based cache storage component.
///
/// This struct defines the settings for the disk cache, including the
/// storage path and maximum number of items to manage.
///
/// # Examples
///
/// Basic configuration with system temporary directory:
/// ```rust
/// use byte_cache::configuration::DiskCfg;
/// use std::env::temp_dir;
///
/// let disk_cfg = DiskCfg {
///     disabled: false,
///     path: Some(temp_dir().join("byte_cache").to_string_lossy().to_string()),
///     items: Some(1000),
/// };
/// ```
///
/// Configuration with disabled disk cache:
/// ```rust
/// use byte_cache::configuration::DiskCfg;
///
/// let disk_cfg = DiskCfg {
///     disabled: true,
///     path: None,
///     items: None,
/// };
/// ```
#[derive(ConstDefault, Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskCfg {
    /// Whether the disk cache is disabled
    #[serde(default)]
    pub disabled: bool,
    /// Optional path to the directory where cached items will be stored
    pub path: Option<String>,
    /// Maximum number of items to store in the disk cache
    pub items: Option<usize>,
}

impl DiskCfg {
    /// Converts the disk configuration into a filesystem cache instance.
    ///
    /// This method initializes a read-write filesystem cache based on the
    /// configuration settings, creating the cache directory if it doesn't exist.
    ///
    /// # Returns
    /// * `Ok(FsCache<ReadWrite>)`: The initialized filesystem cache
    /// * `Err(std::io::Error)`: If cache creation failed or the configuration is invalid
    ///
    /// # Errors
    /// This method returns an error in the following cases:
    /// - If the disk cache is disabled
    /// - If the path or item limit is not specified
    /// - If the filesystem cache initialization fails
    ///
    /// # Example
    /// ```rust,no_run
    /// use omnecache::configuration::DiskCfg;
    ///
    /// async fn create_cache() -> std::io::Result<()> {
    ///     let cfg = DiskCfg {
    ///         disabled: false,
    ///         path: Some("/tmp/cache".to_string()),
    ///         items: Some(1000),
    ///     };
    ///     
    ///     let fs_cache = cfg.as_fs_cache().await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn as_fs_cache(&self) -> std::io::Result<FsCache<ReadWrite>> {
        if self.disabled {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Disk cache is disabled",
            ));
        }

        if let (Some(path), Some(items)) = (self.path.clone(), self.items) {
            FsCache::new_write(PathBuf::from(path), items).await
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Disk cache path or items not specified",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disk_cfg() {
        let cfg = DiskCfg {
            disabled: true,
            path: Some("cache".to_string()),
            items: Some(100),
        };
        assert_eq!(cfg.path, Some("cache".to_string()));
        assert_eq!(cfg.items, Some(100));
    }

    #[test]
    fn test_disk_cfg_default() {
        let cfg = DiskCfg::DEFAULT;
        assert_eq!(cfg.path, None);
        assert_eq!(cfg.items, None);
    }

    #[test]
    fn test_serialize_to_toml() {
        let cfg = DiskCfg {
            disabled: true,
            path: Some("cache".to_string()),
            items: Some(100),
        };
        let toml_str = toml::to_string(&cfg).unwrap();
        assert!(toml_str.contains("path = \"cache\""));
        assert!(toml_str.contains("items = 100"));
    }

    #[test]
    fn test_deserialize_from_toml() {
        let toml_str = r#"
            path = "cache"
            items = 100
        "#;
        let cfg: DiskCfg = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.disabled, false);
        assert_eq!(cfg.path, Some("cache".to_string()));
        assert_eq!(cfg.items, Some(100));
    }

    #[test]
    fn test_deserialize_from_toml_disabled() {
        let toml_str = "disabled = true";
        let cfg: DiskCfg = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.disabled, true);
        assert_eq!(cfg.path, None);
        assert_eq!(cfg.items, None);
    }
}
