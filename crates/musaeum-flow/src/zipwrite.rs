//! A ZIP archive written in memory, with fixed timestamps so the same files give the same bytes.

use std::io::Write;

use flate2::write::DeflateEncoder;
use flate2::{Compression, Crc};

pub struct ZipWriter {
    out: Vec<u8>,
    central: Vec<u8>,
    entries: u16,
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ZipWriter {
    pub fn new() -> Self {
        Self {
            out: Vec::new(),
            central: Vec::new(),
            entries: 0,
        }
    }

    pub fn add(&mut self, name: &str, data: &[u8], compress: bool) {
        const UTF8_NAMES: u16 = 1 << 11;
        const JANUARY_1980: u16 = 0x21;
        let mut crc = Crc::new();
        crc.update(data);
        let compressed;
        let (method, body): (u16, &[u8]) = if compress {
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(6));
            encoder.write_all(data).expect("writing to memory");
            compressed = encoder.finish().expect("writing to memory");
            (8, &compressed)
        } else {
            (0, data)
        };
        let mut fields = Vec::with_capacity(26);
        fields.extend_from_slice(&20u16.to_le_bytes());
        fields.extend_from_slice(&UTF8_NAMES.to_le_bytes());
        fields.extend_from_slice(&method.to_le_bytes());
        fields.extend_from_slice(&0u16.to_le_bytes());
        fields.extend_from_slice(&JANUARY_1980.to_le_bytes());
        fields.extend_from_slice(&crc.sum().to_le_bytes());
        fields.extend_from_slice(&(body.len() as u32).to_le_bytes());
        fields.extend_from_slice(&(data.len() as u32).to_le_bytes());
        fields.extend_from_slice(&(name.len() as u16).to_le_bytes());
        fields.extend_from_slice(&0u16.to_le_bytes());

        let position = self.out.len() as u32;
        self.central.extend_from_slice(b"PK\x01\x02");
        self.central.extend_from_slice(&20u16.to_le_bytes());
        self.central.extend_from_slice(&fields);
        self.central.extend_from_slice(&[0; 10]);
        self.central.extend_from_slice(&position.to_le_bytes());
        self.central.extend_from_slice(name.as_bytes());
        self.entries += 1;

        self.out.extend_from_slice(b"PK\x03\x04");
        self.out.extend_from_slice(&fields);
        self.out.extend_from_slice(name.as_bytes());
        self.out.extend_from_slice(body);
    }

    pub fn finish(mut self) -> Vec<u8> {
        let directory_at = self.out.len() as u32;
        let central = std::mem::take(&mut self.central);
        self.out.extend_from_slice(&central);
        self.out.extend_from_slice(b"PK\x05\x06");
        self.out.extend_from_slice(&[0; 4]);
        self.out.extend_from_slice(&self.entries.to_le_bytes());
        self.out.extend_from_slice(&self.entries.to_le_bytes());
        self.out
            .extend_from_slice(&(central.len() as u32).to_le_bytes());
        self.out.extend_from_slice(&directory_at.to_le_bytes());
        self.out.extend_from_slice(&[0; 2]);
        self.out
    }
}
