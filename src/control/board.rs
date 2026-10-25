//! Bonanza board extensions. RHAP System remains on page 0x09.
use super::CommandError;
use heapless::Vec;

#[derive(defmt::Format)]
pub enum Command {
    Info,
    Status,
}
impl Command {
    pub fn from_bytes(data: &[u8]) -> Result<Self, CommandError> {
        match data {
            [1] => Ok(Self::Info),
            [0x10] => Ok(Self::Status),
            _ => Err(CommandError::Invalid),
        }
    }
}
impl super::ControllerCommand for Command {
    async fn handle(
        &self,
        _controller: &mut super::Controller,
    ) -> Result<Vec<u8, 256>, CommandError> {
        match self {
            Self::Info => {
                let version = concat!(env!("CARGO_PKG_VERSION"), "+g", env!("GIT_HASH")).as_bytes();
                let mut payload = Vec::from_slice(&[1, 0xec, 0x03, version.len() as u8]).unwrap();
                payload.extend_from_slice(version).unwrap();
                Ok(payload)
            }
            Self::Status => Ok(Vec::from_slice(&super::gpio::status()).unwrap()),
        }
    }
}
