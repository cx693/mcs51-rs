//! The STC BSL packet format.
//!
//! STC's in-system-programming protocol is not documented by the vendor; this
//! is a Rust port of the format used by the open-source `stcflash`/`stcgal`
//! tools, which was reverse engineered from the Windows ISP utility.
//!
//! ```text
//! host -> mcu:  46 b9 6a <len:be16> <payload...> <checksum> 16
//! mcu  -> host: 46 b9 68 <len:be16> <payload...> <checksum> 16
//! ```
//!
//! `len` counts from the direction byte to the `0x16` terminator, so it is
//! `payload.len() + 5` for the one-byte-checksum family (STC 89/90) and
//! `payload.len() + 6` for the two-byte family (STC 10/11/12/15/8).

use std::time::Duration;

use crate::transport::{Transport, TransportError};

/// First magic byte of a packet.
pub const PACKET_START: [u8; 2] = [0x46, 0xB9];
/// Terminator byte of a packet.
pub const PACKET_END: u8 = 0x16;
/// Direction magic for packets sent by the MCU.
pub const PACKET_MCU: u8 = 0x68;
/// Direction magic for packets sent by the host.
pub const PACKET_HOST: u8 = 0x6A;

/// How the checksum is sized, which is what separates the two STC families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checksum {
    /// One byte, `sum & 0xFF` (STC 89/90).
    Byte8,
    /// Two bytes, big-endian `sum & 0xFFFF` (STC 10/11/12/15/8).
    Word16,
}

impl Checksum {
    /// Size in bytes on the wire.
    pub const fn len(self) -> usize {
        match self {
            Checksum::Byte8 => 1,
            Checksum::Word16 => 2,
        }
    }

    /// The length field for a payload of `n` bytes.
    pub const fn wire_len(self, n: usize) -> usize {
        n + 5 + (self.len() - 1)
    }

    fn encode(self, sum: u32, out: &mut Vec<u8>) {
        match self {
            Checksum::Byte8 => out.push((sum & 0xFF) as u8),
            Checksum::Word16 => out.extend_from_slice(&((sum & 0xFFFF) as u16).to_be_bytes()),
        }
    }
}

/// Why a packet could not be framed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// A byte where a framing marker was expected.
    BadMagic {
        /// What was expected.
        expected: u8,
        /// What arrived.
        got: u8,
    },
    /// The packet did not end with `0x16`.
    BadTerminator(u8),
    /// The checksum in the packet did not match its contents.
    BadChecksum {
        /// Checksum from the packet.
        found: u32,
        /// Checksum computed locally.
        computed: u32,
    },
    /// The packet was shorter than its own header claims.
    Truncated {
        /// Bytes expected in total.
        expected: usize,
        /// Bytes actually read.
        got: usize,
    },
    /// The link itself failed.
    Transport(TransportError),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::BadMagic { expected, got } => write!(
                f,
                "bad framing magic: expected {expected:#04X}, got {got:#04X}"
            ),
            FrameError::BadTerminator(byte) => {
                write!(f, "packet does not end with 0x16 (got {byte:#04X})")
            }
            FrameError::BadChecksum { found, computed } => write!(
                f,
                "packet checksum {found:#06X} does not match computed {computed:#06X}"
            ),
            FrameError::Truncated { expected, got } => {
                write!(f, "truncated packet: expected {expected} bytes, got {got}")
            }
            FrameError::Transport(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<TransportError> for FrameError {
    fn from(error: TransportError) -> Self {
        FrameError::Transport(error)
    }
}

/// Build a host-to-MCU packet.
pub fn encode_host(payload: &[u8], checksum: Checksum) -> Vec<u8> {
    let mut packet = Vec::with_capacity(payload.len() + 8);
    packet.extend_from_slice(&PACKET_START);
    packet.push(PACKET_HOST);
    packet.extend_from_slice(&(checksum.wire_len(payload.len()) as u16).to_be_bytes());
    packet.extend_from_slice(payload);
    let sum: u32 = packet[2..].iter().map(|b| *b as u32).sum();
    checksum.encode(sum, &mut packet);
    packet.push(PACKET_END);
    packet
}

/// Build an MCU-to-host packet. Used by the simulated BSL and by tests.
pub fn encode_mcu(payload: &[u8], checksum: Checksum) -> Vec<u8> {
    let mut packet = Vec::with_capacity(payload.len() + 9);
    packet.extend_from_slice(&PACKET_START);
    packet.push(PACKET_MCU);
    packet.extend_from_slice(&(checksum.wire_len(payload.len()) as u16).to_be_bytes());
    packet.extend_from_slice(payload);
    let sum: u32 = packet[2..].iter().map(|b| *b as u32).sum();
    checksum.encode(sum, &mut packet);
    packet.push(PACKET_END);
    packet
}

/// Split a host packet into its payload, for a device implementation.
///
/// Returns `None` when the buffer does not (yet) hold a complete packet.
pub fn parse_host(buf: &[u8], checksum: Checksum) -> Option<Result<(Vec<u8>, usize), FrameError>> {
    if buf.len() < 5 {
        return None;
    }
    if buf[0] != PACKET_START[0] || buf[1] != PACKET_START[1] || buf[2] != PACKET_HOST {
        return Some(Err(FrameError::BadMagic {
            expected: PACKET_HOST,
            got: buf[2],
        }));
    }
    let total = u16::from_be_bytes([buf[3], buf[4]]) as usize;
    if buf.len() < total + 2 {
        return None;
    }
    let packet = &buf[..total + 2];
    if packet[packet.len() - 1] != PACKET_END {
        return Some(Err(FrameError::BadTerminator(packet[packet.len() - 1])));
    }
    let ck = checksum.len();
    // `total` counts the direction byte through the terminator, so it is
    // `payload + checksum + 4`; the two start bytes sit in front of it.
    let payload_len = total - 4 - ck;
    let payload = packet[5..5 + payload_len].to_vec();
    let sum: u32 = packet[2..5 + payload_len].iter().map(|b| *b as u32).sum();
    let found: u32 = match checksum {
        Checksum::Byte8 => packet[5 + payload_len] as u32,
        Checksum::Word16 => {
            u16::from_be_bytes([packet[5 + payload_len], packet[5 + payload_len + 1]]) as u32
        }
    };
    let mask = match checksum {
        Checksum::Byte8 => 0xFF,
        Checksum::Word16 => 0xFFFF,
    };
    if sum & mask != found {
        return Some(Err(FrameError::BadChecksum {
            found,
            computed: sum & mask,
        }));
    }
    Some(Ok((payload, total + 2)))
}

/// Read one MCU-to-host packet, returning its payload.
pub fn read_packet<T: Transport>(
    transport: &mut T,
    checksum: Checksum,
    timeout: Duration,
) -> Result<Vec<u8>, FrameError> {
    let mut byte = [0u8; 1];

    // A few BSL revisions omit the frame start on the very first status
    // packet, so accept the direction byte on its own.
    transport.read_exact(&mut byte, timeout)?;
    let rest = if byte[0] == PACKET_MCU {
        Vec::new()
    } else {
        if byte[0] != PACKET_START[0] {
            return Err(FrameError::BadMagic {
                expected: PACKET_START[0],
                got: byte[0],
            });
        }
        transport.read_exact(&mut byte, timeout)?;
        if byte[0] != PACKET_START[1] {
            return Err(FrameError::BadMagic {
                expected: PACKET_START[1],
                got: byte[0],
            });
        }
        transport.read_exact(&mut byte, timeout)?;
        if byte[0] != PACKET_MCU {
            return Err(FrameError::BadMagic {
                expected: PACKET_MCU,
                got: byte[0],
            });
        }
        vec![PACKET_MCU]
    };

    let mut length = [0u8; 2];
    transport.read_exact(&mut length, timeout)?;
    let total = u16::from_be_bytes(length) as usize;
    if total < 4 + checksum.len() {
        return Err(FrameError::Truncated {
            expected: 4 + checksum.len(),
            got: total,
        });
    }

    // The length field counts the direction byte through the terminator, and
    // the five header bytes are already accounted for.
    let mut body = vec![0u8; total - 3];
    transport.read_exact(&mut body, timeout)?;

    if body.last() != Some(&PACKET_END) {
        return Err(FrameError::BadTerminator(*body.last().unwrap_or(&0)));
    }

    let ck = checksum.len();
    if body.len() < ck + 1 {
        return Err(FrameError::Truncated {
            expected: ck + 1,
            got: body.len(),
        });
    }
    let payload_len = body.len() - ck - 1;
    let payload = &body[..payload_len];

    // Checksum covers the direction byte, the length and the payload.
    let mut sum: u32 = PACKET_MCU as u32 + length[0] as u32 + length[1] as u32;
    sum += payload.iter().map(|b| *b as u32).sum::<u32>();
    let found: u32 = match checksum {
        Checksum::Byte8 => body[payload_len] as u32,
        Checksum::Word16 => u16::from_be_bytes([body[payload_len], body[payload_len + 1]]) as u32,
    };
    let mask = match checksum {
        Checksum::Byte8 => 0xFF,
        Checksum::Word16 => 0xFFFF,
    };
    if (sum & mask) != found {
        return Err(FrameError::BadChecksum {
            found,
            computed: sum & mask,
        });
    }

    let _ = rest;
    Ok(payload.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_packet_layout_matches_the_reference_tool() {
        // The STC89 disconnect command: payload [0x82].
        let packet = encode_host(&[0x82], Checksum::Byte8);
        assert_eq!(packet, vec![0x46, 0xB9, 0x6A, 0x00, 0x06, 0x82, 0xF2, 0x16]);

        // The STC89 ping-pong: payload 80 00 00 36 01 <magic>.
        let packet = encode_host(&[0x80, 0x00, 0x00, 0x36, 0x01, 0xF0, 0x01], Checksum::Byte8);
        assert_eq!(packet[0..5], [0x46, 0xB9, 0x6A, 0x00, 0x0C]);
        assert_eq!(packet[5..12], [0x80, 0x00, 0x00, 0x36, 0x01, 0xF0, 0x01]);
        let sum: u32 = packet[2..12].iter().map(|b| *b as u32).sum();
        assert_eq!(packet[12], (sum & 0xFF) as u8);
        assert_eq!(packet[13], 0x16);
    }

    #[test]
    fn sixteen_bit_family_uses_a_two_byte_checksum() {
        let packet = encode_host(&[0x82], Checksum::Word16);
        assert_eq!(packet[0..5], [0x46, 0xB9, 0x6A, 0x00, 0x07]);
        assert_eq!(packet[5], 0x82);
        // The checksum covers the direction byte, the length and the payload.
        let sum: u32 = packet[2..6].iter().map(|b| *b as u32).sum();
        assert_eq!(
            u16::from_be_bytes([packet[6], packet[7]]) as u32,
            sum & 0xFFFF
        );
        assert_eq!(packet[8], 0x16);
    }
}
