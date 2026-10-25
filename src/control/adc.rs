use embassy_time::Timer;
use esp_hal::{
    analog::adc::{Adc, AdcCalCurve, AdcPin},
    peripherals::{ADC1, GPIO3, GPIO4, GPIO5},
};
use heapless::Vec;

use super::CommandError;

// 1004x netlist: TP58/D1 -> R53 -> GPIO5; TP59/D2 -> R32 -> GPIO4;
// TP60/D3 -> R31 -> GPIO3. Each is a 5.1k series resistor plus 100nF to
// ground, with no divider. These are cumulative stack taps above board GND.
pub struct Pins<'d> {
    pub adc: Adc<'d, ADC1<'d>, esp_hal::Blocking>,
    pub domain1: AdcPin<GPIO5<'d>, ADC1<'d>, AdcCalCurve<ADC1<'d>>>,
    pub domain2: AdcPin<GPIO4<'d>, ADC1<'d>, AdcCalCurve<ADC1<'d>>>,
    pub domain3: AdcPin<GPIO3<'d>, ADC1<'d>, AdcCalCurve<ADC1<'d>>>,
}

#[derive(defmt::Format)]
pub enum Command {
    ReadDomain(u8),
    ReadAll,
}

impl Command {
    pub fn from_bytes(buf: &[u8]) -> Result<Self, CommandError> {
        match buf {
            [0x50] => Ok(Self::ReadDomain(1)),
            [0x51] => Ok(Self::ReadDomain(2)),
            [0x52] => Ok(Self::ReadDomain(3)),
            [0x53] => Ok(Self::ReadAll),
            _ => Err(CommandError::Invalid),
        }
    }
}

impl Pins<'_> {
    async fn read_mv(&mut self, domain: u8) -> Result<u16, CommandError> {
        let mut sum = 0u32;
        for _ in 0..16 {
            let mut sample = None;
            for _ in 0..100 {
                let result = match domain {
                    1 => self.adc.read_oneshot(&mut self.domain1),
                    2 => self.adc.read_oneshot(&mut self.domain2),
                    3 => self.adc.read_oneshot(&mut self.domain3),
                    _ => return Err(CommandError::Invalid),
                };
                if let Ok(value) = result {
                    sample = Some(value);
                    break;
                }
                // Yield while conversion runs so trip monitoring stays live.
                Timer::after_micros(50).await;
            }
            sum += u32::from(sample.ok_or(CommandError::Message("ADC conversion timeout"))?);
        }
        Ok(((sum + 8) / 16) as u16)
    }
}

impl super::ControllerCommand for Command {
    async fn handle(
        &self,
        controller: &mut super::Controller,
    ) -> Result<Vec<u8, 256>, CommandError> {
        let mut response = Vec::new();
        match self {
            Command::ReadDomain(domain) => {
                let mv = controller.adc.read_mv(*domain).await?;
                response.extend_from_slice(&mv.to_le_bytes()).unwrap();
            }
            Command::ReadAll => {
                for domain in 1..=3 {
                    let mv = controller.adc.read_mv(domain).await?;
                    response.extend_from_slice(&mv.to_le_bytes()).unwrap();
                }
            }
        }
        Ok(response)
    }
}
