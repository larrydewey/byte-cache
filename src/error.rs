//! # OmneCache Error Types
//!
//! Error types and error handling for the OmneCache caching system.
//!
//! This module defines the error types that can occur during OmneCache operations:
//!
//! * [`OmneCacheableError`]: Errors that occur during the core caching operations
//! * [`ConfigurationError`]: Errors related to configuration loading and saving
//!
//! The error types implement standard Rust traits like `std::error::Error` and
//! `std::fmt::Display` for integration with the Rust error handling ecosystem.
//!
//! ## Error Conversions
//!
//! The module provides convenient `From` implementations to convert between error types:
//!
//! * `std::io::Error` → `ConfigurationError`
//! * `toml::de::Error` → `ConfigurationError`
//! * `toml::ser::Error` → `ConfigurationError`
//!
//! This makes it easy to use the `?` operator in functions that can produce these errors.

/// Errors that can occur during OmneCache operations.
///
/// This enum represents the various error conditions that can occur
/// when working with cacheable data.
#[derive(Debug)]
pub enum CacheableError {
    /// The requested data was not found in any cache or source
    NotFound,
    /// An error occurred while writing to the disk cache
    Io(std::io::Error),
}

impl std::error::Error for CacheableError {}

impl std::fmt::Display for CacheableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "Data not found"),
            Self::Io(err) => write!(f, "IO error: {}", err),
        }
    }
}

impl From<std::io::Error> for CacheableError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Errors that can occur during configuration operations.
///
/// This enum represents the various error conditions that can occur when
/// loading, saving, or working with OmneCache configurations.
#[derive(Debug)]
pub enum ConfigurationError {
    /// IO error occurred (file access, permissions, etc.)
    Io(std::io::Error),
    /// Error parsing TOML during deserialization
    ParseDeError(toml::de::Error),
    /// Error generating TOML during serialization
    ParseSerError(toml::ser::Error),
    /// Configuration directory not found
    CfgDirNotFound,
}

impl std::error::Error for ConfigurationError {}

impl std::fmt::Display for ConfigurationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "IO error: {}", error),
            Self::ParseDeError(error) => {
                write!(f, "deserialization parsing error: {}", error)
            }
            Self::ParseSerError(error) => {
                write!(f, "serialization parsing error: {}", error)
            }
            Self::CfgDirNotFound => write!(f, "directory not found"),
        }
    }
}

/// Convert from std::io::Error to ConfigurationError
impl std::convert::From<std::io::Error> for ConfigurationError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Convert from toml::de::Error to ConfigurationError
impl std::convert::From<toml::de::Error> for ConfigurationError {
    fn from(error: toml::de::Error) -> Self {
        Self::ParseDeError(error)
    }
}

/// Convert from toml::ser::Error to ConfigurationError
impl std::convert::From<toml::ser::Error> for ConfigurationError {
    fn from(error: toml::ser::Error) -> Self {
        Self::ParseSerError(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte_cacheable_error_display() {
        let error = CacheableError::NotFound;
        assert_eq!(format!("{}", error), "Data not found");
    }

    #[test]
    fn test_configuration_error_display() {
        let error = ConfigurationError::CfgDirNotFound;
        assert_eq!(format!("{}", error), "directory not found");
    }

    #[test]
    fn test_configuration_error_io_display() {
        let error = std::io::Error::new(std::io::ErrorKind::Other, "IO error");
        let config_error: ConfigurationError = error.into();
        assert_eq!(format!("{}", config_error), "IO error: IO error");
    }
}
