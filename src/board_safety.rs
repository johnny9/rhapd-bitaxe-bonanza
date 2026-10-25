//! Board-output ordering for the ESP32-controlled 1004x.

#[derive(Default)]
pub struct BoardSafety {
    pub five_volt_enabled: bool,
    pub reset_released: bool,
    pub trip_asserted: bool,
    pub trip_latched: bool,
}

impl BoardSafety {
    pub fn observe_trip(&mut self, asserted: bool) {
        self.trip_asserted = asserted;
        if asserted {
            self.trip_latched = true;
            self.shutdown();
        }
    }

    pub fn shutdown(&mut self) {
        self.reset_released = false;
        self.five_volt_enabled = false;
    }

    pub fn set_five_volt(&mut self, enabled: bool) -> bool {
        if enabled && (self.trip_latched || self.trip_asserted || self.reset_released) {
            return false;
        }
        self.five_volt_enabled = enabled;
        if !enabled {
            self.reset_released = false;
        }
        true
    }

    pub fn set_reset_released(&mut self, released: bool) -> bool {
        if released && (!self.five_volt_enabled || self.trip_asserted || self.trip_latched) {
            return false;
        }
        self.reset_released = released;
        // An explicit assert while safely off acknowledges a cleared trip.
        if !released && !self.five_volt_enabled && !self.trip_asserted {
            self.trip_latched = false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_up_requires_five_volts_before_reset_release() {
        let mut state = BoardSafety::default();
        assert!(!state.set_reset_released(true));
        assert!(state.set_five_volt(true));
        assert!(state.set_reset_released(true));
        assert!(state.set_five_volt(false));
        assert!(!state.reset_released);
    }

    #[test]
    fn trip_shuts_down_and_stays_latched_after_input_clears() {
        let mut state = BoardSafety::default();
        assert!(state.set_five_volt(true));
        assert!(state.set_reset_released(true));
        state.observe_trip(true);
        assert!(!state.reset_released && !state.five_volt_enabled);
        assert!(state.trip_latched);
        state.observe_trip(false);
        assert!(!state.set_five_volt(true));
        assert!(!state.set_reset_released(true));
        assert!(state.set_reset_released(false));
        assert!(!state.trip_latched);
        assert!(state.set_five_volt(true));
    }

    #[test]
    fn active_trip_cannot_be_acknowledged() {
        let mut state = BoardSafety::default();
        state.observe_trip(true);
        assert!(state.set_reset_released(false));
        assert!(state.trip_latched);
        assert!(!state.set_five_volt(true));
    }

    #[test]
    fn disconnect_always_restores_safe_outputs() {
        let mut state = BoardSafety::default();
        state.set_five_volt(true);
        state.set_reset_released(true);
        state.shutdown();
        assert!(!state.five_volt_enabled && !state.reset_released);
    }
}
