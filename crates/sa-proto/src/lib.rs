//! wire types and protocol constants.
//!
//! this crate is deliberately serde-only: no tokio, no terminal, no i/o. the line codec
//! lives in the transports that use it (see `MAX_LINE_LEN`).

pub mod error;
pub mod rpc;
pub mod session;
pub mod tokens;

/// the commit this build came from, stamped by `build.rs`. a client and a daemon built
/// from different commits are not the same program, and say so.
pub const BUILD_REV: &str = env!("SA_BUILD_REV");

/// the version string a binary reports, with the commit it was built from.
pub const BUILD_NAME: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("SA_BUILD_REV"), ")");

/// maximum length of a single newline-delimited frame, excluding the newline.
///
/// a longer line is a protocol error, never a truncated read (spec 3.1).
pub const MAX_LINE_LEN: usize = 1024 * 1024;

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn max_line_len_is_exactly_one_mebibyte() {
        // the protocol bound in spec 3.1: any other value silently changes what is legal.
        assert_eq!(MAX_LINE_LEN, 1_048_576);
    }
}
