//! RP2040 multidrop SWD and MEM-AP transport on 1004x GPIO14/GPIO15.
//! Protocol constants: ARM ADIv5 and RP2040 datasheet section 2.3.4.

use embassy_time::{Duration, Instant, Timer};
use esp_hal::{
    delay::Delay,
    gpio::{Flex, InputConfig, Pull},
};

pub type Result<T> = core::result::Result<T, &'static str>;
pub const CORE0: u32 = 0x01002927;
pub const CORE1: u32 = 0x11002927;
pub const DPIDR: u32 = 0x0bc12477;
const DHCSR: u32 = 0xe000edf0;
const DEBUG_KEY: u32 = 0xa05f0000;
const STUB: u32 = 0x20030000;
const DATA: u32 = 0x20032000;
const CONTEXT: u32 = 0x2003ff00;
const STACK: u32 = 0x20042000;
const PAYLOAD: &[u8] = include_bytes!("../flash_stub/stub.bin");

pub struct Swd<'d> {
    clock: Flex<'d>,
    data: Flex<'d>,
    delay: Delay,
}

impl<'d> Swd<'d> {
    pub fn new(mut clock: Flex<'d>, mut data: Flex<'d>) -> Self {
        clock.set_low();
        clock.set_output_enable(false);
        data.apply_input_config(&InputConfig::default().with_pull(Pull::Up));
        data.set_input_enable(true);
        data.set_output_enable(false);
        Self {
            clock,
            data,
            delay: Delay::new(),
        }
    }

    pub fn release(&mut self) {
        self.clock.set_output_enable(false);
        self.data.set_output_enable(false);
    }

    fn pulse(&mut self) {
        // Keep SWCLK high between bits. The target changes its output on
        // falling edges; sample during the low phase, before the next rise.
        self.clock.set_low();
        self.delay.delay_micros(2);
        self.clock.set_high();
        self.delay.delay_micros(2);
    }

    fn write_bits(&mut self, value: u32, count: usize) {
        for bit in 0..count {
            self.data.set_level(((value >> bit) & 1 != 0).into());
            self.pulse();
        }
    }

    fn read_bits(&mut self, count: usize) -> u32 {
        let mut value = 0;
        for bit in 0..count {
            self.clock.set_low();
            self.delay.delay_micros(2);
            if self.data.is_high() {
                value |= 1 << bit;
            }
            self.clock.set_high();
            self.delay.delay_micros(2);
        }
        value
    }

    fn input_turnaround(&mut self) {
        self.data.set_output_enable(false);
        self.pulse();
    }

    fn output_turnaround(&mut self) {
        self.pulse();
        self.data.set_low();
        self.data.set_output_enable(true);
    }

    fn sequence(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.write_bits(u32::from(*byte), 8);
        }
    }

    fn transfer(&mut self, ap: bool, read: bool, address: u8, value: u32) -> Result<u32> {
        let request = crate::flash_logic::swd_request(ap, read, address);
        for _ in 0..16 {
            self.write_bits(u32::from(request), 8);
            self.input_turnaround();
            let ack = self.read_bits(3);
            if ack != 1 {
                self.output_turnaround();
                self.write_bits(0, 8);
                if ack == 2 {
                    continue;
                }
                return Err(if ack == 4 {
                    "SWD FAULT acknowledgement"
                } else {
                    "No RP2040 SWD acknowledgement"
                });
            }
            let result;
            if read {
                result = self.read_bits(32);
                let parity = self.read_bits(1);
                self.output_turnaround();
                self.write_bits(0, 8);
                if parity != result.count_ones() & 1 {
                    return Err("SWD read parity error");
                }
            } else {
                self.output_turnaround();
                self.write_bits(value, 32);
                self.write_bits(value.count_ones() & 1, 1);
                self.write_bits(0, 8);
                result = 0;
            }
            return Ok(result);
        }
        Err("SWD WAIT retry limit")
    }

    fn dp_write(&mut self, address: u8, value: u32) -> Result<()> {
        self.transfer(false, false, address, value).map(|_| ())
    }
    fn dp_read(&mut self, address: u8) -> Result<u32> {
        self.transfer(false, true, address, 0)
    }
    fn ap_write(&mut self, address: u8, value: u32) -> Result<()> {
        self.transfer(true, false, address, value)?;
        self.dp_read(0x0c)?; // Drain posted writes before polling or changing target.
        Ok(())
    }
    fn ap_read(&mut self, address: u8) -> Result<u32> {
        self.transfer(true, true, address, 0)?;
        self.dp_read(0x0c)
    }

    pub async fn attach(&mut self, target: u32) -> Result<u32> {
        self.clock.set_high();
        self.clock.set_output_enable(true);
        self.data.set_high();
        self.data.set_output_enable(true);
        // JTAG-to-dormant, selection-alert, SWD activation, line reset + idle.
        self.sequence(&[0xff, 0x75, 0x77, 0x77, 0x67]);
        self.sequence(&[
            0xff, 0x92, 0xf3, 0x09, 0x62, 0x95, 0x2d, 0x85, 0x86, 0xe9, 0xaf, 0xdd, 0xe3, 0xa2,
            0x0e, 0xbc, 0x19, 0xa0, 0xf1, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0,
        ]);
        self.write_bits(0x99, 8); // TARGETSEL: target intentionally does not ACK.
        self.input_turnaround();
        let _ = self.read_bits(3);
        self.output_turnaround();
        self.write_bits(target, 32);
        self.write_bits(target.count_ones() & 1, 1);
        self.write_bits(0, 8);
        let id = self
            .dp_read(0)
            .map_err(|_| "SWD attach: DPIDR read failed")?;
        if id != DPIDR {
            return Err("SWD target is not the expected RP2040 DAP");
        }
        self.dp_write(0, 0x1e)
            .map_err(|_| "SWD attach: ABORT write failed")?; // Clear sticky DP errors.
        self.dp_write(8, 0)
            .map_err(|_| "SWD attach: SELECT write failed")?; // APSEL=0, APBANKSEL=0.
        self.dp_write(4, 0x50000000)
            .map_err(|_| "SWD attach: power request failed")?;
        for _ in 0..32 {
            if self
                .dp_read(4)
                .map_err(|_| "SWD attach: power status read failed")?
                & 0xa0000000
                == 0xa0000000
            {
                self.ap_write(0, 0xa2000002)
                    .map_err(|_| "SWD attach: MEM-AP setup failed")?; // MEM-AP 32-bit, address increment off.
                return Ok(id);
            }
            Timer::after_millis(1).await;
        }
        Err("RP2040 debug power-up timeout")
    }

    pub fn mem_read(&mut self, address: u32) -> Result<u32> {
        self.ap_write(4, address)?;
        self.ap_read(0x0c)
    }
    fn mem_write(&mut self, address: u32, value: u32) -> Result<()> {
        self.ap_write(4, address)?;
        self.ap_write(0x0c, value)
    }

    async fn write_block(&mut self, address: u32, data: &[u8]) -> Result<()> {
        for (i, bytes) in data.chunks(4).enumerate() {
            let mut word = [0; 4];
            word[..bytes.len()].copy_from_slice(bytes);
            self.mem_write(address + i as u32 * 4, u32::from_le_bytes(word))?;
            if i % 8 == 7 {
                Timer::after_micros(1).await;
            }
        }
        Ok(())
    }

    async fn halt(&mut self) -> Result<()> {
        self.mem_write(DHCSR, DEBUG_KEY | 3)?;
        for _ in 0..128 {
            if self.mem_read(DHCSR)? & (1 << 17) != 0 {
                return Ok(());
            }
            Timer::after_millis(1).await;
        }
        Err("RP2040 core halt timeout")
    }

    pub async fn prepare(&mut self) -> Result<()> {
        self.attach(CORE1).await?;
        self.halt().await?;
        self.attach(CORE0).await?;
        self.halt().await?;
        // Halting CPUs does not stop peripherals. Hold DMA in reset so an old
        // UART DMA ring cannot overwrite the SRAM algorithm/buffer. Disable the
        // watchdog for this maintenance session; SYSRESETREQ restores defaults.
        self.mem_write(0x4000e000, 1 << 2)?; // RESETS_RESET atomic SET: DMA.
        self.mem_write(0x40058000, 0)?; // WATCHDOG_CTRL.
        self.write_block(STUB, PAYLOAD).await?;
        // Confirm the uploaded SRAM algorithm before allowing any flash erase.
        for (i, bytes) in PAYLOAD.chunks(4).enumerate() {
            let actual = self.mem_read(STUB + i as u32 * 4)?.to_le_bytes();
            if actual[..bytes.len()] != *bytes {
                return Err("RP2040 SRAM algorithm upload verification failed");
            }
            if i % 8 == 7 {
                Timer::after_micros(1).await;
            }
        }
        Ok(())
    }

    async fn core_register(&mut self, register: u32, value: u32) -> Result<()> {
        self.mem_write(0xe000edf8, value)?;
        self.mem_write(0xe000edf4, (1 << 16) | register)?;
        for _ in 0..128 {
            if self.mem_read(DHCSR)? & (1 << 16) != 0 {
                return Ok(());
            }
            Timer::after_millis(1).await;
        }
        Err("RP2040 core register write timeout")
    }

    async fn run(&mut self, operation: u32, offset: u32, length: u32) -> Result<()> {
        let mut context = [0u8; 20];
        for (i, value) in [0, operation, offset, length, DATA].iter().enumerate() {
            context[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        self.write_block(CONTEXT, &context).await?;
        self.core_register(0, CONTEXT).await?;
        self.core_register(17, STACK).await?;
        self.core_register(20, 1).await?; // Privileged MSP, PRIMASK=1 before resuming.
        self.core_register(16, 0x01000000).await?;
        self.core_register(15, STUB | 1).await?;
        self.mem_write(DHCSR, DEBUG_KEY | 1)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if self.mem_read(DHCSR)? & (1 << 17) != 0 {
                let status = self.mem_read(CONTEXT)?;
                if status == 0xd00dfeed {
                    return Ok(());
                }
                if status != 0 {
                    return Err("RP2040 ROM flash algorithm failed");
                }
            }
            if Instant::now() >= deadline {
                self.halt().await?;
                return Err("RP2040 ROM flash operation timeout");
            }
            Timer::after_millis(1).await;
        }
    }

    pub async fn program_page(&mut self, offset: u32, page: &[u8; 256]) -> Result<[u8; 256]> {
        if offset % 4096 == 0 {
            self.run(1, offset, 4096).await?;
        }
        self.write_block(DATA, page).await?;
        self.run(2, offset, 256).await?;
        let mut actual = [0u8; 256];
        for (i, bytes) in actual.chunks_mut(4).enumerate() {
            bytes.copy_from_slice(
                &self
                    .mem_read(0x10000000 + offset + i as u32 * 4)?
                    .to_le_bytes(),
            );
            if i % 8 == 7 {
                Timer::after_micros(1).await;
            }
        }
        if actual != *page {
            return Err("RP2040 flash page readback mismatch");
        }
        Ok(actual)
    }

    pub async fn reset(&mut self) -> Result<()> {
        self.mem_write(0xe000ed0c, 0x05fa0004)?;
        Timer::after_millis(10).await;
        // Debug registers survive SYSRESETREQ; release both cores explicitly.
        self.attach(CORE1).await?;
        self.mem_write(DHCSR, DEBUG_KEY)?;
        self.attach(CORE0).await?;
        self.mem_write(DHCSR, DEBUG_KEY)?;
        Ok(())
    }
}
