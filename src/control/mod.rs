use defmt::info;

use embassy_futures::select::{select, Either};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_usb::{
    class::cdc_acm::{CdcAcmClass, Receiver, Sender},
    driver::EndpointError,
};
use heapless::Vec;

pub mod i2c;
const I2C_COMMAND: u8 = 5;

pub mod programmer;
const PROGRAMMER_COMMAND: u8 = 0x0a;

pub mod board;
const BOARD_COMMAND: u8 = 0;
pub mod system;
const SYSTEM_COMMAND: u8 = 9;

pub mod gpio;
const GPIO_COMMAND: u8 = 6;

pub mod adc;
const ADC_COMMAND: u8 = 7;

pub mod fan;
const FAN_COMMAND: u8 = 8;
const BRIDGE_COMMAND: u8 = 0x0b;

// pub mod led;
// const LED_COMMAND: u8 = 8;

#[derive(defmt::Format)]
struct Command {
    id: u8,
    inner: CommandInner,
}

#[allow(clippy::large_enum_variant)]
#[derive(defmt::Format)]
enum CommandInner {
    Bridge(u8),
    Programmer(programmer::Command),
    Board(board::Command),
    System(system::Command),
    I2c(i2c::Command),
    Gpio(gpio::Command),
    Adc(adc::Command),
    Fan(fan::Command),
    Error(CommandError),
}

impl Command {
    fn from_bytes(buf: &[u8]) -> Result<Self, CommandError> {
        let [id, bus, page, data @ ..] = buf else {
            return Err(CommandError::Invalid);
        };
        if *bus != 0 {
            return Err(CommandError::Invalid);
        }

        match *page {
            BRIDGE_COMMAND => match data {
                [command @ 1..=5] => Ok(Self {
                    id: *id,
                    inner: CommandInner::Bridge(*command),
                }),
                _ => Err(CommandError::Invalid),
            },
            PROGRAMMER_COMMAND => Ok(Self {
                id: *id,
                inner: CommandInner::Programmer(programmer::Command::from_bytes(data)?),
            }),
            BOARD_COMMAND => Ok(Self {
                id: *id,
                inner: CommandInner::Board(board::Command::from_bytes(data)?),
            }),
            SYSTEM_COMMAND => Ok(Self {
                id: *id,
                inner: CommandInner::System(system::Command::from_bytes(data)?),
            }),
            I2C_COMMAND => Ok(Self {
                id: *id,
                inner: CommandInner::I2c(i2c::Command::from_bytes(data)?),
            }),
            GPIO_COMMAND => Ok(Self {
                id: *id,
                inner: CommandInner::Gpio(gpio::Command::from_bytes(data)?),
            }),
            ADC_COMMAND => Ok(Self {
                id: *id,
                inner: CommandInner::Adc(adc::Command::from_bytes(data)?),
            }),
            FAN_COMMAND => Ok(Self {
                id: *id,
                inner: CommandInner::Fan(fan::Command::from_bytes(data)?),
            }),
            _ => Err(CommandError::Invalid),
        }
    }
}

#[derive(defmt::Format)]
pub enum CommandError {
    Timeout, // 0x10
    Invalid, // 0x11
    Denied,  // RHAP message error
    Fault,   // RHAP message error
    BufferOverflow,
    Message(&'static str), // 0xff
}

impl CommandError {
    fn status(&self) -> crate::rhap::Status {
        use crate::rhap::Status;
        match self {
            Self::Timeout => Status::Timeout,
            Self::Invalid => Status::Invalid,
            Self::BufferOverflow => Status::BufferOverflow,
            Self::Denied | Self::Fault | Self::Message(_) => Status::Message,
        }
    }
    fn message(&self) -> &[u8] {
        match self {
            Self::Denied => b"Board command denied",
            Self::Fault => b"Board or bridge fault",
            Self::Message(message) => message.as_bytes(),
            _ => &[],
        }
    }
}

static COMMAND_CHANNEL: Channel<CriticalSectionRawMutex, Command, 8> = Channel::new();

pub struct Controller {
    tx: Sender<'static, super::UsbDriver>,
    i2c: super::I2cDriver,
    adc: adc::Pins<'static>,
    fan: fan::Pins<'static>,
    programmer: programmer::Programmer<'static>,
    bridge: &'static crate::bridge::BridgeMutex,
}

pub trait ControllerCommand {
    async fn handle(&self, controller: &mut Controller) -> Result<Vec<u8, 256>, CommandError>;
}

impl Controller {
    pub async fn run(&mut self) -> Result<(), ControlTaskError> {
        loop {
            let cmd = COMMAND_CHANNEL.receive().await;
            let res = match cmd.inner {
                CommandInner::Bridge(command) => crate::bridge::command(self.bridge, command).await,
                CommandInner::Programmer(cmd) => self.programmer.handle(cmd).await,
                CommandInner::I2c(_) if programmer::is_active() => Err(CommandError::Denied),
                CommandInner::Board(cmd) => cmd.handle(self).await,
                CommandInner::System(cmd) => cmd.handle(self).await,
                CommandInner::I2c(cmd) => cmd.handle(self).await,
                CommandInner::Gpio(cmd) => cmd.handle(self).await,
                CommandInner::Adc(cmd) => cmd.handle(self).await,
                CommandInner::Fan(cmd) => cmd.handle(self).await,
                CommandInner::Error(err) => Err(err),
            };

            let mut buf = [0; 260];
            let length = match res {
                Ok(ref payload) => {
                    crate::rhap::response(&mut buf, cmd.id, crate::rhap::Status::Success, payload)
                }
                Err(ref error) => {
                    crate::rhap::response(&mut buf, cmd.id, error.status(), error.message())
                }
            }
            .unwrap();
            for packet in buf[..length].chunks(64) {
                self.tx.write_packet(packet).await?;
            }
            if length % 64 == 0 {
                self.tx.write_packet(&[]).await?;
            }
        }
    }
}

#[embassy_executor::task]
pub async fn usb_task(
    class: CdcAcmClass<'static, super::UsbDriver>,
    i2c: super::I2cDriver,
    adc: adc::Pins<'static>,
    fan: fan::Pins<'static>,
    programmer: programmer::Programmer<'static>,
    bridge: &'static crate::bridge::BridgeMutex,
) -> ! {
    let (tx, mut rx, mut ctrl) = class.split_with_control();
    let mut controller = Controller {
        tx,
        i2c,
        adc,
        fan,
        programmer,
        bridge,
    };

    loop {
        rx.wait_connection().await;
        while !rx.dtr() {
            ctrl.control_changed().await;
        }
        info!("Control: Connected");
        let _ = select(pipe_usb_read(&mut rx, &mut ctrl), controller.run()).await;
        controller.programmer.abort();
        fan::force_full_speed();
        gpio::shutdown();
        while COMMAND_CHANNEL.try_receive().is_ok() {}
        info!("Control: Disconnected");
    }
}

pub enum ControlTaskError {
    Disconnected,
}

impl From<EndpointError> for ControlTaskError {
    fn from(val: EndpointError) -> Self {
        match val {
            EndpointError::BufferOverflow => ControlTaskError::Disconnected,
            EndpointError::Disabled => ControlTaskError::Disconnected {},
        }
    }
}

async fn pipe_usb_read(
    rx: &mut Receiver<'static, super::UsbDriver>,
    ctrl: &mut embassy_usb::class::cdc_acm::ControlChanged<'static>,
) -> Result<(), ControlTaskError> {
    let mut decoder = crate::rhap::Decoder::new();
    let mut packet = [0; 64];
    loop {
        let read = select(rx.read_packet(&mut packet), ctrl.control_changed());
        let result = if decoder.is_empty() {
            Ok(read.await)
        } else {
            embassy_time::with_timeout(embassy_time::Duration::from_millis(100), read).await
        };
        let n = match result {
            Ok(Either::First(result)) => result?,
            Ok(Either::Second(())) => {
                if !rx.dtr() {
                    return Err(ControlTaskError::Disconnected);
                }
                continue;
            }
            Err(_) => {
                COMMAND_CHANNEL
                    .send(Command {
                        id: decoder.id(),
                        inner: CommandInner::Error(CommandError::Timeout),
                    })
                    .await;
                decoder.clear();
                continue;
            }
        };
        if decoder.push(&packet[..n]).is_err() {
            COMMAND_CHANNEL
                .send(Command {
                    id: decoder.id(),
                    inner: CommandInner::Error(CommandError::BufferOverflow),
                })
                .await;
            decoder.clear();
            continue;
        }
        loop {
            let command = match decoder.frame() {
                Ok(Some(frame)) => match Command::from_bytes(&frame[2..]) {
                    Ok(command) => command,
                    Err(error) => Command {
                        id: frame[2],
                        inner: CommandInner::Error(error),
                    },
                },
                Ok(None) => break,
                Err(_) => {
                    COMMAND_CHANNEL
                        .send(Command {
                            id: decoder.id(),
                            inner: CommandInner::Error(CommandError::Invalid),
                        })
                        .await;
                    decoder.clear();
                    break;
                }
            };
            decoder.consume();
            COMMAND_CHANNEL.send(command).await;
        }
    }
}
