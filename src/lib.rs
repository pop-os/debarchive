//! This crate provides direct access to files within a Debian archive.
//! 
//! # Features
//! 
//! - [x] Reading files from archives
//! - [x] Extracting files from archives
//! - [ ] Writing new debian archives
//! 
//! # Examples
//! 
//! ```rust,no_run
//! extern crate debarchive;
//! 
//! use debarchive::Archive;
//! use std::path::Path;
//! 
//! fn main() {
//!     let path = &Path::new("name_version_arch.deb");
//!     let archive = Archive::new(path).unwrap();
//!     archive.data(|entry| {
//!         if let Ok(path) = entry.path() {
//!             println!("data: {}", path.display());
//!         }
//!     });
//! 
//!     let control_map = archive.control_map().unwrap();
//!     println!("Control: {:#?}", control_map);
//! }

extern crate ar;
extern crate tar;
extern crate xz2;
extern crate libflate;

mod archive;

pub use self::archive::*;