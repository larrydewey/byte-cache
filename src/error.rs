//! # OmneCache Error Types
//!
//! Error types and error handling for the OmneCache caching system.
//!
//! This module defines the error types that can occur during OmneCache operations:
//!
//! * [`CacheableError`]: Errors that occur during the core caching operations
//! * [`ConfigurationError`]: Errors related to configuration loading and saving
//! * [`TokioError`]: Wrapper for various tokio-related errors
//!
//! The error types implement standard Rust traits like `std::error::Error` and
//! `std::fmt::Display` for integration with the Rust error handling ecosystem.
//!
//! ## Error Conversions
//!
//! The module provides convenient `From` implementations to convert between error types:
//!
//! * `std::io::Error` → `ConfigurationError` and `CacheableError`
//! * `toml::de::Error` → `ConfigurationError`
//! * `toml::ser::Error` → `ConfigurationError`
//! * `tokio::task::JoinError` → `TokioError` and `CacheableError`
//! * `tokio::time::error::Elapsed` → `TokioError` and `CacheableError`
//! * `nix::errno::Errno` → `CacheableError`
//!
//! This makes it easy to use the `?` operator in functions that can produce these errors.

#[derive(Debug)]
pub enum TokioError {
    Timeout(tokio::time::error::Elapsed),
    Time(tokio::time::error::Error),
    Task(tokio::task::JoinError),
    Io(tokio::io::Error),
}

impl std::error::Error for TokioError {}

impl std::fmt::Display for TokioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokioError::Timeout(elapsed) => write!(f, "timout error encountered: {}", elapsed),
            TokioError::Time(error) => write!(f, "timer error encountered: {}", error),
            Self::Task(error) => write!(f, "join error encountered: {}", error),
            TokioError::Io(error) => write!(f, "io error encountered: {}", error),
        }
    }
}

impl From<tokio::time::error::Elapsed> for TokioError {
    fn from(value: tokio::time::error::Elapsed) -> Self {
        Self::Timeout(value)
    }
}

impl From<tokio::time::error::Error> for TokioError {
    fn from(value: tokio::time::error::Error) -> Self {
        Self::Time(value)
    }
}

impl From<tokio::io::Error> for TokioError {
    fn from(value: tokio::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<tokio::task::JoinError> for TokioError {
    fn from(value: tokio::task::JoinError) -> Self {
        Self::Task(value)
    }
}

/// Errors that can occur during OmneCache operations.
///
/// This enum represents the various error conditions that can occur
/// when working with cacheable data.
#[derive(Debug)]
pub enum CacheableError {
    /// The requested data was not found in any cache or source
    NotFound,
    /// Cache-write error when no viable cache is available for writing
    WriteError,
    /// Empty buffer error when a value is unexpectedly empty
    EmptyBuffer,
    /// Empty Key error when a key string is unexpectedly empty
    EmptyKey,
    /// Tokio-related errors (timeouts, tasks, etc.)
    Tokio(TokioError),
    /// An error occurred during file I/O operations
    Io(std::io::Error),
    /// Errors from the nix crate's system calls
    Nix(nix::errno::Errno),
}

impl std::error::Error for CacheableError {}

impl std::fmt::Display for CacheableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "Data not found"),
            Self::Tokio(error) => write!(f, "Tokio Error: {}", error),
            Self::WriteError => write!(f, "Could not find a viable cache to write to"),
            Self::EmptyBuffer => write!(f, "Keys cannot have an empty value"),
            Self::EmptyKey => write!(f, "Keys cannot be empty"),
            Self::Io(err) => write!(f, "IO error: {}", err),
            Self::Nix(err) => write!(f, "Nix error: {}", err),
        }
    }
}

impl From<tokio::task::JoinError> for CacheableError {
    fn from(error: tokio::task::JoinError) -> Self {
        Self::Tokio(error.into())
    }
}

impl From<tokio::time::error::Elapsed> for CacheableError {
    fn from(error: tokio::time::error::Elapsed) -> Self {
        Self::Tokio(error.into())
    }
}

impl From<TokioError> for CacheableError {
    fn from(error: TokioError) -> Self {
        Self::Tokio(error)
    }
}

impl From<std::io::Error> for CacheableError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<nix::errno::Errno> for CacheableError {
    fn from(error: nix::errno::Errno) -> Self {
        Self::Nix(error)
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
