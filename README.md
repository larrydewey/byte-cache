<!-- cargo-rdme start -->

# OmneCache

A flexible multi-layer caching system for byte-oriented data with configurable storage.

OmneCache provides a hierarchical caching system with three optional layers:

* **Memory**: Fast in-memory LRU cache for recently accessed items
* **Sideload**: Pre-loaded content that can be used without downloading
* **Disk**: Persistent storage in the filesystem

## Architecture

When retrieving data, OmneCache checks each enabled cache layer in order
(memory → sideload → disk). If the data is not found
in any cache, a `NotFound` error is returned.

## Configuration

OmneCache offers a flexible configuration system through the [`configuration`] module,
allowing you to:

* Enable or disable specific cache layers
* Set capacity limits for each layer
* Define custom paths for disk and sideload caches
* Load and save configurations from/to TOML files

## Example Usage

To use OmneCache, implement the [`Cacheable`] trait for your data type and
configure the cache layers according to your needs.

```rust
use byte_cache::{OmneCache, Cacheable, configuration::{OmneCacheCfg, MemoryCfg, DiskCfg, SideloadCfg}};
use std::path::PathBuf;

// Configure and build a OmneCache instance
let cache = OmneCacheCfg {
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
```

<!-- cargo-rdme end -->

