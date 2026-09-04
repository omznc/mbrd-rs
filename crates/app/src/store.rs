//! Where a file goes, on a platform that has files and on one that does not.
//!
//! Every read and write in this app that is not a test goes through here. On
//! the three native platforms each function is `std::fs` with the same name and
//! costs nothing; on the web it is `webfs.rs`, which keeps the same files in
//! the store a browser tab is given instead of a disk.
//!
//! A facade rather than a `cfg` at each call site, for the reason `pipeline.rs`
//! gives for the same shape: there are a dozen call sites and one difference
//! between them, so the difference belongs in one file. It also keeps the
//! native build honest — nothing here wraps `std::fs` in anything, so a
//! `store::read` on a desktop is the same read it always was.
//!
//! [`read_dir_paths`] is the one function that is not a rename of a `std::fs`
//! one. `read_dir` hands back an iterator of entries whose type differs per
//! platform; every caller in this app immediately turns that into paths, so
//! this returns the paths and the two platforms keep one call site.

#[cfg(not(target_family = "wasm"))]
use std::io::Result;
#[cfg(not(target_family = "wasm"))]
use std::path::{Path, PathBuf};

#[cfg(not(target_family = "wasm"))]
pub use native::*;
#[cfg(target_family = "wasm")]
pub use web::*;

#[cfg(not(target_family = "wasm"))]
mod native {
    use super::*;

    pub fn read(path: &Path) -> Result<Vec<u8>> {
        std::fs::read(path)
    }

    pub fn read_to_string(path: &Path) -> Result<String> {
        std::fs::read_to_string(path)
    }

    pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
        std::fs::write(path, bytes)
    }

    pub fn create_dir_all(path: &Path) -> Result<()> {
        std::fs::create_dir_all(path)
    }

    pub fn remove_file(path: &Path) -> Result<()> {
        std::fs::remove_file(path)
    }

    pub fn rename(from: &Path, to: &Path) -> Result<()> {
        std::fs::rename(from, to)
    }

    pub fn exists(path: &Path) -> bool {
        path.exists()
    }

    pub fn is_dir(path: &Path) -> bool {
        path.is_dir()
    }

    pub fn is_file(path: &Path) -> bool {
        path.is_file()
    }

    pub fn read_dir_paths(dir: &Path) -> Result<Vec<PathBuf>> {
        let mut out: Vec<PathBuf> =
            std::fs::read_dir(dir)?.filter_map(|entry| Some(entry.ok()?.path())).collect();
        out.sort();
        Ok(out)
    }
}

#[cfg(target_family = "wasm")]
mod web {
    pub use crate::webfs::{
        create_dir_all, exists, is_dir, is_file, read, read_dir_paths, read_to_string, remove_file,
        rename, write,
    };
}
