//! Pure conversions shared by the ESP32 fan driver and host-side unit tests.

pub const TACH_SAMPLE_MS: u64 = 500;
pub const TACH_PULSES_PER_REVOLUTION: u64 = 2;

/// Active-low, eight-bit duty: zero holds the output low for full fan speed.
pub fn percent_to_raw_duty(percent: u8) -> Option<u8> {
    if percent > 100 {
        return None;
    }
    let scaled = u16::from(percent) * 255;
    let mut duty = scaled / 100;
    let remainder = scaled % 100;
    // Match Python round(), including ties to even.
    if remainder > 50 || (remainder == 50 && duty % 2 == 1) {
        duty += 1;
    }
    Some(255 - duty as u8)
}

pub fn rpm_from_pulses(pulses: u32, elapsed_us: u64) -> Option<u32> {
    if elapsed_us == 0 {
        return None;
    }
    let rpm = u64::from(pulses) * 60_000_000 / (elapsed_us * TACH_PULSES_PER_REVOLUTION);
    u32::try_from(rpm).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_low_endpoints_and_rounding_match_anvil() {
        assert_eq!(percent_to_raw_duty(0), Some(255));
        assert_eq!(percent_to_raw_duty(50), Some(127));
        assert_eq!(percent_to_raw_duty(30), Some(179));
        assert_eq!(percent_to_raw_duty(100), Some(0));
        assert_eq!(percent_to_raw_duty(101), None);
    }

    #[test]
    fn duty_is_monotonic_and_never_wraps() {
        let mut previous = 255;
        for percent in 0..=100 {
            let duty = percent_to_raw_duty(percent).unwrap();
            assert!(duty <= previous);
            previous = duty;
        }
    }

    #[test]
    fn rpm_uses_actual_elapsed_time_and_two_pulses_per_revolution() {
        assert_eq!(rpm_from_pulses(50, 500_000), Some(3000));
        assert_eq!(rpm_from_pulses(50, 600_000), Some(2500));
        assert_eq!(rpm_from_pulses(0, 500_000), Some(0));
        assert_eq!(rpm_from_pulses(1, 0), None);
    }
}
