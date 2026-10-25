//! Image validation and ordered upload accounting; no hardware dependencies.

pub const CAPACITY: u32 = 2 * 1024 * 1024;
pub const PAGE_SIZE: usize = 256;
pub const HEADER_SIZE: usize = 264;

pub fn swd_request(ap: bool, read: bool, address: u8) -> u8 {
    let a2 = (address >> 2) & 1;
    let a3 = (address >> 3) & 1;
    let parity = ap as u8 ^ read as u8 ^ a2 ^ a3;
    0x81 | ((ap as u8) << 1) | ((read as u8) << 2) | (a2 << 3) | (a3 << 4) | (parity << 5)
}

pub fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    crc
}

pub fn boot2_crc(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in data {
        crc ^= u32::from(*byte) << 24;
        for _ in 0..8 {
            crc = (crc << 1) ^ (0x04c11db7 & 0u32.wrapping_sub(crc >> 31));
        }
    }
    crc
}

pub fn le32(data: &[u8]) -> u32 {
    u32::from_le_bytes(data[..4].try_into().unwrap())
}

pub fn valid_header(size: u32, header: &[u8; HEADER_SIZE]) -> bool {
    if size < HEADER_SIZE as u32
        || size > CAPACITY
        || boot2_crc(&header[..252]) != le32(&header[252..])
    {
        return false;
    }
    let sp = le32(&header[256..]);
    let reset = le32(&header[260..]);
    sp & 7 == 0
        && sp > 0x20000000
        && sp <= 0x20042000
        && reset & 1 != 0
        && reset & !1 >= 0x10000100
        && reset & !1 < 0x10000000 + size
}

pub struct Upload {
    pub size: u32,
    pub expected_crc: u32,
    pub offset: u32,
    pub crc: u32,
    header: [u8; HEADER_SIZE],
}

impl Upload {
    pub fn new(size: u32, crc: u32, header: [u8; HEADER_SIZE]) -> Option<Self> {
        valid_header(size, &header).then_some(Self {
            size,
            expected_crc: crc,
            offset: 0,
            crc: u32::MAX,
            header,
        })
    }

    pub fn accepts(&self, offset: u32, data: &[u8]) -> bool {
        if offset != self.offset || offset % PAGE_SIZE as u32 != 0 || offset >= self.size {
            return false;
        }
        let count = ((self.size - offset) as usize).min(PAGE_SIZE);
        if data.len() != count {
            return false;
        }
        for (i, byte) in data.iter().enumerate() {
            let address = offset as usize + i;
            if address < HEADER_SIZE && self.header[address] != *byte {
                return false;
            }
        }
        true
    }

    pub fn advance_verified(&mut self, actual_flash: &[u8]) {
        self.crc = crc32_update(self.crc, actual_flash);
        self.offset += actual_flash.len() as u32;
    }

    pub fn complete(&self) -> bool {
        self.offset == self.size && !self.crc == self.expected_crc
    }

    pub fn reply(&self) -> [u8; 9] {
        let mut result = [1u8; 9];
        result[1..5].copy_from_slice(&self.offset.to_le_bytes());
        result[5..9].copy_from_slice(&(!self.crc).to_le_bytes());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image() -> [u8; 300] {
        let mut image = [0xa5u8; 300];
        image[252..256].copy_from_slice(&boot2_crc(&[0xa5; 252]).to_le_bytes());
        image[256..260].copy_from_slice(&0x20042000u32.to_le_bytes());
        image[260..264].copy_from_slice(&0x10000109u32.to_le_bytes());
        image
    }
    #[test]
    fn swd_requests_match_arm_wire_encoding() {
        assert_eq!(swd_request(false, true, 0), 0xa5); // DPIDR read.
        assert_eq!(swd_request(false, false, 0x0c), 0x99); // TARGETSEL.
        assert_eq!(swd_request(true, true, 0x0c), 0x9f); // MEM-AP DRW read.
    }
    #[test]
    fn standard_crc_check_vectors() {
        assert_eq!(!crc32_update(u32::MAX, b"123456789"), 0xcbf43926);
        assert_eq!(boot2_crc(b"123456789"), 0x0376e6e7);
    }
    #[test]
    fn malformed_boot_vectors_and_sizes_rejected() {
        let image = image();
        let mut header: [u8; HEADER_SIZE] = image[..HEADER_SIZE].try_into().unwrap();
        assert!(valid_header(300, &header));
        assert!(!valid_header(CAPACITY + 1, &header));
        assert!(!valid_header(263, &header));
        header[0] ^= 1;
        assert!(!valid_header(300, &header));
        header[0] ^= 1;
        header[260] &= !1;
        assert!(!valid_header(300, &header));
    }
    #[test]
    fn ordered_pages_tail_and_crc_required_for_completion() {
        let image = image();
        let mut upload = Upload::new(
            300,
            !crc32_update(u32::MAX, &image),
            image[..HEADER_SIZE].try_into().unwrap(),
        )
        .unwrap();
        assert!(!upload.complete());
        assert!(!upload.accepts(256, &image[256..]));
        assert!(!upload.accepts(0, &image[..255]));
        assert!(upload.accepts(0, &image[..256]));
        upload.advance_verified(&image[..256]);
        assert!(!upload.accepts(0, &image[..256]));
        assert!(!upload.complete());
        assert!(upload.accepts(256, &image[256..]));
        upload.advance_verified(&image[256..]);
        assert!(upload.complete());
        upload.crc ^= 1;
        assert!(!upload.complete());
    }
    #[test]
    fn validated_prefix_cannot_be_substituted_before_erase() {
        let image = image();
        let upload = Upload::new(300, 0, image[..HEADER_SIZE].try_into().unwrap()).unwrap();
        let mut other = image;
        other[0] ^= 1;
        assert!(!upload.accepts(0, &other[..256]));
    }
}
