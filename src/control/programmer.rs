//! Streaming RP2040 flash service. No image-sized ESP32 allocation is needed.

use super::CommandError;
use crate::{
    flash_logic::{self, Upload, HEADER_SIZE, PAGE_SIZE},
    swd::Swd,
};
use core::sync::atomic::{AtomicBool, Ordering};
use heapless::Vec;

static ACTIVE: AtomicBool = AtomicBool::new(false);
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

#[derive(defmt::Format)]
pub enum Command {
    Capabilities,
    Probe,
    Begin {
        size: u32,
        crc: u32,
        header: [u8; HEADER_SIZE],
    },
    Page {
        offset: u32,
        data: Vec<u8, PAGE_SIZE>,
    },
    Commit,
    Abort,
}

impl Command {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CommandError> {
        match bytes {
            [0] => Ok(Self::Capabilities),
            [1] => Ok(Self::Probe),
            [0x10, data @ ..] if data.len() == 8 + HEADER_SIZE => Ok(Self::Begin {
                size: flash_logic::le32(data),
                crc: flash_logic::le32(&data[4..]),
                header: data[8..].try_into().unwrap(),
            }),
            [0x20, data @ ..] if data.len() > 4 && data.len() <= 4 + PAGE_SIZE => Ok(Self::Page {
                offset: flash_logic::le32(data),
                data: Vec::from_slice(&data[4..]).unwrap(),
            }),
            [0x30] => Ok(Self::Commit),
            [0x7f] => Ok(Self::Abort),
            _ => Err(CommandError::Invalid),
        }
    }
}

pub struct Programmer<'d> {
    swd: Swd<'d>,
    upload: Option<Upload>,
}

impl<'d> Programmer<'d> {
    pub fn new(swd: Swd<'d>) -> Self {
        Self { swd, upload: None }
    }

    pub fn abort(&mut self) {
        // Do not reset a partly written target. A later BEGIN can halt/recover it.
        self.swd.release();
        self.upload = None;
        ACTIVE.store(false, Ordering::Relaxed);
    }

    pub async fn handle(&mut self, command: Command) -> Result<Vec<u8, 256>, CommandError> {
        let result = self.execute(command).await;
        if result.is_err() {
            self.abort();
        }
        result.map_err(CommandError::Message)
    }

    async fn execute(&mut self, command: Command) -> Result<Vec<u8, 256>, &'static str> {
        match command {
            Command::Capabilities => {
                let mut data = Vec::from_slice(&[1, 1]).unwrap();
                data.extend_from_slice(&(PAGE_SIZE as u16).to_le_bytes())
                    .unwrap();
                data.extend_from_slice(&flash_logic::CAPACITY.to_le_bytes())
                    .unwrap();
                data.extend_from_slice(&[14, 15]).unwrap();
                Ok(data)
            }
            Command::Probe => {
                if is_active() {
                    return Err("SWD programming already active");
                }
                let result = self.swd.attach(crate::swd::CORE0).await;
                self.swd.release();
                Ok(Vec::from_slice(&result?.to_le_bytes()).unwrap())
            }
            Command::Begin { size, crc, header } => {
                if is_active() {
                    return Err("SWD programming already active; abort and restart");
                }
                let upload =
                    Upload::new(size, crc, header).ok_or("Invalid RP2040 image header/size")?;
                if !super::gpio::prepare_programming() {
                    return Err("Disable TPS546 output and clear trip before SWD programming");
                }
                self.upload = Some(upload);
                ACTIVE.store(true, Ordering::Relaxed);
                self.swd.prepare().await?;
                Ok(Vec::from_slice(&self.upload.as_ref().unwrap().reply()).unwrap())
            }
            Command::Page { offset, data } => {
                let upload = self
                    .upload
                    .as_ref()
                    .ok_or("Start a SWD programming session first")?;
                if !upload.accepts(offset, &data) {
                    return Err("SWD page offset/length/header mismatch; restart upload");
                }
                if super::gpio::fault_latched() {
                    return Err("ASIC trip during SWD upload");
                }
                let mut padded = [0xffu8; PAGE_SIZE];
                padded[..data.len()].copy_from_slice(&data);
                let actual = self.swd.program_page(offset, &padded).await?;
                let upload = self.upload.as_mut().unwrap();
                upload.advance_verified(&actual[..data.len()]);
                Ok(Vec::from_slice(&upload.reply()).unwrap())
            }
            Command::Commit => {
                let upload = self.upload.as_ref().ok_or("No SWD programming session")?;
                if !upload.complete() {
                    return Err("Incomplete SWD upload or image CRC mismatch");
                }
                if super::gpio::fault_latched() {
                    return Err("ASIC trip before SWD commit");
                }
                let reply = upload.reply();
                self.swd.reset().await?;
                self.abort();
                Ok(Vec::from_slice(&reply).unwrap())
            }
            Command::Abort => {
                self.abort();
                Ok(Vec::from_slice(&[0]).unwrap())
            }
        }
    }
}
