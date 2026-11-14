//! peer credential checks (spec 4.1).
//!
//! the daemon trusts exactly one identity: its own unix user. everything else is refused
//! before a single frame is read.

/// compares a peer's uid against the daemon's own effective uid.
///
/// kept pure so the rejection path is testable without a second unix user.
pub fn check_uid(expected: u32, actual: u32) -> Result<(), PeerError> {
    if expected == actual {
        Ok(())
    } else {
        Err(PeerError::UidMismatch { expected, actual })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PeerError {
    #[error("peer uid {actual} does not match daemon uid {expected}")]
    UidMismatch { expected: u32, actual: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_daemons_own_uid() {
        assert_eq!(check_uid(1000, 1000), Ok(()));
    }

    #[test]
    fn uid_check_rejects_mismatch() {
        let err = check_uid(1000, 0).expect_err("root must be refused");
        assert_eq!(
            err,
            PeerError::UidMismatch {
                expected: 1000,
                actual: 0
            }
        );
    }

    #[test]
    fn mismatch_names_both_uids() {
        let err = check_uid(1000, 0).expect_err("refused");
        let text = err.to_string();
        assert!(text.contains("1000"), "{text}");
        assert!(text.contains('0'), "{text}");
    }
}
