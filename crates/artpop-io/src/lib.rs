//! artpop-io: interchange formats behind narrow Rust interfaces.
//!
//! CFITSIO (via the `fitsio` crate, vendored and statically linked) is the
//! only native dependency. Core scientific types never see FFI handles.

// Errors carry JSON details for agents; they are cold-path values, so the
// large-Err lint is not useful. `!(x > 0.0)` comparisons deliberately reject NaN.
#![allow(clippy::result_large_err, clippy::neg_cmp_op_on_partial_ord)]

pub mod csv;
pub mod fits;
pub mod preview;
