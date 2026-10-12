//! artpop-core: the scientific engine of artpop-rust.
//!
//! This crate has no knowledge of files, networks, CLI, or MCP. Every
//! numerical algorithm (IMF sampling, isochrone integrals, populations,
//! spatial probability grids, weighted histograms, PSFs, convolution, and
//! detector noise) is implemented here in Rust.

// Errors carry JSON details for agents; they are cold-path values, so the
// large-Err lint is not useful. `!(x > 0.0)` comparisons deliberately reject NaN.
#![allow(clippy::result_large_err, clippy::neg_cmp_op_on_partial_ord)]

pub mod convolve;
pub mod error;
pub mod filters;
pub mod image;
pub mod imager;
pub mod imf;
pub mod interp;
pub mod isochrone;
pub mod mist;
pub mod population;
pub mod psf;
pub mod rng;
pub mod spatial;
pub mod special;

pub use error::{Error, ErrorCode, Result};

/// Version of the numerical contract implemented by this build.
pub const ALGORITHM_PROFILE: &str = "rust-v1";
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
