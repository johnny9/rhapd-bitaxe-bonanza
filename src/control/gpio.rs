use core::cell::RefCell;

use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
use embassy_time::Timer;
use esp_hal::gpio::{Input, Level, Output};
use heapless::Vec;

use super::CommandError;
use crate::board_safety::BoardSafety;

pub struct Pins<'d> {
    pub asic_reset: Output<'d>,
    pub asic_trip: Input<'d>,
    pub five_volt_enable: Output<'d>,
    pub vr_en: Output<'d>,
    pub vr_pgood: Input<'d>,
    pub safety: BoardSafety,
}

static BOARD: Mutex<CriticalSectionRawMutex, RefCell<Option<Pins<'static>>>> =
    Mutex::new(RefCell::new(None));

pub fn initialize(pins: Pins<'static>) {
    BOARD.lock(|board| *board.borrow_mut() = Some(pins));
}

fn with_board<T>(f: impl FnOnce(&mut Pins<'static>) -> T) -> T {
    BOARD.lock(|board| f(board.borrow_mut().as_mut().unwrap()))
}

impl Pins<'_> {
    fn apply_outputs(&mut self) {
        // Assert reset before removing 5 V; establish 5 V before releasing reset.
        if !self.safety.reset_released {
            self.asic_reset.set_low();
        }
        self.five_volt_enable
            .set_level(Level::from(self.safety.five_volt_enabled));
        if self.safety.reset_released {
            self.asic_reset.set_high();
        }
    }

    fn service_trip(&mut self) {
        self.safety.observe_trip(self.asic_trip.is_high());
        if self.safety.trip_latched {
            self.vr_en.set_low();
            super::fan::force_full_speed();
            self.apply_outputs();
        }
    }
}

pub fn shutdown() {
    with_board(|pins| {
        pins.safety.shutdown();
        pins.apply_outputs();
        pins.vr_en.set_low();
        super::fan::force_full_speed();
    });
}

pub fn prepare_programming() -> bool {
    with_board(|pins| {
        pins.service_trip();
        if pins.vr_pgood.is_high() || pins.safety.trip_latched {
            return false;
        }
        pins.safety.shutdown();
        pins.apply_outputs();
        pins.vr_en.set_low();
        super::fan::force_full_speed();
        true
    })
}

pub fn fault_latched() -> bool {
    with_board(|pins| {
        pins.service_trip();
        pins.safety.trip_latched
    })
}

pub fn data_enabled() -> bool {
    with_board(|pins| pins.safety.reset_released && !pins.safety.trip_latched)
}

/// [schema, flags, active-low fan duty]. Flags: 5V, reset asserted, trip input,
/// trip latched, VR_EN, VR_PGOOD. All describe ESP32 signals on the 1004x.
pub fn status() -> [u8; 3] {
    with_board(|pins| {
        pins.service_trip();
        let flags = pins.safety.five_volt_enabled as u8
            | (((!pins.safety.reset_released) as u8) << 1)
            | ((pins.safety.trip_asserted as u8) << 2)
            | ((pins.safety.trip_latched as u8) << 3)
            | ((pins.vr_en.is_set_high() as u8) << 4)
            | ((pins.vr_pgood.is_high() as u8) << 5);
        [1, flags, super::fan::requested_duty()]
    })
}

#[embassy_executor::task]
pub async fn trip_task() -> ! {
    loop {
        with_board(|pins| pins.service_trip());
        Timer::after_millis(1).await;
    }
}

#[derive(defmt::Format)]
pub enum Command {
    SetAsicReset { released: bool },
    GetAsicReset,
    Set5vEn { level: bool },
    Get5vEn,
    GetAsicTrip,
    SetVrEn { level: bool },
    GetVrEn,
    GetVrPgood,
}

impl Command {
    pub fn from_bytes(buf: &[u8]) -> Result<Self, CommandError> {
        match buf {
            [0x00] | [0x02] => Ok(Self::GetAsicReset),
            [0x00, level @ 0..=1] | [0x02, level @ 0..=1] => Ok(Self::SetAsicReset {
                released: *level == 1,
            }),
            [0x01] => Ok(Self::Get5vEn),
            [0x01, level @ 0..=1] => Ok(Self::Set5vEn { level: *level == 1 }),
            [0x03] => Ok(Self::GetAsicTrip),
            [0x04] => Ok(Self::GetVrEn),
            [0x04, level @ 0..=1] => Ok(Self::SetVrEn { level: *level == 1 }),
            [0x05] => Ok(Self::GetVrPgood),
            _ => Err(CommandError::Invalid),
        }
    }
}

impl super::ControllerCommand for Command {
    async fn handle(
        &self,
        controller: &mut super::Controller,
    ) -> Result<Vec<u8, 256>, CommandError> {
        if matches!(self, Command::SetAsicReset { released: true }) && !data_enabled() {
            if super::programmer::is_active() {
                return Err(CommandError::Denied);
            }
            // Keep reset asserted until both UARTs and the bridge have a clean boundary.
            crate::bridge::prepare_stream(controller.bridge).await?;
        }
        with_board(|pins| {
            pins.service_trip();
            if super::programmer::is_active()
                && matches!(
                    self,
                    Command::SetAsicReset { released: true }
                        | Command::Set5vEn { level: true }
                        | Command::SetVrEn { level: true }
                )
            {
                return Err(CommandError::Denied);
            }
            let level = match self {
                Command::GetAsicReset => pins.safety.reset_released,
                Command::SetAsicReset { released } => {
                    if !pins.safety.set_reset_released(*released) {
                        return Err(CommandError::Fault);
                    }
                    pins.apply_outputs();
                    pins.safety.reset_released
                }
                Command::Get5vEn => pins.safety.five_volt_enabled,
                Command::Set5vEn { level } => {
                    if !pins.safety.set_five_volt(*level) {
                        return Err(CommandError::Denied);
                    }
                    pins.apply_outputs();
                    pins.safety.five_volt_enabled
                }
                Command::GetAsicTrip => pins.safety.trip_asserted,
                Command::GetVrEn => pins.vr_en.is_set_high(),
                Command::SetVrEn { level } => {
                    if *level && pins.safety.trip_latched {
                        return Err(CommandError::Fault);
                    }
                    pins.vr_en.set_level(Level::from(*level));
                    *level
                }
                Command::GetVrPgood => pins.vr_pgood.is_high(),
            };
            Ok(Vec::from_slice(&[level as u8]).unwrap())
        })
    }
}
