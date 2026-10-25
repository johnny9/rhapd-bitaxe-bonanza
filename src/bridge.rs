//! The RP2040 owns only transport. Physical safety stays on this ESP32.
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{with_timeout, Duration, Timer};
use embedded_io_async::Write;
use esp_hal::{uart::Uart, Async};
use heapless::Vec;

use crate::{
    bridge_protocol as protocol,
    control::{gpio, programmer, CommandError},
    uart,
};

pub struct Bridge {
    uart: Uart<'static, Async>,
    next_id: u8,
    stream: Option<protocol::Status>,
}

pub type BridgeMutex = Mutex<CriticalSectionRawMutex, Bridge>;
const TIMEOUT: Duration = Duration::from_millis(500);

pub fn initialize(uart: Uart<'static, Async>) -> &'static BridgeMutex {
    static CELL: static_cell::StaticCell<BridgeMutex> = static_cell::StaticCell::new();
    CELL.init(Mutex::new(Bridge {
        uart,
        next_id: 0,
        stream: None,
    }))
}

impl Bridge {
    async fn transact(&mut self, command: u8) -> Result<Vec<u8, 256>, CommandError> {
        with_timeout(TIMEOUT, self.exchange(command))
            .await
            .map_err(|_| CommandError::Timeout)?
    }

    async fn exchange(&mut self, command: u8) -> Result<Vec<u8, 256>, CommandError> {
        // A timeout/cancelled USB request may leave an old reply. Never accept it
        // as the next command's response, or carry a partial frame across calls.
        let mut discarded = [0; 128];
        while self.uart.read_ready() {
            self.uart
                .read_buffered(&mut discarded)
                .map_err(|_| CommandError::Fault)?;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.uart
            .write_all(&[6, 0, id, 0, 0, command])
            .await
            .map_err(|_| CommandError::Fault)?;
        self.uart
            .flush_async()
            .await
            .map_err(|_| CommandError::Fault)?;
        loop {
            let mut header = [0; 3];
            self.uart
                .read_exact_async(&mut header)
                .await
                .map_err(|_| CommandError::Fault)?;
            let length = u16::from_le_bytes([header[0], header[1]]) as usize;
            if !(4..=259).contains(&length) {
                return Err(CommandError::Invalid);
            }
            let mut payload = [0; 256];
            let payload = &mut payload[..length - 3];
            self.uart
                .read_exact_async(payload)
                .await
                .map_err(|_| CommandError::Fault)?;
            if header[2] != id {
                continue;
            }
            return match payload {
                [0x10] => Err(CommandError::Timeout),
                [0x11] => Err(CommandError::Invalid),
                [0x12] => Err(CommandError::Denied),
                [0x13] => Err(CommandError::Fault),
                _ => Vec::from_slice(payload).map_err(|_| CommandError::BufferOverflow),
            };
        }
    }

    async fn reset(&mut self) -> Result<Vec<u8, 256>, CommandError> {
        self.stream = None;
        if !protocol::valid_info(&self.transact(1).await?) {
            return Err(CommandError::Invalid);
        }
        if !protocol::valid_queues(&self.transact(5).await?) {
            return Err(CommandError::Invalid);
        }
        let before = protocol::status(&self.transact(3).await?).ok_or(CommandError::Invalid)?;
        let payload = self.transact(4).await?;
        let after = protocol::status(&payload).ok_or(CommandError::Invalid)?;
        if !protocol::reset_complete(before, after) {
            return Err(CommandError::Fault);
        }
        self.stream = Some(after);
        Ok(payload)
    }
}

pub async fn prepare_stream(bridge: &BridgeMutex) -> Result<Vec<u8, 256>, CommandError> {
    if programmer::is_active() || gpio::data_enabled() {
        return Err(CommandError::Denied);
    }
    // The UART owner remains paused on error/cancellation until another reset.
    // Neither a USB reconnect nor a diagnostic resumes it.
    let result = async {
        uart::pause().await?;
        let mut guard = bridge.lock().await;
        let payload = guard.reset().await?;
        uart::resume().await?; // drain ESP RX after the bridge's reset ACK
        Ok(payload)
    }
    .await;
    if result.is_err() {
        gpio::shutdown();
    }
    result
}

pub async fn command(bridge: &BridgeMutex, command: u8) -> Result<Vec<u8, 256>, CommandError> {
    if programmer::is_active() {
        return Err(CommandError::Denied);
    }
    if command == 4 {
        return prepare_stream(bridge).await;
    }
    let mut guard = bridge.lock().await;
    let payload = guard.transact(command).await?;
    let valid = match command {
        1 => protocol::valid_info(&payload),
        2 => payload.len() == 9 && payload[0] == 1,
        3 => protocol::status(&payload).is_some(),
        5 => protocol::valid_queues(&payload),
        _ => false,
    };
    if !valid {
        return Err(CommandError::Invalid);
    }
    Ok(payload)
}

#[embassy_executor::task]
pub async fn monitor_task(bridge: &'static BridgeMutex) -> ! {
    loop {
        Timer::after_millis(250).await;
        if !gpio::data_enabled() || programmer::is_active() {
            continue;
        }
        let mut guard = bridge.lock().await;
        if !gpio::data_enabled() || programmer::is_active() {
            continue;
        }
        let bridge = &mut *guard;
        let current = match bridge.transact(3).await {
            Ok(payload) => protocol::status(&payload),
            Err(_) => None,
        };
        match (bridge.stream, current) {
            (Some(before), Some(after)) if protocol::same_stream(before, after) => {
                bridge.stream = Some(after)
            }
            _ => {
                bridge.stream = None;
                gpio::shutdown();
            }
        }
    }
}
