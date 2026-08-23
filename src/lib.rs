#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

pub mod check;
pub mod config;
pub mod error;
pub mod extract;
pub mod mapping;
pub mod pack;
pub mod runner;
pub mod scaffold;
pub mod tree;

pub use error::{Error, Result};
