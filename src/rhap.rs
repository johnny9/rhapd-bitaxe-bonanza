//! RHAP framing shared by the firmware and host tests; no hardware dependencies.
pub const MAX_REQUEST: usize = 4098;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Success = 0,
    Timeout = 0x10,
    Invalid = 0x11,
    BufferOverflow = 0x12,
    Message = 0xff,
}

pub fn response(out: &mut [u8], id: u8, status: Status, payload: &[u8]) -> Option<usize> {
    let length = payload.len().checked_add(4)?;
    if length > out.len() || length > u16::MAX as usize {
        return None;
    }
    out[..2].copy_from_slice(&(length as u16).to_le_bytes());
    out[2] = id;
    out[3] = status as u8;
    out[4..length].copy_from_slice(payload);
    Some(length)
}

pub struct Decoder {
    buf: [u8; MAX_REQUEST],
    len: usize,
}
impl Decoder {
    pub const fn new() -> Self {
        Self {
            buf: [0; MAX_REQUEST],
            len: 0,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn id(&self) -> u8 {
        if self.len >= 3 {
            self.buf[2]
        } else {
            0xff
        }
    }
    pub fn clear(&mut self) {
        self.len = 0;
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), ()> {
        if bytes.len() > self.buf.len() - self.len {
            return Err(());
        }
        self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
    pub fn frame(&self) -> Result<Option<&[u8]>, ()> {
        if self.len < 2 {
            return Ok(None);
        }
        let length = u16::from_le_bytes([self.buf[0], self.buf[1]]) as usize;
        if !(6..=MAX_REQUEST).contains(&length) {
            return Err(());
        }
        Ok((self.len >= length).then_some(&self.buf[..length]))
    }
    pub fn consume(&mut self) {
        let length = self
            .frame()
            .ok()
            .flatten()
            .expect("complete validated frame")
            .len();
        self.buf.copy_within(length..self.len, 0);
        self.len -= length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success_data_cannot_be_confused_with_error_status() {
        let mut buf = [0; 260];
        assert_eq!(response(&mut buf, 42, Status::Success, &[0x10]), Some(5));
        assert_eq!(&buf[..5], &[5, 0, 42, 0, 0x10]);
        assert_eq!(response(&mut buf, 42, Status::Timeout, &[]), Some(4));
        assert_eq!(&buf[..4], &[4, 0, 42, 0x10]);
    }
    #[test]
    fn lengths_include_status_and_full_payload() {
        let mut buf = [0; 260];
        assert_eq!(
            response(&mut buf, 255, Status::Success, &[0xa5; 256]),
            Some(260)
        );
        assert_eq!(&buf[..4], &[4, 1, 255, 0]);
        assert_eq!(response(&mut buf, 1, Status::Message, &[1; 257]), None);
        for status in [Status::Invalid, Status::BufferOverflow, Status::Message] {
            assert_eq!(response(&mut buf, 7, status, &[]), Some(4));
            assert_eq!(buf[3], status as u8);
        }
    }
    #[test]
    fn accepts_every_split_and_coalesced_requests() {
        let frame = [7, 0, 42, 0, 6, 0, 1];
        for split in 0..frame.len() {
            let mut d = Decoder::new();
            d.push(&frame[..split]).unwrap();
            assert_eq!(d.frame(), Ok(None));
            d.push(&frame[split..]).unwrap();
            assert_eq!(d.frame(), Ok(Some(frame.as_slice())));
            d.push(&[6, 0, 43, 0, 0, 1]).unwrap();
            d.consume();
            assert_eq!(d.frame(), Ok(Some([6, 0, 43, 0, 0, 1].as_slice())));
            d.consume();
            assert!(d.is_empty());
        }
    }
    #[test]
    fn rejects_invalid_lengths_without_indexing_or_wrapping() {
        for length in [0u16, 1, 2, 3, 4, 5, 4099, 65535] {
            let mut d = Decoder::new();
            d.push(&length.to_le_bytes()).unwrap();
            assert_eq!(d.frame(), Err(()));
            d.clear();
            assert!(d.is_empty());
        }
        let mut d = Decoder::new();
        assert!(d.push(&[0; MAX_REQUEST + 1]).is_err());
        assert_eq!(d.id(), 255);
    }
    #[test]
    fn keeps_incomplete_tail_for_next_usb_packet() {
        let mut d = Decoder::new();
        d.push(&[6, 0, 1, 0, 0, 1, 7, 0, 2]).unwrap();
        d.consume();
        assert_eq!(d.id(), 2);
        assert_eq!(d.frame(), Ok(None));
        d.push(&[0, 6, 0, 1]).unwrap();
        assert_eq!(d.frame().unwrap().unwrap().len(), 7);
    }
}
