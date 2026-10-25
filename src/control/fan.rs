use core::sync::atomic::{AtomicU8, Ordering};

use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    gpio::Input,
    ledc::{
        channel::{Channel, ChannelHW},
        LowSpeed,
    },
};
use heapless::Vec;

use super::CommandError;
use crate::fan_logic;

// The PWM owner runs independently of the control/bridge tasks. A failed bridge
// heartbeat can therefore force full speed even while a USB request is pending.
static REQUESTED_DUTY: AtomicU8 = AtomicU8::new(0);

pub struct Pins<'d> {
    pub tach: Input<'d>,
}

pub fn requested_duty() -> u8 {
    REQUESTED_DUTY.load(Ordering::Relaxed)
}

pub fn force_full_speed() {
    REQUESTED_DUTY.store(0, Ordering::Relaxed);
}

#[embassy_executor::task]
pub async fn pwm_task(pwm: Channel<'static, LowSpeed>) -> ! {
    let mut current = 0;
    pwm.set_duty_hw(current as u32);
    loop {
        let requested = REQUESTED_DUTY.load(Ordering::Relaxed);
        if requested != current {
            pwm.set_duty_hw(requested as u32);
            current = requested;
        }
        Timer::after_millis(10).await;
    }
}

#[derive(defmt::Format)]
pub enum Command {
    SetDuty(u8),  // 0x00: native eight-bit active-low PWM
    ReadRpm32,    // 0x01: native little-endian u32 RPM
    SetSpeed(u8), // 0x10: existing Bonanza percent API
    ReadRpm16,    // 0x20: existing Bonanza little-endian u16 RPM
}

impl Command {
    pub fn from_bytes(buf: &[u8]) -> Result<Self, CommandError> {
        match buf {
            [0x00, duty] => Ok(Self::SetDuty(*duty)),
            [0x01] => Ok(Self::ReadRpm32),
            [0x10, speed] if *speed <= 100 => Ok(Self::SetSpeed(*speed)),
            [0x20] => Ok(Self::ReadRpm16),
            _ => Err(CommandError::Invalid),
        }
    }
}

impl Pins<'_> {
    async fn read_rpm(&mut self) -> Result<u32, CommandError> {
        let start = Instant::now();
        let mut pulses = 0u32;
        let mut was_high = self.tach.is_high();
        while start.elapsed() < Duration::from_millis(fan_logic::TACH_SAMPLE_MS) {
            let is_high = self.tach.is_high();
            if was_high && !is_high {
                pulses = pulses.saturating_add(1);
            }
            was_high = is_high;
            Timer::after_micros(100).await;
        }
        fan_logic::rpm_from_pulses(pulses, start.elapsed().as_micros()).ok_or(CommandError::Invalid)
    }
}

impl super::ControllerCommand for Command {
    async fn handle(
        &self,
        controller: &mut super::Controller,
    ) -> Result<Vec<u8, 256>, CommandError> {
        match self {
            Command::SetDuty(duty) => {
                if *duty != 0 && super::gpio::fault_latched() {
                    return Err(CommandError::Fault);
                }
                REQUESTED_DUTY.store(*duty, Ordering::Relaxed);
                Ok(Vec::from_slice(&[*duty]).unwrap())
            }
            Command::SetSpeed(speed) => {
                let duty = fan_logic::percent_to_raw_duty(*speed).ok_or(CommandError::Invalid)?;
                if duty != 0 && super::gpio::fault_latched() {
                    return Err(CommandError::Fault);
                }
                REQUESTED_DUTY.store(duty, Ordering::Relaxed);
                Ok(Vec::from_slice(&[0]).unwrap())
            }
            Command::ReadRpm32 => {
                let rpm = controller.fan.read_rpm().await?;
                Ok(Vec::from_slice(&rpm.to_le_bytes()).unwrap())
            }
            Command::ReadRpm16 => {
                let rpm = controller.fan.read_rpm().await?;
                let rpm = u16::try_from(rpm).map_err(|_| CommandError::Invalid)?;
                Ok(Vec::from_slice(&rpm.to_le_bytes()).unwrap())
            }
        }
    }
}
