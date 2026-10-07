//! End-to-end tests for the `mcs51` driver, exercised as a real subprocess.

use std::path::{Path, PathBuf};
use std::process::Command;

fn mcs51() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mcs51"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str]) -> (String, String, bool) {
    let output = mcs51().args(args).output().expect("run mcs51");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

#[test]
fn help_is_printed_without_arguments() {
    let (stdout, _, ok) = run(&[]);
    assert!(ok);
    assert!(stdout.contains("USAGE:"));
    assert!(stdout.contains("assemble, link and package"));
}

#[test]
fn unknown_commands_fail_loudly() {
    let (_, stderr, ok) = run(&["frobnicate"]);
    assert!(!ok);
    assert!(stderr.contains("unknown command"));
}

#[test]
fn scaffolds_builds_and_runs_a_project() {
    let dir = tmp("scaffold");
    let path = dir.display().to_string();

    let (stdout, stderr, ok) = run(&["new", &path]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("created project"));
    assert!(dir.join("mcs51.toml").exists());
    assert!(dir.join("src/main.asm").exists());

    // Scaffolding twice is an error rather than a silent overwrite.
    let (_, stderr, ok) = run(&["new", &path]);
    assert!(!ok);
    assert!(stderr.contains("already contains a project"), "{stderr}");

    let (stdout, stderr, ok) = run(&["build", &path]);
    assert!(ok, "{stderr}\n{stdout}");
    assert!(stdout.contains("memory"));
    assert!(stdout.contains("CODE"));

    for name in ["blink.ihx", "blink.bin", "blink.map", "blink.lst"] {
        let artifact = dir.join("build").join(name);
        assert!(artifact.exists(), "missing {}", artifact.display());
    }

    let hex = std::fs::read_to_string(dir.join("build/blink.ihx")).unwrap();
    assert!(hex.starts_with(':'));
    assert!(hex.trim_end().ends_with(":00000001FF"));
    let map = std::fs::read_to_string(dir.join("build/blink.map")).unwrap();
    assert!(map.contains("Sections"));
    // 裸机模板里的标签：主循环和延时子程序。
    assert!(map.contains("delay"), "{map}");
    assert!(map.contains("loop"), "{map}");

    let (stdout, stderr, ok) = run(&["run", &path, "--instructions", "900000"]);
    assert!(ok, "{stderr}");
    // 裸机固件是死循环，跑满预算才对；跑到别的地方说明向量表或栈坏了。
    assert!(
        stdout.contains("StepLimit"),
        "the template firmware did not keep running: {stdout}"
    );
    let pc = stdout
        .lines()
        .find_map(|line| {
            let (_, rest) = line.split_once("PC 0x")?;
            u32::from_str_radix(rest.split_whitespace().next()?, 16).ok()
        })
        .expect("a PC in the dump");
    assert!(
        (0x0030..0x0100).contains(&pc),
        "PC 0x{pc:04X} is outside the template's code"
    );
    // 任务表是内核的东西，裸机输出里不该再有。
    assert!(!stdout.contains("tasks"), "{stdout}");
}

#[test]
fn assembles_a_single_file_and_prints_a_listing() {
    let dir = tmp("asm");
    let source = dir.join("tiny.asm");
    std::fs::write(
        &source,
        "\
        .area CODE (ABS)
        .org  0x0100
start:  MOV   A, #0x42
        SJMP  start
",
    )
    .unwrap();
    let (stdout, stderr, ok) = run(&["asm", &source.display().to_string()]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("74 42"), "{stdout}");
    assert!(stdout.contains("80 FC"), "{stdout}");
    assert!(stdout.contains("0100"), "{stdout}");
}

#[test]
fn reports_assembly_errors_with_positions() {
    let dir = tmp("asm-error");
    let source = dir.join("bad.asm");
    std::fs::write(&source, "        FROBNICATE A, B\n").unwrap();
    let (_, stderr, ok) = run(&["asm", &source.display().to_string()]);
    assert!(!ok);
    assert!(stderr.contains("unknown mnemonic"), "{stderr}");
    assert!(stderr.contains(":1:"), "{stderr}");
}

#[test]
fn converts_between_hex_and_binary() {
    let dir = tmp("convert");
    let bin = dir.join("in.bin");
    // A tiny program: MOV A,#0x42 ; SJMP $
    std::fs::write(&bin, [0x74u8, 0x42, 0x80, 0xFE]).unwrap();

    let hex = dir.join("out.ihx");
    let (_, stderr, ok) = run(&[
        "bin2hex",
        &bin.display().to_string(),
        &hex.display().to_string(),
        "--base",
        "0x0100",
    ]);
    assert!(ok, "{stderr}");
    let text = std::fs::read_to_string(&hex).unwrap();
    assert!(text.contains(":04010000744280F"), "{text}");

    let back = dir.join("back.bin");
    let (_, stderr, ok) = run(&[
        "hex2bin",
        &hex.display().to_string(),
        &back.display().to_string(),
    ]);
    assert!(ok, "{stderr}");
    assert_eq!(std::fs::read(&back).unwrap(), vec![0x74, 0x42, 0x80, 0xFE]);

    let (stdout, stderr, ok) = run(&["hexdump", &hex.display().to_string()]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("0100:"), "{stdout}");
}

#[test]
fn disassembles_an_image() {
    let dir = tmp("disasm");
    let bin = dir.join("code.bin");
    // MOV A,#0x42 ; MOV DPTR,#0x1234 ; SJMP $ ; RET
    std::fs::write(&bin, [0x74u8, 0x42, 0x90, 0x12, 0x34, 0x80, 0xFE, 0x22]).unwrap();
    let (stdout, stderr, ok) = run(&["disasm", &bin.display().to_string()]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("MOV A,#0x42"), "{stdout}");
    assert!(stdout.contains("MOV DPTR,#0x1234"), "{stdout}");
    assert!(stdout.contains("RET"), "{stdout}");
}

#[test]
fn describes_the_instruction_set() {
    let (stdout, _, ok) = run(&["isa", "0x74"]);
    assert!(ok);
    assert!(stdout.contains("MOV"), "{stdout}");
    assert!(stdout.contains("2 bytes"), "{stdout}");

    let (stdout, _, ok) = run(&["isa"]);
    assert!(ok);
    assert!(stdout.contains("255 opcodes"), "{stdout}");

    let (_, stderr, ok) = run(&["isa", "0xA5"]);
    assert!(!ok);
    assert!(stderr.contains("not a defined 8051 opcode"), "{stderr}");
}

#[test]
fn size_reports_an_image() {
    let dir = tmp("size");
    let bin = dir.join("code.bin");
    std::fs::write(&bin, [0u8; 32]).unwrap();
    let (stdout, stderr, ok) = run(&["size", &bin.display().to_string()]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("32 bytes"), "{stdout}");
}

#[test]
fn a_project_without_a_manifest_still_builds() {
    // `mcs51 build` in a bare directory falls back to sensible defaults.
    let dir = tmp("bare");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.asm"),
        "\
        .area HOME (ABS)
        .org 0x0000
        LJMP main
        .area CODE
main:   MOV SP, #0x30
        SJMP main
",
    )
    .unwrap();
    let (stdout, stderr, ok) = run(&["build", &dir.display().to_string()]);
    assert!(ok, "{stderr}\n{stdout}");
    assert!(dir.join("build/firmware.ihx").exists());
}

#[test]
fn lists_serial_ports_without_failing() {
    let (stdout, _, ok) = run(&["devices"]);
    assert!(ok);
    assert!(stdout.contains("可用串口"), "{stdout}");
}

#[test]
fn a_flash_dry_run_shows_the_plan_and_touches_nothing() {
    let dir = tmp("flash-dry");
    let path = dir.display().to_string();
    assert!(run(&["new", &path]).2);

    let (stdout, stderr, ok) = run(&["flash", &path, "--dry-run"]);
    assert!(ok, "{stderr}\n{stdout}");
    assert!(stdout.contains("protocol    stc89"), "{stdout}");
    assert!(stdout.contains("baud        115200"), "{stdout}");
    assert!(stdout.contains("parity none"), "{stdout}");
    assert!(stdout.contains("keep the chip's"), "{stdout}");
    assert!(stdout.contains("dry run"), "{stdout}");
    // No port was configured, so it must not have tried to open one.
    assert!(!stderr.contains("cannot open"), "{stderr}");

    // The plan reports the real firmware size.
    assert!(stdout.contains("blink.ihx"), "{stdout}");
    let size: usize = stdout
        .lines()
        .find_map(|line| {
            let (_, rest) = line.split_once('(')?;
            rest.strip_suffix(" bytes)")?.parse().ok()
        })
        .expect("an image size");
    // 裸机模板很小，但必须是个合理的镜像尺寸。
    assert!(
        (8..8192).contains(&size),
        "template firmware size {size} is implausible"
    );
}

#[test]
fn a_flash_simulation_downloads_and_verifies_the_firmware() {
    let dir = tmp("flash-sim");
    let path = dir.display().to_string();
    assert!(run(&["new", &path]).2);

    let (stdout, stderr, ok) = run(&["flash", &path, "--simulate"]);
    assert!(ok, "{stderr}\n{stdout}");
    // Progress goes to stderr so stdout stays pipeable; look at both.
    let both = format!("{stdout}{stderr}");
    assert!(both.contains("STC89C52RC"), "{both}");
    assert!(both.contains("BSL version"), "{both}");
    assert!(stdout.contains("校验通过"), "{stdout}");
    for stage in ["handshake", "baud", "erase", "program", "options", "run"] {
        assert!(
            both.contains(&format!("[{stage}]")),
            "missing {stage}: {both}"
        );
    }
}

#[test]
fn a_flash_simulation_honours_the_protocol_switch() {
    let dir = tmp("flash-stc12");
    let path = dir.display().to_string();
    assert!(run(&["new", &path]).2);
    std::fs::write(
        dir.join("mcs51.toml"),
        std::fs::read_to_string(dir.join("mcs51.toml"))
            .unwrap()
            .replace("protocol = \"stc89\"", "protocol = \"stc12\""),
    )
    .unwrap();

    let (stdout, stderr, ok) = run(&["flash", &path, "--simulate"]);
    assert!(ok, "{stderr}\n{stdout}");
    let both = format!("{stdout}{stderr}");
    assert!(both.contains("protocol    stc12"), "{both}");
    assert!(both.contains("parity even"), "{both}");
    assert!(both.contains("STC12C5A60S2"), "{both}");
    assert!(stdout.contains("校验通过"), "{stdout}");
}

#[test]
fn a_flash_without_a_port_explains_itself() {
    let dir = tmp("flash-noport");
    let path = dir.display().to_string();
    assert!(run(&["new", &path]).2);
    let (_, stderr, ok) = run(&["flash", &path]);
    assert!(!ok);
    assert!(stderr.contains("没有串口"), "{stderr}");
}

#[test]
fn a_flash_of_a_missing_chip_fails_with_a_clear_message() {
    let (_, stderr, ok) = run(&[
        "probe",
        "--port",
        "/dev/definitely-not-a-serial-port",
        "--protocol",
        "stc89",
    ]);
    assert!(!ok);
    assert!(
        stderr.contains("cannot open") || stderr.contains("No such file"),
        "{stderr}"
    );
}

#[test]
fn an_unknown_protocol_is_rejected() {
    let dir = tmp("flash-badproto");
    let path = dir.display().to_string();
    assert!(run(&["new", &path]).2);
    let (_, stderr, ok) = run(&["flash", &path, "--protocol", "nonsense", "--dry-run"]);
    assert!(!ok);
    assert!(stderr.contains("unknown protocol"), "{stderr}");
}
