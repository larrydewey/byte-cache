use std::num::NonZeroUsize;

use const_default::ConstDefault;

use super::*;

/// Configuration for the in-memory LRU cache component.
///
/// This struct defines the settings for the memory cache, including
/// whether it's enabled and the maximum number of items it can hold.
///
/// # Examples
///
/// Basic memory cache with 500 item limit:
/// ```rust
/// use byte_cache::configuration::MemoryCfg;
///
/// let memory_cfg = MemoryCfg {
///     disabled: false,
///     items: Some(500),
/// };
/// ```
///
/// Disabled memory cache:
/// ```rust
/// use byte_cache::configuration::MemoryCfg;
///
/// let memory_cfg = MemoryCfg {
///     disabled: true,
///     items: None,
/// };
/// ```
#[derive(ConstDefault, Default, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCfg {
    /// Whether the memory cache is enabled
    #[serde(default)]
    pub disabled: bool,
    /// Maximum number of items to store in the memory cache
    pub items: Option<usize>,
}

impl MemoryCfg {
    /// Creates an in-memory LRU cache based on the configuration.
    ///
    /// # Returns
    /// * `Ok(lru::LruCache<String, Vec<u8>>)`: The created LRU cache
    /// * `Err(std::io::Error)`: If the memory cache is disabled or incorrectly configured
    ///
    /// # Errors
    /// This method will return an error in the following cases:
    /// - If the memory cache is disabled
    /// - If the item count is not specified (items is None)
    /// - If the item count is zero (invalid NonZeroUsize)
    pub async fn lru_cache(&self) -> std::io::Result<lru::LruCache<String, Vec<u8>>> {
        if self.disabled {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Memory cache is disabled",
            ));
        }

        if let Some(items) = self.items {
            if let Some(count) = NonZeroUsize::new(items) {
                Ok(lru::LruCache::new(count))
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Memory cache items must be a positive number",
                ))
            }
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Memory cache items not specified",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_cfg() {
        let cfg = MemoryCfg {
            disabled: true,
            items: Some(100),
        };
        assert_eq!(cfg.disabled, true);
        assert_eq!(cfg.items, Some(100));
    }

    #[test]
    fn test_memory_cfg_default() {
        let cfg = MemoryCfg::DEFAULT;
        assert_eq!(cfg.disabled, false);
        assert_eq!(cfg.items, None);
    }

    #[test]
    fn test_serialize_to_toml() {
        let cfg = MemoryCfg {
            disabled: false,
            items: Some(100),
        };
        let toml_str = toml::to_string(&cfg).unwrap();
        assert!(toml_str.contains("disabled = false"));
        assert!(toml_str.contains("items = 100"));
    }

    #[test]
    fn test_deserialize_from_toml() {
        let toml_str = r#"
            items = 100
        "#;
        let cfg: MemoryCfg = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.disabled, false);
        assert_eq!(cfg.items, Some(100));
    }
}
