use std::path::PathBuf;

use const_default::ConstDefault;

use crate::fs::{FsCache, Read};

use super::*;

/// Configuration for the sideload cache component.
///
/// The sideload cache provides pre-loaded content that can be accessed
/// without requiring download from the original source. This is useful
/// for testing, offline operation, or providing fallback content.
#[derive(ConstDefault, Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SideloadCfg {
    /// Whether the sideload cache is enabled
    #[serde(default)]
    pub disabled: bool,
    /// Optional path to the directory containing sideloaded content
    pub path: Option<String>,
    /// Maximum number of items to manage in the sideload cache
    pub(crate) items: Option<usize>,
}

impl SideloadCfg {
    /// Creates a new sideload cache configuration with the specified path.
    ///
    /// # Parameters
    /// * `path`: Optional path to the sideload directory
    ///
    /// # Returns
    /// A new SideloadCfg instance with the specified settings
    ///
    /// # Errors
    /// Returns an error if the path does not exist or is invalid
    ///
    /// # Example
    /// ```rust,ignore
    /// use std::path::PathBuf;
    /// use byte_cache::configuration::SideloadCfg;
    ///
    /// let sideload_cfg = SideloadCfg::new("sideload".to_string()).await;
    /// assert_eq!(sideload_cfg.path, Some("sideload".to_string()));
    /// ```
    ///
    /// # Note
    /// This function checks if the specified path exists and counts the number of items in it.
    /// If the path does not exist, it initializes the items count to 0.
    ///
    /// # See Also
    /// [`FsCache::read`] for reading from the sideload cache.
    /// [`FsCache`] for the cache implementation.
    /// [`Read`] for the read trait.
    /// [`SideloadCfg`] for the sideload cache configuration.
    pub async fn new(path: String) -> std::io::Result<Self> {
        let info: PathBuf = path.clone().into();
        let items = if info.exists() {
            info.read_dir()?.count()
        } else {
            0usize
        };

        Ok(Self {
            disabled: false,
            path: Some(path),
            items: Some(items),
        })
    }

    /// Converts the sideload cfg into a filesystem cache.
    ///
    /// # Returns
    /// A result containing the filesystem cache or an error if the cache is disabled or invalid.
    ///
    /// # Errors
    /// Returns an error if the sideload cache is disabled or the path is invalid.
    ///
    /// # Example
    /// ```rust,ignore
    /// use std::path::PathBuf;
    /// use byte_cache::configuration::SideloadCfg;
    /// use byte_cache::fs::FsCache;
    /// use byte_cache::fs::Read;
    ///
    /// let sideload_cfg = SideloadCfg::new("sideload".to_string()).await;
    /// let fs_cache = sideload_cfg.as_fs_cache().await;
    /// assert!(fs_cache.is_ok());
    /// ```
    ///
    /// # Note
    /// This function checks if the sideload cache is disabled and if the path exists.
    /// If the cache is disabled, it returns an error.
    /// If the path does not exist, it returns an error.
    pub async fn as_fs_cache(&self) -> std::io::Result<FsCache<Read>> {
        if self.disabled {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Sideload cache is disabled",
            ));
        }

        // Get file count from the path.
        if let Some(path) = &self.path {
            let path: PathBuf = path.into();

            if !path.exists() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Sideload cache path does not exist",
                ));
            }

            FsCache::new_read(path).await
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Sideload cache path not specified",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sideload_cfg() {
        let cfg = SideloadCfg {
            disabled: true,
            path: Some("sideload".to_string()),
            items: Some(100),
        };
        assert_eq!(cfg.path, Some("sideload".to_string()));
        assert_eq!(cfg.items, Some(100));
    }

    #[test]
    fn test_sideload_cfg_default() {
        let cfg = SideloadCfg::DEFAULT;
        assert_eq!(cfg.path, None);
        assert_eq!(cfg.items, None);
    }

    #[test]
    fn test_serialize_to_toml() {
        let cfg = SideloadCfg {
            disabled: true,
            path: Some("sideload".to_string()),
            items: Some(100),
        };
        let toml_str = toml::to_string(&cfg).unwrap();
        assert!(toml_str.contains("path = \"sideload\""));
        assert!(toml_str.contains("items = 100"));
    }

    #[test]
    fn test_deserialize_from_toml() {
        let toml_str = r#"enabled = true
path = "sideload"
items = 100"#;
        let cfg: SideloadCfg = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.path, Some("sideload".to_string()));
        assert_eq!(cfg.items, Some(100));
    }
}
