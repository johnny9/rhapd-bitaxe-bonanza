use defmt::info;
use esp_hal::{
    delay::Delay,
    peripherals::{LPWR, USB0},
    system,
};
use heapless::Vec;

use super::CommandError;

// Commands require a magic payload to guard against accidental reboot
// from line noise or a framing desync.
const REBOOT_MAGIC: [u8; 4] = [0xde, 0xad, 0xbe, 0xef];
const REBOOT_TO_BOOTLOADER_MAGIC: [u8; 4] = [0xb0, 0x07, 0x10, 0xad];

#[derive(defmt::Format)]
pub enum Command {
    Reboot,
    RebootToBootloader,
}

impl Command {
    pub fn from_bytes(buf: &[u8]) -> Result<Self, CommandError> {
        match buf {
            [0x01, rest @ ..] if rest == REBOOT_MAGIC => Ok(Self::Reboot),
            [0x02, rest @ ..] if rest == REBOOT_TO_BOOTLOADER_MAGIC => Ok(Self::RebootToBootloader),
            _ => Err(CommandError::Invalid),
        }
    }
}

impl super::ControllerCommand for Command {
    async fn handle(
        &self,
        _controller: &mut super::Controller,
    ) -> Result<Vec<u8, 256>, CommandError> {
        super::gpio::shutdown();
        // Let the independent PWM owner apply full fan before the ESP resets.
        embassy_time::Timer::after_millis(20).await;
        match self {
            Command::Reboot => {
                info!("System: rebooting");
            }
            Command::RebootToBootloader => {
                info!("System: rebooting to bootloader");
                // Hand the USB PHY back to the USB-Serial-JTAG
                // peripheral. The OTG selection made survives a
                // software reset, and the ROM would otherwise enter its
                // USB-OTG download mode, which espflash does not
                // handle. Keep interrupts masked from the switch
                // through the reset, so the OTG core does not interfere
                // with the reset.
                critical_section::with(|_| {
                    USB0::regs().dctl().modify(|_, w| w.sftdiscon().set_bit());

                    // Detach from the host long enough for it to
                    // notice, then attach as the USB-Serial-JTAG
                    // device. The OTG core releases its pull-up on soft
                    // disconnect; the USB-Serial-JTAG controller pulls
                    // up as soon as the PHY is its.
                    //
                    // A hub needs SE0 for only 2.5 us to see a detach.
                    // Add some margin in case a hub needs a polling
                    // interval to report it.
                    Delay::new().delay_millis(50);

                    LPWR::regs().usb_conf().modify(|_, w| {
                        w.sw_hw_usb_phy_sel().clear_bit();
                        w.sw_usb_phy_sel().clear_bit()
                    });
                    // The ROM reads this bit after reset and enters download
                    // mode, the programmatic equivalent of holding BOOT.
                    LPWR::regs()
                        .option1()
                        .modify(|_, w| w.force_download_boot().set_bit());
                    system::software_reset()
                })
            }
        }
        system::software_reset()
    }
}
