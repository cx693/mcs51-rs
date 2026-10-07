//! # mcs51-flash
//!
//! Download firmware to real 8051 hardware.
//!
//! STC microcontrollers ship a UART bootstrap loader (their "ISP" mode). The
//! vendor only documents it through a Windows GUI, so this crate is a Rust port
//! of the packet format and command sequences that the open-source
//! [`stcflash`](https://github.com/cx693/StcFlash) / [`stcgal`](https://github.com/grigorig/stcgal)
//! tools reverse engineered from it.
//!
//! ## What it does
//!
//! ```no_run
//! use mcs51_flash::{flash_stc, FlashRequest, StcFamily, StcOptions};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let request = FlashRequest {
//!     port: "/dev/ttyUSB0".into(),
//!     baud: 115_200,
//!     options: StcOptions {
//!         family: StcFamily::Stc89,
//!         ..Default::default()
//!     },
//!     image: std::fs::read_to_string("build/blink.ihx")?,
//! };
//! let report = flash_stc(&request)?;
//! println!("{} programmed, {} bytes", report.model, report.bytes);
//! # Ok(())
//! # }
//! ```
//!
//! ## Why it is testable
//!
//! Every protocol implementation talks to a [`transport::Transport`], never to a
//! serial port directly. That trait has three implementations: a real port
//! behind the `serial` feature, an in-memory [`transport::LoopbackTransport`],
//! and a [`transport::SimTransport`] that drives a real 8051 firmware image
//! inside [`mcs51_sim`]. The tests therefore exercise the exact code path that
//! talks to silicon.
//!
//! ## What is and is not verified
//!
//! The framing, checksums, baud-rate arithmetic and command sequences are
//! checked against captured layouts from the reference tools and against a
//! simulated BSL that speaks the same protocol. They have **not** been run
//! against a physical STC part from this repository — treat the first download
//! to a new board as a smoke test, and keep `--verify` on.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod frame;
pub mod mock;
pub mod stc;
pub mod transport;

pub use frame::{Checksum, FrameError};
pub use stc::{ResetMode, Stc89Options, StcFamily, StcFlasher, StcInfo, StcOptions, model_name};
pub use transport::{Device, LoopbackTransport, Parity, SimTransport, Transport, TransportError};

#[cfg(feature = "serial")]
pub use transport::SerialTransport;

use std::fmt;
use std::time::Duration;

use mcs51_ihex::Image;

/// Why a download failed.
#[derive(Debug)]
pub enum FlashError {
    /// The link failed.
    Transport(TransportError),
    /// The chip answered, but not with what the protocol expects.
    Protocol(String),
    /// The firmware image could not be read.
    Image(String),
    /// The image does not fit the memory map the request describes.
    ImageTooLarge {
        /// Bytes the image occupies.
        size: usize,
        /// Bytes available.
        capacity: usize,
    },
    /// The chip reported a different memory than what was written back.
    VerifyFailed {
        /// Address of the first difference.
        address: u16,
        /// Byte that was written.
        expected: u8,
        /// Byte read back.
        actual: u8,
    },
}

impl fmt::Display for FlashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlashError::Transport(error) => write!(f, "{error}"),
            FlashError::Protocol(message) => write!(f, "{message}"),
            FlashError::Image(message) => write!(f, "{message}"),
            FlashError::ImageTooLarge { size, capacity } => write!(
                f,
                "the image is {size} bytes but the target only has {capacity}"
            ),
            FlashError::VerifyFailed {
                address,
                expected,
                actual,
            } => write!(
                f,
                "verify failed at {address:#06X}: wrote {expected:#04X}, read back {actual:#04X}"
            ),
        }
    }
}

impl std::error::Error for FlashError {}

impl From<TransportError> for FlashError {
    fn from(error: TransportError) -> Self {
        FlashError::Transport(error)
    }
}

/// The phase a download is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Resetting the chip into its bootloader.
    Reset,
    /// Finding the chip's synchronisation byte and reading its status.
    Handshake,
    /// Moving both ends to the transfer baud rate.
    BaudSwitch,
    /// Erasing flash.
    Erase,
    /// Writing the image.
    Program,
    /// Reading the image back.
    Verify,
    /// Writing the option (fuse) byte.
    Options,
    /// Running the program.
    Run,
}

impl Stage {
    /// A short label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Stage::Reset => "reset",
            Stage::Handshake => "handshake",
            Stage::BaudSwitch => "baud",
            Stage::Erase => "erase",
            Stage::Program => "program",
            Stage::Verify => "verify",
            Stage::Options => "options",
            Stage::Run => "run",
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A progress report.
#[derive(Debug, Clone, PartialEq)]
pub enum ProgressEvent {
    /// A new phase started.
    Stage(Stage),
    /// A phase knows how much work it has.
    Begin {
        /// The phase.
        stage: Stage,
        /// Total units of work.
        total: usize,
    },
    /// Work was done.
    Advance {
        /// The phase.
        stage: Stage,
        /// Units done so far.
        done: usize,
        /// Total units.
        total: usize,
    },
    /// A line of human-readable information.
    Message(String),
}

/// Where download progress goes.
pub struct Progress {
    quiet: bool,
    callback: Option<Box<dyn FnMut(&ProgressEvent)>>,
    events: Vec<ProgressEvent>,
}

impl fmt::Debug for Progress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Progress")
            .field("quiet", &self.quiet)
            .field("events", &self.events.len())
            .finish()
    }
}

impl Default for Progress {
    fn default() -> Self {
        Self::new()
    }
}

impl Progress {
    /// A progress reporter that prints to stderr.
    pub fn new() -> Self {
        Self {
            quiet: false,
            callback: None,
            events: Vec::new(),
        }
    }

    /// A progress reporter that records events and prints nothing.
    pub fn silent() -> Self {
        Self {
            quiet: true,
            callback: None,
            events: Vec::new(),
        }
    }

    /// Suppress the built-in output.
    pub fn quiet(mut self) -> Self {
        self.quiet = true;
        self
    }

    /// Install a callback, called for every event.
    pub fn with_callback(mut self, callback: impl FnMut(&ProgressEvent) + 'static) -> Self {
        self.callback = Some(Box::new(callback));
        self
    }

    /// Every event so far.
    pub fn events(&self) -> &[ProgressEvent] {
        &self.events
    }

    fn emit(&mut self, event: ProgressEvent) {
        if let Some(callback) = self.callback.as_mut() {
            callback(&event);
        }
        self.events.push(event);
    }

    /// Report a new phase.
    pub fn stage(&mut self, stage: Stage) {
        if !self.quiet {
            eprintln!("[{stage}]");
        }
        self.emit(ProgressEvent::Stage(stage));
    }

    /// Report a phase's total size.
    pub fn begin(&mut self, stage: Stage, total: usize) {
        self.emit(ProgressEvent::Begin { stage, total });
    }

    /// Report progress inside a phase.
    pub fn advance(&mut self, stage: Stage, done: usize) {
        let _ = stage;
        self.emit(ProgressEvent::Advance {
            stage,
            done,
            total: 0,
        });
    }

    /// Report a line of information.
    pub fn message(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !self.quiet {
            eprintln!("  {text}");
        }
        self.emit(ProgressEvent::Message(text));
    }

    /// Report what the chip said about itself.
    pub fn info(&mut self, info: &StcInfo) {
        self.message(format!("model {}", info.model));
        self.message(format!("target frequency {:.3} MHz", info.clock_mhz()));
        self.message(format!("BSL version {}", info.bsl_version));
        self.message(format!("options {}", Stc89Options::new(info.option_byte)));
    }
}

/// Everything needed for one download.
#[derive(Debug, Clone)]
pub struct FlashRequest {
    /// Serial port name, for example `/dev/ttyUSB0` or `COM3`.
    pub port: String,
    /// Transfer baud rate. The handshake always starts at
    /// [`StcOptions::handshake_baud`].
    pub baud: u32,
    /// Protocol tuning.
    pub options: StcOptions,
    /// Intel HEX text.
    pub image: String,
}

/// What a finished download did.
#[derive(Debug, Clone, PartialEq)]
pub struct FlashReport {
    /// The model the chip reported.
    pub model: String,
    /// The BSL version the chip reported.
    pub bsl_version: String,
    /// The measured system clock.
    pub clock_hz: f64,
    /// Bytes written.
    pub bytes: usize,
    /// Lowest programmed address.
    pub low: u16,
    /// Highest programmed address.
    pub high: u16,
    /// Time the whole download took.
    pub elapsed: Duration,
}

/// Parse an Intel HEX image and flatten it into a byte vector starting at zero.
///
/// The STC BSL writes from address 0 with no gaps, so an image with holes is
/// padded with `0xFF` the way an erased part reads.
pub fn flatten_image(text: &str, capacity: usize) -> Result<Vec<u8>, FlashError> {
    let image = Image::from_intel_hex(text).map_err(|e| FlashError::Image(e.to_string()))?;
    let high = image.max_addr().map(|a| a as usize + 1).unwrap_or(0);
    if high > capacity {
        return Err(FlashError::ImageTooLarge {
            size: high,
            capacity,
        });
    }
    let mut data = vec![0xFFu8; high];
    for (address, byte) in image.iter() {
        data[address as usize] = byte;
    }
    Ok(data)
}

/// Download an image over a transport, using the STC bootloader protocol.
pub fn flash_over_transport<T: Transport>(
    transport: T,
    options: StcOptions,
    image: &str,
    capacity: usize,
    progress: &mut Progress,
) -> Result<FlashReport, FlashError> {
    let started = std::time::Instant::now();
    let data = flatten_image(image, capacity)?;
    let low = 0u16;
    let high = if data.is_empty() {
        0
    } else {
        (data.len() - 1) as u16
    };

    let mut flasher = StcFlasher::new(transport, options);
    let info = flasher.connect(progress)?;
    flasher.handshake(progress)?;
    flasher.erase(data.len().max(1), progress)?;
    flasher.program(&data, progress)?;
    flasher.program_options(progress)?;
    progress.stage(Stage::Run);
    flasher.disconnect()?;

    Ok(FlashReport {
        model: info.model,
        bsl_version: info.bsl_version,
        clock_hz: info.clock_hz,
        bytes: data.len(),
        low,
        high,
        elapsed: started.elapsed(),
    })
}

/// Open a real serial port and download an image.
#[cfg(feature = "serial")]
pub fn flash_stc(request: &FlashRequest) -> Result<FlashReport, FlashError> {
    let mut options = request.options.clone();
    options.transfer_baud = request.baud;
    let transport = SerialTransport::open(&request.port, options.handshake_baud)?;
    let mut progress = Progress::new();
    flash_over_transport(transport, options, &request.image, 64 * 1024, &mut progress)
}

/// Connect to a chip and print what it reports, without writing anything.
#[cfg(feature = "serial")]
pub fn probe_stc(
    port: &str,
    options: StcOptions,
    progress: &mut Progress,
) -> Result<StcInfo, FlashError> {
    let transport = SerialTransport::open(port, options.handshake_baud)?;
    let mut flasher = StcFlasher::new(transport, options);
    flasher.connect(progress)
}

/// Every serial port the operating system lists.
#[cfg(feature = "serial")]
pub fn serial_ports() -> Vec<String> {
    SerialTransport::available_ports()
}

/// Serial ports, or an empty list when built without the `serial` feature.
#[cfg(not(feature = "serial"))]
pub fn serial_ports() -> Vec<String> {
    Vec::new()
}
