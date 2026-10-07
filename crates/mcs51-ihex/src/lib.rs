//! # mcs51-ihex
//!
//! Firmware image containers for the `mcs51-rs` toolchain: Intel HEX, raw binary,
//! and a sparse in-memory image that both convert through.
//!
//! This is the Rust counterpart of SDCC's `packihx` (normalise an Intel HEX
//! file) and `makebin` (Intel HEX to raw binary for a ROM programmer), plus the
//! parts of `sdobjcopy` that the firmware build actually needs.
//!
//! ```
//! use mcs51_ihex::Image;
//!
//! let mut image = Image::new();
//! image.write(0x0000, &[0x02, 0x00, 0x30]); // LJMP 0x0030
//! let hex = image.to_intel_hex();
//! assert!(hex.contains(":03000000020030"));
//! assert_eq!(Image::from_intel_hex(&hex).unwrap(), image);
//! ```

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A sparse byte-addressed image of up to 64 KiB.
///
/// The 8051 code space is 64 KiB and firmware images are usually dense in a few
/// small windows, so a map keyed by address beats a 64 KiB array for both
/// reporting and merging.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Image {
    bytes: BTreeMap<u16, u8>,
}

/// Something went wrong reading or writing an image format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageError {
    /// A line did not start with `:`.
    MissingStartCode {
        /// 1-based line number.
        line: usize,
    },
    /// A line was not valid hexadecimal.
    BadHex {
        /// 1-based line number.
        line: usize,
        /// The offending text.
        text: String,
    },
    /// The byte count field did not match the number of data bytes.
    LengthMismatch {
        /// 1-based line number.
        line: usize,
        /// Byte count field.
        expected: usize,
        /// Bytes actually present.
        actual: usize,
    },
    /// The record checksum did not match.
    BadChecksum {
        /// 1-based line number.
        line: usize,
        /// Checksum from the file.
        found: u8,
        /// Checksum computed from the record.
        computed: u8,
    },
    /// The record type is not one this tool understands.
    UnsupportedRecord {
        /// 1-based line number.
        line: usize,
        /// The record type byte.
        kind: u8,
    },
    /// The file contains no data before EOF.
    Empty,
    /// Two records place different bytes at the same address.
    Conflict {
        /// The address in dispute.
        addr: u16,
        /// Byte already present.
        existing: u8,
        /// Byte the new record wants.
        incoming: u8,
    },
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageError::MissingStartCode { line } => {
                write!(f, "line {line}: Intel HEX records must start with ':'")
            }
            ImageError::BadHex { line, text } => {
                write!(f, "line {line}: not hexadecimal: {text}")
            }
            ImageError::LengthMismatch {
                line,
                expected,
                actual,
            } => write!(
                f,
                "line {line}: record declares {expected} data bytes but carries {actual}"
            ),
            ImageError::BadChecksum {
                line,
                found,
                computed,
            } => write!(
                f,
                "line {line}: checksum {found:#04X} does not match computed {computed:#04X}"
            ),
            ImageError::UnsupportedRecord { line, kind } => {
                write!(f, "line {line}: unsupported record type {kind:#04X}")
            }
            ImageError::Empty => write!(f, "the file contains no data records"),
            ImageError::Conflict {
                addr,
                existing,
                incoming,
            } => write!(
                f,
                "address {addr:#06X} is defined twice with different values \
                 ({existing:#04X} and {incoming:#04X})"
            ),
        }
    }
}

impl std::error::Error for ImageError {}

impl Image {
    /// An empty image.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build an image from a dense byte slice placed at `base`.
    pub fn from_bytes(base: u16, data: &[u8]) -> Self {
        let mut image = Self::new();
        image.write(base, data);
        image
    }

    /// `true` if no bytes are present.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Number of bytes actually present.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Lowest address present.
    pub fn min_addr(&self) -> Option<u16> {
        self.bytes.keys().next().copied()
    }

    /// Highest address present.
    pub fn max_addr(&self) -> Option<u16> {
        self.bytes.keys().next_back().copied()
    }

    /// Write `data` at `addr`, overwriting.
    pub fn write(&mut self, addr: u16, data: &[u8]) {
        for (i, byte) in data.iter().enumerate() {
            let a = addr.wrapping_add(i as u16);
            self.bytes.insert(a, *byte);
        }
    }

    /// Write `data` at `addr`, failing if a *different* byte is already there.
    pub fn write_checked(&mut self, addr: u16, data: &[u8]) -> Result<(), ImageError> {
        for (i, byte) in data.iter().enumerate() {
            let a = addr.wrapping_add(i as u16);
            if let Some(&existing) = self.bytes.get(&a)
                && existing != *byte
            {
                return Err(ImageError::Conflict {
                    addr: a,
                    existing,
                    incoming: *byte,
                });
            }
            self.bytes.insert(a, *byte);
        }
        Ok(())
    }

    /// Read one byte.
    pub fn get(&self, addr: u16) -> Option<u8> {
        self.bytes.get(&addr).copied()
    }

    /// Replace one byte.
    pub fn set(&mut self, addr: u16, value: u8) {
        self.bytes.insert(addr, value);
    }

    /// Remove one byte.
    pub fn clear(&mut self, addr: u16) {
        self.bytes.remove(&addr);
    }

    /// Copy a range out, returning `None` if any byte is missing.
    pub fn read(&self, addr: u16, len: usize) -> Option<Vec<u8>> {
        (0..len)
            .map(|i| self.bytes.get(&addr.wrapping_add(i as u16)).copied())
            .collect()
    }

    /// All present bytes, in ascending address order.
    pub fn iter(&self) -> impl Iterator<Item = (u16, u8)> + '_ {
        self.bytes.iter().map(|(a, b)| (*a, *b))
    }

    /// Contiguous runs of present bytes, as `(start, data)` pairs.
    ///
    /// This is what makes HEX and binary output well-formed: gaps become
    /// separate records rather than a wall of `0xFF` padding.
    pub fn runs(&self) -> Vec<(u16, Vec<u8>)> {
        let mut out: Vec<(u16, Vec<u8>)> = Vec::new();
        for (addr, byte) in self.iter() {
            match out.last_mut() {
                Some((start, data))
                    if start.wrapping_add(data.len() as u16) == addr
                        && data.len() < u16::MAX as usize =>
                {
                    data.push(byte);
                }
                _ => out.push((addr, vec![byte])),
            }
        }
        out
    }

    /// The image as MAP-file style lines: `addr : bytes  ASCII`.
    pub fn to_hex_dump(&self) -> String {
        let mut out = String::new();
        for (start, data) in self.runs() {
            for (i, chunk) in data.chunks(16).enumerate() {
                let addr = start.wrapping_add((i * 16) as u16);
                let _ = write!(out, "{addr:04X}:");
                for byte in chunk {
                    let _ = write!(out, " {byte:02X}");
                }
                for _ in chunk.len()..16 {
                    out.push_str("   ");
                }
                out.push_str("  ");
                for byte in chunk {
                    out.push(if (0x20..0x7F).contains(byte) {
                        *byte as char
                    } else {
                        '.'
                    });
                }
                out.push('\n');
            }
        }
        out
    }

    /// Total number of bytes, ignoring gaps, as a size report.
    pub fn size_report(&self) -> SizeReport {
        let mut report = SizeReport::default();
        for (start, data) in self.runs() {
            report.runs += 1;
            report.bytes += data.len();
            report.low = Some(report.low.map_or(start, |l: u16| l.min(start)));
            let end = start.wrapping_add(data.len() as u16 - 1);
            report.high = Some(report.high.map_or(end, |h: u16| h.max(end)));
        }
        report.span = match (report.low, report.high) {
            (Some(l), Some(h)) => Some((h as u32 + 1) - l as u32),
            _ => None,
        };
        report
    }

    /// Free bytes inside a region, given that region's start and length.
    pub fn free_in(&self, start: u16, len: u32) -> u32 {
        let mut used = 0u32;
        for addr in start as u32..start as u32 + len {
            if self.bytes.contains_key(&(addr as u16)) {
                used += 1;
            }
        }
        len - used
    }

    /// Serialise as Intel HEX with 16-byte records.
    pub fn to_intel_hex(&self) -> String {
        self.to_intel_hex_with(16)
    }

    /// Serialise as Intel HEX with `record_len`-byte records.
    ///
    /// Extended linear address records (`:02000004`) are emitted when the image
    /// crosses a 64 KiB boundary, which cannot happen on a plain 8051 but can on
    /// the DS80C390-family parts that the same toolchain can target.
    pub fn to_intel_hex_with(&self, record_len: usize) -> String {
        let record_len = record_len.clamp(1, 255);
        let mut out = String::new();
        let mut upper = 0u16;
        for (start, data) in self.runs() {
            let mut offset = 0usize;
            while offset < data.len() {
                let addr = start as u32 + offset as u32;
                let this_upper = (addr >> 16) as u16;
                if this_upper != upper {
                    upper = this_upper;
                    emit_record(&mut out, 0, 0x04, &(upper.to_be_bytes()));
                }
                let n = record_len.min(data.len() - offset);
                let chunk = &data[offset..offset + n];
                emit_record(&mut out, addr as u16, 0x00, chunk);
                offset += n;
            }
        }
        emit_record(&mut out, 0, 0x01, &[]);
        out
    }

    /// Serialise as raw binary, padding gaps with `fill`.
    ///
    /// Returns `None` for an empty image because there is no base address to
    /// start from.
    pub fn to_binary(&self, fill: u8) -> Option<Vec<u8>> {
        let low = self.min_addr()?;
        let high = self.max_addr()?;
        let mut out = vec![fill; high as usize - low as usize + 1];
        for (addr, byte) in self.iter() {
            out[addr as usize - low as usize] = byte;
        }
        Some(out)
    }

    /// Parse an Intel HEX file.
    pub fn from_intel_hex(text: &str) -> Result<Self, ImageError> {
        let mut image = Self::new();
        let mut upper = 0u32;
        let mut saw_data = false;
        let mut saw_eof = false;

        for (i, raw) in text.lines().enumerate() {
            let line = i + 1;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some(body) = trimmed.strip_prefix(':') else {
                return Err(ImageError::MissingStartCode { line });
            };
            let bytes = parse_hex(body).ok_or_else(|| ImageError::BadHex {
                line,
                text: trimmed.to_string(),
            })?;
            if bytes.len() < 5 {
                return Err(ImageError::BadHex {
                    line,
                    text: trimmed.to_string(),
                });
            }
            let count = bytes[0] as usize;
            if bytes.len() != count + 5 {
                return Err(ImageError::LengthMismatch {
                    line,
                    expected: count,
                    actual: bytes.len().saturating_sub(5),
                });
            }
            let checksum = bytes[bytes.len() - 1];
            let computed = checksum_of(&bytes[..bytes.len() - 1]);
            if checksum != computed {
                return Err(ImageError::BadChecksum {
                    line,
                    found: checksum,
                    computed,
                });
            }

            let addr = ((bytes[1] as u32) << 8) | bytes[2] as u32;
            let kind = bytes[3];
            let data = &bytes[4..4 + count];
            match kind {
                0x00 => {
                    saw_data = true;
                    for (j, byte) in data.iter().enumerate() {
                        let a = upper + addr + j as u32;
                        if a > 0xFFFF {
                            // Silently wrap: the 8051 code space is 64 KiB.
                            let wrapped = a as u16;
                            image.bytes.insert(wrapped, *byte);
                        } else {
                            image.bytes.insert(a as u16, *byte);
                        }
                    }
                }
                0x01 => {
                    saw_eof = true;
                    break;
                }
                0x02 => {
                    if count >= 2 {
                        upper = (((data[0] as u32) << 8) | data[1] as u32) << 4;
                    }
                }
                0x04 => {
                    if count >= 2 {
                        upper = (((data[0] as u32) << 8) | data[1] as u32) << 16;
                    }
                }
                0x03 | 0x05 => { /* start address records: nothing to place */ }
                other => {
                    return Err(ImageError::UnsupportedRecord { line, kind: other });
                }
            }
        }

        if !saw_data && !saw_eof {
            return Err(ImageError::Empty);
        }
        Ok(image)
    }

    /// Parse a raw binary file placed at `base`.
    pub fn from_binary(base: u16, data: &[u8]) -> Self {
        Self::from_bytes(base, data)
    }
}

/// A summary of an image's footprint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SizeReport {
    /// Number of contiguous runs.
    pub runs: usize,
    /// Number of bytes present.
    pub bytes: usize,
    /// Lowest address.
    pub low: Option<u16>,
    /// Highest address.
    pub high: Option<u16>,
    /// `high - low + 1`.
    pub span: Option<u32>,
}

impl std::fmt::Display for SizeReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.low, self.high) {
            (Some(low), Some(high)) => write!(
                f,
                "{} bytes in {} region(s), {low:#06X}..={high:#06X} (span {})",
                self.bytes,
                self.runs,
                self.span.unwrap_or(0)
            ),
            _ => write!(f, "empty image"),
        }
    }
}

fn checksum_of(bytes: &[u8]) -> u8 {
    (!bytes.iter().fold(0u8, |acc, b| acc.wrapping_add(*b))).wrapping_add(1)
}

fn emit_record(out: &mut String, addr: u16, kind: u8, data: &[u8]) {
    let mut body = Vec::with_capacity(data.len() + 4);
    body.push(data.len() as u8);
    body.push((addr >> 8) as u8);
    body.push(addr as u8);
    body.push(kind);
    body.extend_from_slice(data);
    let checksum = checksum_of(&body);
    out.push(':');
    for byte in body.iter().chain(std::iter::once(&checksum)) {
        let _ = write!(out, "{byte:02X}");
    }
    out.push('\n');
}

fn parse_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_intel_hex() {
        let mut image = Image::new();
        image.write(0x0000, &[0x02, 0x00, 0x30]);
        image.write(0x0030, &[0x74, 0x55, 0x22]);
        let text = image.to_intel_hex();
        assert_eq!(Image::from_intel_hex(&text).unwrap(), image);
        assert!(text.ends_with(":00000001FF\n"));
    }

    #[test]
    fn gaps_become_separate_runs() {
        let mut image = Image::new();
        image.write(0x0000, &[1, 2, 3]);
        image.write(0x0100, &[4, 5]);
        assert_eq!(image.runs().len(), 2);
        assert_eq!(image.size_report().bytes, 5);
        assert_eq!(image.size_report().span, Some(0x102));
    }

    #[test]
    fn binary_output_pads_gaps() {
        let mut image = Image::new();
        image.write(0x0000, &[0xAA]);
        image.write(0x0004, &[0xBB]);
        assert_eq!(
            image.to_binary(0xFF).unwrap(),
            vec![0xAA, 0xFF, 0xFF, 0xFF, 0xBB]
        );
    }

    #[test]
    fn rejects_a_corrupt_checksum() {
        // Valid record is ":03000000020030CB"; flip the checksum.
        assert!(Image::from_intel_hex(":03000000020030CB\n:00000001FF\n").is_ok());
        let err = Image::from_intel_hex(":03000000020030CC\n:00000001FF\n").unwrap_err();
        assert!(matches!(err, ImageError::BadChecksum { .. }), "{err}");
    }

    #[test]
    fn rejects_a_truncated_record() {
        let err = Image::from_intel_hex(":03000000020031\n").unwrap_err();
        assert!(matches!(err, ImageError::LengthMismatch { .. }), "{err}");
    }

    #[test]
    fn rejects_conflicting_duplicates() {
        let mut image = Image::new();
        image.write(0x10, &[0xAA]);
        let err = image.write_checked(0x10, &[0xBB]).unwrap_err();
        assert!(matches!(err, ImageError::Conflict { .. }));
        image.write_checked(0x10, &[0xAA]).unwrap();
    }
}
