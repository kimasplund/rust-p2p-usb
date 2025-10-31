//! Protocol version management

use serde::{Deserialize, Serialize};

/// Protocol version using semantic versioning
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
}

/// Current protocol version
pub const CURRENT_VERSION: ProtocolVersion = ProtocolVersion {
    major: 1,
    minor: 0,
    patch: 0,
};

impl ProtocolVersion {
    /// Check if this version is compatible with another version
    pub fn is_compatible_with(&self, other: &ProtocolVersion) -> bool {
        self.major == other.major && self.minor >= other.minor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_compatibility() {
        let v1_0_0 = ProtocolVersion {
            major: 1,
            minor: 0,
            patch: 0,
        };
        let v1_1_0 = ProtocolVersion {
            major: 1,
            minor: 1,
            patch: 0,
        };
        let v2_0_0 = ProtocolVersion {
            major: 2,
            minor: 0,
            patch: 0,
        };

        assert!(v1_1_0.is_compatible_with(&v1_0_0));
        assert!(!v1_0_0.is_compatible_with(&v1_1_0));
        assert!(!v2_0_0.is_compatible_with(&v1_0_0));
    }
}
