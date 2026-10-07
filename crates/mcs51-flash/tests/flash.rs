//! End-to-end tests for the STC download path.
//!
//! Everything here runs against [`MockStc`], a byte-level model of an STC part
//! sitting in its bootloader, so the flasher is exercised on the exact code path
//! that would drive a real CH340 adapter.

use std::cell::RefCell;
use std::rc::Rc;

use mcs51_flash::mock::MockStc;
use mcs51_flash::{
    FlashError, LoopbackTransport, Progress, ProgressEvent, ResetMode, StcFamily, StcOptions,
    flash_over_transport, flatten_image,
};

fn options(family: StcFamily) -> StcOptions {
    StcOptions {
        family,
        transfer_baud: 115_200,
        reset: ResetMode::Manual,
        write_options: true,
        option_override: None,
        ..Default::default()
    }
}

/// A tiny image: `MOV A,#0x42 ; SJMP $`, expressed as Intel HEX.
fn tiny_hex() -> String {
    mcs51_ihex::Image::from_bytes(0, &[0x74, 0x42, 0x80, 0xFE]).to_intel_hex()
}

fn stages(progress: &Progress) -> Vec<&'static str> {
    progress
        .events()
        .iter()
        .filter_map(|event| match event {
            ProgressEvent::Stage(stage) => Some(stage.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn flashes_an_image_into_the_simulated_stc89() {
    let chip = Rc::new(RefCell::new(MockStc::stc89(8192)));
    let transport = LoopbackTransport::shared(Rc::clone(&chip), 2400);
    let mut progress = Progress::silent();

    let report = flash_over_transport(
        transport,
        options(StcFamily::Stc89),
        &tiny_hex(),
        8192,
        &mut progress,
    )
    .expect("download should succeed");

    assert_eq!(report.model, "STC89C52RC");
    assert_eq!(report.bytes, 4);
    assert_eq!(report.high, 3);
    assert!(
        report.clock_hz > 11_000_000.0 && report.clock_hz < 11_100_000.0,
        "clock was {}",
        report.clock_hz
    );
    assert_eq!(&chip.borrow().flash[..4], &[0x74, 0x42, 0x80, 0xFE]);
    assert!(chip.borrow().running, "the chip should be told to run");
    // Everything past the image must still read as erased.
    assert!(chip.borrow().flash[4..].iter().all(|b| *b == 0xFF));
    assert_eq!(
        stages(&progress),
        vec![
            "reset",
            "handshake",
            "baud",
            "erase",
            "program",
            "options",
            "run"
        ]
    );
}

#[test]
fn flashes_the_real_firmware_byte_for_byte() {
    // Build a real firmware with the project's own toolchain, push it through
    // the simulated bootloader, and compare what landed in flash.
    let firmware = build_bare_metal(BLINK);
    let expected: Vec<u8> = firmware.iter().map(|(_, byte)| byte).collect();
    assert!(
        expected.len() > 40,
        "sanity: the image should be a real program, got {} bytes",
        expected.len()
    );

    let chip = Rc::new(RefCell::new(MockStc::stc89(8192)));
    let transport = LoopbackTransport::shared(Rc::clone(&chip), 2400);
    let mut progress = Progress::silent();
    let report = flash_over_transport(
        transport,
        options(StcFamily::Stc89),
        &firmware.to_intel_hex(),
        8192,
        &mut progress,
    )
    .expect("download");

    assert_eq!(report.bytes, expected.len());
    assert_eq!(&chip.borrow().flash[..expected.len()], &expected[..]);
    // Address zero really is the firmware's reset vector.
    assert_eq!(chip.borrow().flash[0], 0x02, "LJMP at address zero");
}

#[test]
fn flashes_through_the_stc12_dialect() {
    let chip = Rc::new(RefCell::new(MockStc::stc12(8192)));
    let transport = LoopbackTransport::shared(Rc::clone(&chip), 2400);
    let mut progress = Progress::silent();
    let report = flash_over_transport(
        transport,
        options(StcFamily::Stc12),
        &tiny_hex(),
        8192,
        &mut progress,
    )
    .expect("download");

    assert_eq!(report.model, "STC12C5A60S2");
    assert_eq!(&chip.borrow().flash[..4], &[0x74, 0x42, 0x80, 0xFE]);
    // That dialect negotiates a two-byte checksum, and the model would have
    // rejected every packet otherwise.
    assert!(!chip.borrow().received.is_empty());
}

#[test]
fn the_option_byte_is_written_back_unchanged_by_default() {
    let chip = Rc::new(RefCell::new(MockStc::stc89(4096)));
    let transport = LoopbackTransport::shared(Rc::clone(&chip), 2400);
    let before = chip.borrow().option_byte;
    let mut progress = Progress::silent();
    flash_over_transport(
        transport,
        options(StcFamily::Stc89),
        &tiny_hex(),
        4096,
        &mut progress,
    )
    .expect("download");
    assert_eq!(chip.borrow().option_byte, before);

    // ... and can be overridden when the board needs different fuses.
    let chip = Rc::new(RefCell::new(MockStc::stc89(4096)));
    let transport = LoopbackTransport::shared(Rc::clone(&chip), 2400);
    let mut opts = options(StcFamily::Stc89);
    opts.option_override = Some(0xFC);
    let mut progress = Progress::silent();
    flash_over_transport(transport, opts, &tiny_hex(), 4096, &mut progress).expect("download");
    assert_eq!(chip.borrow().option_byte, 0xFC);
}

#[test]
fn an_image_too_large_for_the_chip_is_refused_before_touching_it() {
    // The image occupies four bytes; tell the flasher the part only has two.
    let chip = Rc::new(RefCell::new(MockStc::stc89(256)));
    let transport = LoopbackTransport::shared(Rc::clone(&chip), 2400);
    let mut progress = Progress::silent();
    let error = flash_over_transport(
        transport,
        options(StcFamily::Stc89),
        &tiny_hex(),
        2,
        &mut progress,
    )
    .unwrap_err();
    match error {
        FlashError::ImageTooLarge { size, capacity } => {
            assert_eq!(size, 4);
            assert_eq!(capacity, 2);
        }
        other => panic!("expected ImageTooLarge, got {other}"),
    }
    // The chip was never even synchronised with.
    assert!(!chip.borrow().synchronised);
}

#[test]
fn a_corrupt_image_is_reported_not_panicked() {
    let chip = Rc::new(RefCell::new(MockStc::stc89(4096)));
    let transport = LoopbackTransport::shared(Rc::clone(&chip), 2400);
    let mut progress = Progress::silent();
    let error = flash_over_transport(
        transport,
        options(StcFamily::Stc89),
        ":03000000020030CC\n:00000001FF\n",
        4096,
        &mut progress,
    )
    .unwrap_err();
    assert!(matches!(error, FlashError::Image(_)), "{error}");
}

#[test]
fn holes_in_the_image_are_filled_with_erased_bytes() {
    let mut image = mcs51_ihex::Image::new();
    image.write(0x0000, &[0x02, 0x00, 0x10]);
    image.write(0x0010, &[0x22]);
    let flat = flatten_image(&image.to_intel_hex(), 64).unwrap();
    assert_eq!(flat.len(), 0x11);
    assert_eq!(&flat[0..3], &[0x02, 0x00, 0x10]);
    assert!(flat[3..0x10].iter().all(|b| *b == 0xFF));
    assert_eq!(flat[0x10], 0x22);
}

#[test]
fn a_chip_that_never_answers_times_out() {
    /// A device that is not in its bootloader.
    struct Dead;

    impl mcs51_flash::Device for Dead {
        fn host_to_device(&mut self, _byte: u8) {}
        fn device_to_host(&mut self) -> Option<u8> {
            None
        }
    }

    let transport = LoopbackTransport::new(Dead, 2400);
    let mut opts = options(StcFamily::Stc89);
    opts.sync_timeout = std::time::Duration::from_millis(80);
    let mut progress = Progress::silent();
    let error =
        flash_over_transport(transport, opts, &tiny_hex(), 8192, &mut progress).unwrap_err();
    // The message must tell the operator what to check, not just "timed out".
    let text = error.to_string();
    assert!(text.contains("没有应答同步脉冲"), "{text}");
    for hint in ["断电重上电", "TXD/RXD", "ISP"] {
        assert!(text.contains(hint), "错误信息缺少排查提示 `{hint}`：{text}");
    }
    // It should also mention how long it waited.
    assert!(
        text.contains("等了 0 秒") || text.contains("等了"),
        "{text}"
    );
}

#[test]
fn the_transfer_baud_rate_tracks_the_requested_speed() {
    // The model records the baud setting the host sent; assert it is consistent
    // with 115200 baud on an 11.0592 MHz part.
    let chip = Rc::new(RefCell::new(MockStc::stc89(4096)));
    let transport = LoopbackTransport::shared(Rc::clone(&chip), 2400);
    let mut progress = Progress::silent();
    flash_over_transport(
        transport,
        options(StcFamily::Stc89),
        &tiny_hex(),
        4096,
        &mut progress,
    )
    .expect("download");

    let received = chip.borrow().received.clone();
    let handshake = received
        .iter()
        .find(|payload| payload.first() == Some(&0x8F))
        .expect("a baud check packet");
    let brt = u16::from_be_bytes([handshake[1], handshake[2]]);
    let actual = 11_059_200.0 / (32.0 * (65536.0 - brt as f64));
    assert!(
        (actual - 115_200.0).abs() / 115_200.0 < 0.02,
        "the chip would have run at {actual} baud"
    );
    assert_eq!(handshake[3], 0xFF - (brt >> 8) as u8);
    assert_eq!(chip.borrow().baud, 115_200);
}

/// Assemble and link a bare-metal application the way the CLI does.
fn build_bare_metal(source: &str) -> mcs51_ihex::Image {
    let output = mcs51_asm::assemble(source, &mcs51_asm::Options::new("blink.asm"));
    assert!(
        output.is_ok(),
        "the example should assemble:\n{}",
        output.render_diagnostics()
    );
    let linked = mcs51_ld::link(&[output.object], &mcs51_ld::LinkOptions::default());
    assert!(linked.is_ok(), "{}", linked.render_diagnostics());
    linked.code
}

/// A bare-metal application: reset vector, vector table, and a real loop.
const BLINK: &str = r#"
        .area CODE (ABS)
        .org  0x0000
        LJMP  main
        .org  0x0003
        RETI
        .org  0x000B
        RETI
        .org  0x0013
        RETI
        .org  0x001B
        RETI
        .org  0x0023
        RETI

        .org  0x0030
main:
        MOV   SP, #0x30
loop:
        CPL   P1_0
        MOV   R7, #200
inner:  DJNZ  R7, inner
        SJMP  loop
        .end
"#;
