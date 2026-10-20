use crate::{Error, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_HEADROOM_PERCENT: u8 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capacity {
    pub total_bytes: u64,
    pub free_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Projection {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub additional_bytes: u64,
    pub free_after_bytes: u64,
    pub headroom_bytes: u64,
    pub headroom_percent: u8,
    pub allowed: bool,
}

impl Projection {
    pub fn used_by_active_set(&self) -> u64 {
        self.total_bytes.saturating_sub(self.free_bytes)
    }
}

/// Project free space after activating `additional_bytes`.
///
/// Headroom is a percentage of total capacity, not of free space, so the
/// threshold does not move as the device fills up.
pub fn project(
    capacity: Capacity,
    additional_bytes: u64,
    headroom_percent: u8,
) -> Result<Projection> {
    if headroom_percent > 100 {
        return Err(Error::Capacity("headroom must be 0-100 percent".into()));
    }
    if capacity.total_bytes == 0 {
        return Err(Error::Capacity("total capacity is zero".into()));
    }
    if capacity.free_bytes > capacity.total_bytes {
        return Err(Error::Capacity("free space exceeds total capacity".into()));
    }
    let headroom_bytes =
        (u128::from(capacity.total_bytes) * u128::from(headroom_percent) / 100) as u64;
    let free_after_bytes = capacity.free_bytes.saturating_sub(additional_bytes);
    Ok(Projection {
        total_bytes: capacity.total_bytes,
        free_bytes: capacity.free_bytes,
        additional_bytes,
        free_after_bytes,
        headroom_bytes,
        headroom_percent,
        allowed: additional_bytes <= capacity.free_bytes && free_after_bytes >= headroom_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_when_projected_free_drops_below_headroom() {
        let capacity = Capacity {
            total_bytes: 1000,
            free_bytes: 200,
        };
        assert!(project(capacity, 50, 10).unwrap().allowed);
        assert!(!project(capacity, 101, 10).unwrap().allowed);
        assert_eq!(project(capacity, 101, 10).unwrap().headroom_bytes, 100);
    }

    #[test]
    fn refuses_more_than_free() {
        let capacity = Capacity {
            total_bytes: 1000,
            free_bytes: 100,
        };
        let projection = project(capacity, 101, 0).unwrap();
        assert!(!projection.allowed);
        assert_eq!(projection.free_after_bytes, 0);
    }

    #[test]
    fn headroom_uses_total_capacity() {
        let capacity = Capacity {
            total_bytes: 1000,
            free_bytes: 500,
        };
        assert!(!project(capacity, 401, 10).unwrap().allowed);
        assert!(project(capacity, 400, 10).unwrap().allowed);
    }

    #[test]
    fn rejects_invalid_inputs() {
        assert!(project(
            Capacity {
                total_bytes: 0,
                free_bytes: 0
            },
            0,
            10
        )
        .is_err());
        assert!(project(
            Capacity {
                total_bytes: 100,
                free_bytes: 101
            },
            0,
            10
        )
        .is_err());
        assert!(project(
            Capacity {
                total_bytes: 100,
                free_bytes: 100
            },
            0,
            101
        )
        .is_err());
    }
}
