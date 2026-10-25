//! Bonanza 1004 transport protocol 3. No 1002 GPIO/lease protocol support.
pub const DATA_BAUDRATE: u32 = 5_000_000;
pub const STATUS_LEN: usize = 84;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub phase: u8,
    pub uptime_ms: u64,
    pub generation: u32,
}

fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

pub fn valid_info(data: &[u8]) -> bool {
    data.len() >= 5
        && data[..3] == [1, 3, 0]
        && (1..=63).contains(&data[3])
        && data.len() == 4 + data[3] as usize
        && data[4..].iter().all(|b| (0x20..=0x7e).contains(b))
}

pub fn status(data: &[u8]) -> Option<Status> {
    if data.len() != STATUS_LEN
        || data[0] != 1
        || data[1] > 3
        || data[5] > 1
        || u16::from_le_bytes([data[6], data[7]]) != 1004
        || u32_at(data, 8) & 0x3f != 0x3f
    {
        return None;
    }
    Some(Status {
        phase: data[1],
        uptime_ms: u64::from_le_bytes(data[12..20].try_into().unwrap()),
        generation: u32_at(data, 20),
    })
}

pub fn valid_queues(data: &[u8]) -> bool {
    data.len() == 68
        && data[..4] == [1, 0, 0, 0]
        && u32_at(data, 4) == DATA_BAUDRATE
        && u32_at(data, 8) != 0
}

pub fn reset_complete(before: Status, after: Status) -> bool {
    after.phase == 1
        && after.uptime_ms >= before.uptime_ms
        && after.generation == before.generation.wrapping_add(1)
}

pub fn same_stream(before: Status, after: Status) -> bool {
    after.phase == 1 && after.uptime_ms >= before.uptime_ms && after.generation == before.generation
}

#[cfg(test)]
mod tests {
    use super::*;
    fn payload() -> [u8; STATUS_LEN] {
        let mut p = [0; STATUS_LEN];
        p[0] = 1;
        p[1] = 1;
        p[6..8].copy_from_slice(&1004u16.to_le_bytes());
        p[8] = 0x3f;
        p
    }
    #[test]
    fn rejects_old_protocols_bad_lengths_and_versions() {
        assert!(valid_info(&[1, 3, 0, 1, b'x']));
        for major in [0, 1, 2, 4] {
            assert!(!valid_info(&[1, major, 0, 1, b'x']));
        }
        assert!(!valid_info(&[1, 3, 0, 2, b'x']));
        assert!(!valid_info(&[1, 3, 0, 1, 0]));
        for len in 0..5 {
            assert!(!valid_info(&[1, 3, 0, 1, b'x'][..len]));
        }
    }
    #[test]
    fn rejects_wrong_board_capabilities_and_schema() {
        let p = payload();
        assert!(status(&p).is_some());
        for len in 0..STATUS_LEN {
            assert!(status(&p[..len]).is_none());
        }
        let mut p = payload();
        p[6..8].copy_from_slice(&1002u16.to_le_bytes());
        assert!(status(&p).is_none());
        let mut p = payload();
        p[8] &= !8;
        assert!(status(&p).is_none());
        let mut p = payload();
        p[0] = 2;
        assert!(status(&p).is_none());
    }
    #[test]
    fn requires_matching_data_baud() {
        let mut p = [0; 68];
        p[0] = 1;
        p[8] = 1;
        p[4..8].copy_from_slice(&DATA_BAUDRATE.to_le_bytes());
        assert!(valid_queues(&p));
        p[4..8].copy_from_slice(&2_000_000u32.to_le_bytes());
        assert!(!valid_queues(&p));
    }
    #[test]
    fn reset_requires_running_and_exact_generation_advance() {
        let before = Status {
            phase: 3,
            uptime_ms: 10,
            generation: u32::MAX,
        };
        let after = Status {
            phase: 1,
            uptime_ms: 11,
            generation: 0,
        };
        assert!(reset_complete(before, after));
        assert!(!reset_complete(
            before,
            Status {
                generation: 1,
                ..after
            }
        ));
        assert!(!reset_complete(before, Status { phase: 3, ..after }));
        assert!(!reset_complete(
            before,
            Status {
                uptime_ms: 0,
                ..after
            }
        ));
    }
    #[test]
    fn detects_restart_reset_and_fault_during_mining() {
        let before = Status {
            phase: 1,
            uptime_ms: 10,
            generation: 2,
        };
        assert!(same_stream(
            before,
            Status {
                uptime_ms: 11,
                ..before
            }
        ));
        assert!(!same_stream(
            before,
            Status {
                uptime_ms: 0,
                ..before
            }
        ));
        assert!(!same_stream(
            before,
            Status {
                generation: 3,
                ..before
            }
        ));
        assert!(!same_stream(before, Status { phase: 3, ..before }));
    }
}
