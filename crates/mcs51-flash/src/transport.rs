//! Byte transports: a real serial port, an in-memory link, and a link into the
//! `mcs51-sim` simulator.
//!
//! The protocol implementations only ever see this trait, which is what makes
//! the whole flasher testable without hardware: the same code that talks to a
//! CH340 on `/dev/ttyUSB0` also talks to a simulated 8051 running a bootloader.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;
use std::rc::Rc;
use std::time::{Duration, Instant};

use mcs51_sim::Machine;

/// Serial parity, as the STC BSLs use it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parity {
    /// No parity (STC 89/90 series).
    None,
    /// Even parity (STC 10/11/12/15/8 series use it for error correction).
    Even,
}

impl fmt::Display for Parity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Parity::None => "none",
            Parity::Even => "even",
        })
    }
}

/// Why a transport operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The peer did not answer within the timeout.
    Timeout {
        /// How many bytes were still missing.
        wanted: usize,
        /// Bytes actually received before giving up.
        got: usize,
    },
    /// The operating system refused the operation.
    Io(String),
    /// The transport cannot do what was asked (for example, change baud rate).
    Unsupported(&'static str),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::Timeout { wanted, got } => {
                write!(f, "timed out: wanted {wanted} byte(s), got {got}")
            }
            TransportError::Io(message) => f.write_str(message),
            TransportError::Unsupported(what) => write!(f, "this transport cannot {what}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// A bidirectional byte link to the target.
pub trait Transport {
    /// Send bytes.
    fn write_all(&mut self, data: &[u8]) -> Result<(), TransportError>;

    /// Read exactly `buf.len()` bytes, giving up after `timeout`.
    fn read_exact(&mut self, buf: &mut [u8], timeout: Duration) -> Result<(), TransportError>;

    /// Read one byte, if one arrives within `timeout`.
    fn read_byte(&mut self, timeout: Duration) -> Result<Option<u8>, TransportError> {
        let mut one = [0u8; 1];
        match self.read_exact(&mut one, timeout) {
            Ok(()) => Ok(Some(one[0])),
            Err(TransportError::Timeout { .. }) => Ok(None),
            Err(other) => Err(other),
        }
    }

    /// How many bytes are waiting to be read.
    fn bytes_available(&mut self) -> Result<usize, TransportError>;

    /// Discard anything already buffered.
    fn drain(&mut self) -> Result<(), TransportError> {
        let mut scratch = [0u8; 256];
        while self.bytes_available()? > 0 {
            let take = self.bytes_available()?.min(scratch.len());
            if self
                .read_exact(&mut scratch[..take], Duration::from_millis(50))
                .is_err()
            {
                break;
            }
        }
        Ok(())
    }

    /// Change the line speed.
    fn set_baud(&mut self, baud: u32) -> Result<(), TransportError>;

    /// The current line speed.
    fn baud(&self) -> u32;

    /// Change the parity setting.
    fn set_parity(&mut self, _parity: Parity) -> Result<(), TransportError> {
        Ok(())
    }

    /// Drive the DTR handshake line, used to reset some boards automatically.
    fn set_dtr(&mut self, _level: bool) -> Result<(), TransportError> {
        Ok(())
    }

    /// Drive the RTS handshake line.
    fn set_rts(&mut self, _level: bool) -> Result<(), TransportError> {
        Ok(())
    }

    /// A human-readable name, for progress messages.
    fn describe(&self) -> String;
}

// ---------------------------------------------------------------------------
// Real serial port
// ---------------------------------------------------------------------------

#[cfg(feature = "serial")]
mod serial_backend {
    use super::*;

    /// A real serial port, normally a USB-to-TTL adapter wired to the MCU's
    /// RXD/TXD.
    pub struct SerialTransport {
        port: Box<dyn serialport::SerialPort>,
        baud: u32,
        name: String,
    }

    impl SerialTransport {
        /// Open a port.
        pub fn open(name: &str, baud: u32) -> Result<Self, TransportError> {
            let port = serialport::new(name, baud)
                .timeout(Duration::from_millis(200))
                .parity(serialport::Parity::None)
                .data_bits(serialport::DataBits::Eight)
                .stop_bits(serialport::StopBits::One)
                .flow_control(serialport::FlowControl::None)
                .open()
                .map_err(|e| TransportError::Io(format!("cannot open {name}: {e}")))?;
            Ok(Self {
                port,
                baud,
                name: name.to_string(),
            })
        }

        /// Every serial port the operating system knows about.
        pub fn available_ports() -> Vec<String> {
            serialport::available_ports()
                .map(|ports| ports.into_iter().map(|p| p.port_name).collect())
                .unwrap_or_default()
        }
    }

    impl Transport for SerialTransport {
        fn write_all(&mut self, data: &[u8]) -> Result<(), TransportError> {
            use std::io::Write;
            self.port
                .write_all(data)
                .map_err(|e| TransportError::Io(format!("write failed: {e}")))?;
            self.port
                .flush()
                .map_err(|e| TransportError::Io(format!("flush failed: {e}")))
        }

        fn read_exact(&mut self, buf: &mut [u8], timeout: Duration) -> Result<(), TransportError> {
            use std::io::Read;
            let deadline = Instant::now() + timeout;
            let mut got = 0usize;
            while got < buf.len() {
                if Instant::now() >= deadline {
                    return Err(TransportError::Timeout {
                        wanted: buf.len(),
                        got,
                    });
                }
                match self.port.read(&mut buf[got..]) {
                    Ok(0) => {}
                    Ok(n) => got += n,
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(e) => return Err(TransportError::Io(format!("read failed: {e}"))),
                }
            }
            Ok(())
        }

        fn bytes_available(&mut self) -> Result<usize, TransportError> {
            self.port
                .bytes_to_read()
                .map(|n| n as usize)
                .map_err(|e| TransportError::Io(format!("bytes_to_read failed: {e}")))
        }

        fn set_baud(&mut self, baud: u32) -> Result<(), TransportError> {
            self.port
                .set_baud_rate(baud)
                .map_err(|e| TransportError::Io(format!("cannot set {baud} baud: {e}")))?;
            self.baud = baud;
            Ok(())
        }

        fn baud(&self) -> u32 {
            self.baud
        }

        fn set_parity(&mut self, parity: Parity) -> Result<(), TransportError> {
            let setting = match parity {
                Parity::None => serialport::Parity::None,
                Parity::Even => serialport::Parity::Even,
            };
            self.port
                .set_parity(setting)
                .map_err(|e| TransportError::Io(format!("cannot set parity: {e}")))
        }

        fn set_dtr(&mut self, level: bool) -> Result<(), TransportError> {
            self.port
                .write_data_terminal_ready(level)
                .map_err(|e| TransportError::Io(format!("DTR failed: {e}")))
        }

        fn set_rts(&mut self, level: bool) -> Result<(), TransportError> {
            self.port
                .write_request_to_send(level)
                .map_err(|e| TransportError::Io(format!("RTS failed: {e}")))
        }

        fn describe(&self) -> String {
            format!("{} @ {} baud", self.name, self.baud)
        }
    }
}

#[cfg(feature = "serial")]
pub use serial_backend::SerialTransport;

// ---------------------------------------------------------------------------
// In-memory link
// ---------------------------------------------------------------------------

/// A device model that the [`LoopbackTransport`] feeds.
///
/// This is the target side of the protocol: a test implements it to answer the
/// flasher the way real silicon would.
pub trait Device {
    /// Accept one byte from the host.
    fn host_to_device(&mut self, byte: u8);
    /// Produce one byte for the host, if any.
    fn device_to_host(&mut self) -> Option<u8>;
    /// The host changed the line speed.
    fn set_baud(&mut self, _baud: u32) {}
    /// The host changed the parity setting.
    fn set_parity(&mut self, _parity: Parity) {}
}

/// A transport wired straight to a [`Device`], with no I/O in between.
///
/// The device is shared, so a test can keep a handle and inspect what actually
/// landed in the model's flash after the download consumed the transport.
pub struct LoopbackTransport<D: Device> {
    device: Rc<RefCell<D>>,
    to_host: VecDeque<u8>,
    baud: u32,
}

impl<D: Device> LoopbackTransport<D> {
    /// Wrap a device.
    pub fn new(device: D, baud: u32) -> Self {
        Self::shared(Rc::new(RefCell::new(device)), baud)
    }

    /// Wrap a device that the caller keeps a handle on.
    pub fn shared(device: Rc<RefCell<D>>, baud: u32) -> Self {
        Self {
            device,
            to_host: VecDeque::new(),
            baud,
        }
    }

    /// A handle to the device under test.
    pub fn handle(&self) -> Rc<RefCell<D>> {
        Rc::clone(&self.device)
    }

    /// The device under test.
    pub fn device(&self) -> std::cell::Ref<'_, D> {
        self.device.borrow()
    }

    /// The device under test, mutably.
    pub fn device_mut(&self) -> std::cell::RefMut<'_, D> {
        self.device.borrow_mut()
    }

    fn pump(&mut self) {
        let mut device = self.device.borrow_mut();
        while let Some(byte) = device.device_to_host() {
            self.to_host.push_back(byte);
        }
    }
}

impl<D: Device> Transport for LoopbackTransport<D> {
    fn write_all(&mut self, data: &[u8]) -> Result<(), TransportError> {
        {
            let mut device = self.device.borrow_mut();
            for byte in data {
                device.host_to_device(*byte);
            }
        }
        self.pump();
        Ok(())
    }

    fn read_exact(&mut self, buf: &mut [u8], _timeout: Duration) -> Result<(), TransportError> {
        self.pump();
        if self.to_host.len() < buf.len() {
            return Err(TransportError::Timeout {
                wanted: buf.len(),
                got: self.to_host.len(),
            });
        }
        for slot in buf.iter_mut() {
            *slot = self.to_host.pop_front().expect("length checked above");
        }
        Ok(())
    }

    fn bytes_available(&mut self) -> Result<usize, TransportError> {
        self.pump();
        Ok(self.to_host.len())
    }

    fn set_baud(&mut self, baud: u32) -> Result<(), TransportError> {
        self.baud = baud;
        self.device.borrow_mut().set_baud(baud);
        Ok(())
    }

    fn baud(&self) -> u32 {
        self.baud
    }

    fn set_parity(&mut self, parity: Parity) -> Result<(), TransportError> {
        self.device.borrow_mut().set_parity(parity);
        Ok(())
    }

    fn describe(&self) -> String {
        format!("loopback @ {} baud", self.baud)
    }
}

// ---------------------------------------------------------------------------
// Link into the simulator
// ---------------------------------------------------------------------------

/// A transport that talks to a real 8051 bootloader running inside
/// [`mcs51_sim::Machine`].
///
/// Bytes written by the host are pushed into the simulated UART's receive
/// queue; bytes the firmware transmits come back through the simulated UART's
/// console. This is what lets the self-hosted ISP be verified end to end
/// without a chip on the desk.
pub struct SimTransport {
    machine: Machine,
    consumed: usize,
    baud: u32,
    /// Machine cycles to run after a write before the firmware is expected to
    /// have processed it.
    settle_cycles: u64,
    /// Machine cycles to wait for a response before giving up.
    timeout_cycles: u64,
}

impl SimTransport {
    /// Wrap a machine that is already running a bootloader.
    pub fn new(machine: Machine) -> Self {
        Self {
            machine,
            consumed: 0,
            baud: 9600,
            settle_cycles: 40_000,
            timeout_cycles: 4_000_000,
        }
    }

    /// The simulator, for assertions after a download.
    pub fn machine(&self) -> &Machine {
        &self.machine
    }

    /// The simulator, mutably.
    pub fn machine_mut(&mut self) -> &mut Machine {
        &mut self.machine
    }

    /// Take the firmware back out of the simulator.
    pub fn into_machine(self) -> Machine {
        self.machine
    }

    /// Everything the firmware has transmitted so far.
    pub fn console(&self) -> &[u8] {
        self.machine.console()
    }

    fn wait_for(&mut self, count: usize) -> Result<(), TransportError> {
        let deadline = self.machine.cycles + self.timeout_cycles;
        while self.machine.console().len() < count {
            if self.machine.cycles > deadline {
                return Err(TransportError::Timeout {
                    wanted: count,
                    got: self.machine.console().len(),
                });
            }
            self.machine.step();
        }
        Ok(())
    }
}

impl Transport for SimTransport {
    fn write_all(&mut self, data: &[u8]) -> Result<(), TransportError> {
        self.machine.feed_uart(data);
        // Let the firmware drain its receive interrupt before the host starts
        // waiting for an answer.
        let target = self.machine.cycles + self.settle_cycles;
        while self.machine.cycles < target {
            self.machine.step();
        }
        Ok(())
    }

    fn read_exact(&mut self, buf: &mut [u8], _timeout: Duration) -> Result<(), TransportError> {
        self.wait_for(self.consumed + buf.len())?;
        let console = self.machine.console();
        buf.copy_from_slice(&console[self.consumed..self.consumed + buf.len()]);
        self.consumed += buf.len();
        Ok(())
    }

    fn bytes_available(&mut self) -> Result<usize, TransportError> {
        Ok(self.machine.console().len() - self.consumed)
    }

    fn set_baud(&mut self, baud: u32) -> Result<(), TransportError> {
        self.baud = baud;
        // Keep the simulated transmitter's timing consistent with the line
        // speed the firmware has been told to use.
        let cycles = (self.machine.peripherals().uart.tx_cycles.max(1)) as u64;
        let _ = cycles;
        self.settle_cycles = 40_000;
        Ok(())
    }

    fn baud(&self) -> u32 {
        self.baud
    }

    fn describe(&self) -> String {
        "simulated 8051".to_string()
    }
}
