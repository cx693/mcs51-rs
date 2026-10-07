//! `mcs51` — the `mcs51-rs` build driver.
//!
//! ```text
//! mcs51 new  <dir>                 scaffold a project
//! mcs51 build [dir]                assemble, link and package the firmware
//! mcs51 run  [dir]                 build, then run the firmware in the simulator
//! mcs51 asm  <file.asm>            assemble one file and print a listing
//! mcs51 disasm <file>              disassemble a HEX or binary image
//! mcs51 size <file>                print a size report for an image
//! mcs51 hex2bin <in.ihx> <out.bin> convert Intel HEX to raw binary
//! mcs51 bin2hex <in.bin> <out.ihx> convert raw binary to Intel HEX
//! mcs51 hexdump <file>             hex dump of an image
//! mcs51 isa [opcode]               describe the instruction set
//! ```

#![forbid(unsafe_code)]

// 清单解析器在库那边（见 src/lib.rs），这样测试可以拿去和真 TOML 解析器对照。
use mcs51_build::manifest;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::ExitCode;

use mcs51_asm::{Options, assemble};
use mcs51_flash::{Progress, ResetMode, StcFamily, StcOptions, Transport};
use mcs51_ihex::Image;
use mcs51_isa::{Mnemonic, decode_at, instruction_length, mem::Space};
use mcs51_ld::{LinkOptions, LinkerScript, link};
use mcs51_sim::{Machine, StopReason};

use manifest::Manifest;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None | Some("help") | Some("-h") | Some("--help") => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some("new") => cmd_new(&args[1..]),
        Some("build") => cmd_build(&args[1..]),
        Some("run") => cmd_run(&args[1..]),
        Some("flash") => cmd_flash(&args[1..]),
        Some("probe") => cmd_probe(&args[1..]),
        Some("devices") => cmd_devices(&args[1..]),
        Some("asm") => cmd_asm(&args[1..]),
        Some("disasm") => cmd_disasm(&args[1..]),
        Some("size") => cmd_size(&args[1..]),
        Some("hex2bin") => cmd_hex2bin(&args[1..]),
        Some("bin2hex") => cmd_bin2hex(&args[1..]),
        Some("hexdump") => cmd_hexdump(&args[1..]),
        Some("isa") => cmd_isa(&args[1..]),
        Some(other) => {
            eprintln!("error: unknown command `{other}`\n");
            print!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "\
mcs51-rs -- an 8051 toolchain written in Rust

USAGE:
    mcs51 <command> [options]

COMMANDS:
    new <dir>                  scaffold a project (mcs51.toml + src/main.asm)
    build [dir]                assemble, link and package the firmware
    run [dir]                  build, then run the firmware in the simulator
    flash [dir|file]           download the firmware to a chip over a serial port
    probe                      connect to the chip and print its identity
    devices                    list serial ports
    asm <file.asm>             assemble one file and print a listing
    disasm <file>              disassemble a HEX or binary image
    size <file>                size report for a HEX or binary image
    hex2bin <in> <out>         Intel HEX to raw binary
    bin2hex <in> <out>         raw binary to Intel HEX
    hexdump <file>             hex dump of an image
    isa [opcode]               describe the instruction set

OPTIONS:
    -C, --dir <dir>            project directory (default: current directory)
        --quiet                only print problems
        --instructions <n>     instruction budget for `run`
        --input <text>         bytes to feed the UART before `run`
        --base <addr>          load address for raw binary input

FLASH OPTIONS:
    -p, --port <PORT>          serial port, e.g. /dev/ttyUSB0 or COM3
    -P, --protocol <FAMILY>    stc89 (default) or stc12
    -b, --baud <BAUD>          transfer baud rate (default 115200)
        --handshake <BAUD>     baud rate used to find the chip (default 2400)
    -r, --reset <MODE>         manual, dtr, rts, dtr_inverted, rts_inverted
        --option <BYTE>        option/fuse byte to write (default: keep the chip's)
        --no-options           do not write the option byte at all
        --flash-size <BYTES>   size of the part, used to reject an oversized image
        --dry-run              show what would be sent, do not touch the port
        --simulate             download into a simulated chip and verify the result
";

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Options that consume the argument that follows them.
const VALUE_OPTIONS: [&str; 14] = [
    "-C",
    "--dir",
    "--instructions",
    "--input",
    "--base",
    "--count",
    "-p",
    "--port",
    "-P",
    "--protocol",
    "-b",
    "--baud",
    "--handshake",
    "--timeout",
];

/// The positional arguments, with option values removed.
fn positionals(args: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if VALUE_OPTIONS.contains(&arg.as_str()) {
            i += 2;
            continue;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        out.push(arg);
        i += 1;
    }
    out
}

fn project_dir(args: &[String]) -> Result<PathBuf, String> {
    if let Some(dir) = option(args, "--dir").or_else(|| option(args, "-C")) {
        return Ok(PathBuf::from(dir));
    }
    // `mcs51 flash build/blink.ihx` names a file, not a project directory, so
    // only accept a positional that really is a directory.
    Ok(positionals(args)
        .first()
        .map(|p| PathBuf::from(p.as_str()))
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| PathBuf::from(".")))
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn option(args: &[String], name: &str) -> Option<String> {
    let index = args.iter().position(|a| a == name)?;
    args.get(index + 1).cloned()
}

/// Look for a long option or its short alias: `option_any(args, &["--port", "-p"])`.
fn option_any(args: &[String], names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| option(args, name))
}

fn load_manifest(args: &[String]) -> Result<Manifest, String> {
    let dir = project_dir(args)?;
    if dir.join("mcs51.toml").exists() {
        Manifest::load(&dir).map_err(|e| e.to_string())
    } else {
        Ok(Manifest::defaults(dir))
    }
}

fn read_image(path: &Path) -> Result<Image, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    if text.trim_start().starts_with(':') {
        Image::from_intel_hex(&text).map_err(|e| format!("{}: {e}", path.display()))
    } else {
        Ok(Image::from_binary(0, &bytes))
    }
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

fn cmd_new(args: &[String]) -> Result<(), String> {
    let dir = positionals(args)
        .first()
        .copied()
        .ok_or_else(|| "usage: mcs51 new <dir>".to_string())?;
    let root = PathBuf::from(dir);
    if root.join("mcs51.toml").exists() {
        return Err(format!("{} already contains a project", root.display()));
    }
    std::fs::create_dir_all(root.join("src"))
        .map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    std::fs::write(root.join("mcs51.toml"), manifest::TEMPLATE_MANIFEST)
        .map_err(|e| format!("cannot write mcs51.toml: {e}"))?;
    std::fs::write(root.join("src/main.asm"), manifest::TEMPLATE_APP)
        .map_err(|e| format!("cannot write src/main.asm: {e}"))?;
    println!("created project `{}`", root.display());
    println!("  mcs51.toml");
    println!("  src/main.asm");
    println!("\nnext:  cd {} && mcs51 build && mcs51 run", root.display());
    Ok(())
}

/// A symbol worth reporting: a real label in a section, with its address.
#[derive(Debug, Clone)]
struct SymbolEntry {
    name: String,
    value: u32,
    section: Option<String>,
}

/// A finished firmware, however it was produced.
struct Firmware {
    code: Image,
    symbols: Vec<SymbolEntry>,
    sections: Vec<mcs51_ld::PlacedSection>,
    /// 生成的目标机汇编（只有 Rust 工程会有；汇编工程用 units 里的清单）。
    assembly: String,
    units: Vec<mcs51_asm::AssembleOutput>,
    map: Option<String>,
}

/// `true` when the application is generated by a Rust program rather than
/// assembled from `.asm` sources.
fn is_rust_project(manifest: &Manifest) -> bool {
    manifest.build.language == "rust"
        || manifest
            .build
            .sources
            .iter()
            .any(|source| source.ends_with(".rs"))
        || manifest.root.join("Cargo.toml").exists()
}

/// Run the project's Rust generator and read back the firmware it produced.
///
/// The generator is an ordinary Cargo binary: it runs on the host, emits 8051
/// code and writes `.ihx` / `.bin` / `.map` / `.lst` / `.asm` into the output
/// directory. This is the only way to have "the application written in Rust" —
/// `rustc` has no MCS-51 backend, so the Rust runs at build time, not on the chip.
fn build_rust_project(manifest: &Manifest) -> Result<Firmware, String> {
    let manifest_path = manifest.root.join("Cargo.toml");
    if !manifest_path.exists() {
        return Err(format!(
            "{} 声明了 Rust 工程，但找不到 {}",
            manifest.path.display(),
            manifest_path.display()
        ));
    }

    let mut command = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    command
        .arg("run")
        .arg("--release")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&manifest_path);
    if !manifest.build.rust_bin.is_empty() {
        command.arg("--bin").arg(&manifest.build.rust_bin);
    }
    // Let the generator find its own crate directory even though cargo runs it
    // with our working directory.
    command
        .env("MCS51_OUT_DIR", manifest.out_dir())
        .env("MCS51_PROJECT_NAME", &manifest.project.name);

    let output = command
        .output()
        .map_err(|e| format!("无法运行 cargo（用于生成 Rust 应用）: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "Rust 应用生成失败（cargo run 退出码 {:?}）:\n{stdout}{stderr}",
            output.status.code()
        ));
    }
    if !flag_from_env("MCS51_QUIET") {
        // The generator prints its own summary; pass it through only on failure
        // so a successful `mcs51 build` stays tidy.
        let _ = output.stdout;
    }

    // Read back what it wrote.
    let out_dir = manifest.out_dir();
    let stem = &manifest.project.name;
    let hex_path = out_dir.join(format!("{stem}.ihx"));
    let hex = std::fs::read_to_string(&hex_path).map_err(|e| {
        format!(
            "Rust 生成器没有产出 {}: {e}\n（生成器的产物名必须等于 mcs51.toml 里的 project.name）",
            hex_path.display()
        )
    })?;
    let image = Image::from_intel_hex(&hex).map_err(|e| e.to_string())?;
    let read = |ext: &str| -> String {
        std::fs::read_to_string(out_dir.join(format!("{stem}.{ext}"))).unwrap_or_default()
    };
    let assembler_source = read("asm");
    Ok(Firmware {
        code: image.clone(),
        symbols: Vec::new(),
        sections: Vec::new(),
        map: Some(read("map")),
        units: Vec::new(),
        assembly: assembler_source,
    })
}

fn flag_from_env(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| value == "1")
}

fn build_project(manifest: &Manifest) -> Result<Firmware, String> {
    if is_rust_project(manifest) {
        return build_rust_project(manifest);
    }

    let script = match &manifest.link.script {
        Some(path) => {
            let full = manifest.resolve(path);
            let text = std::fs::read_to_string(&full)
                .map_err(|e| format!("cannot read {}: {e}", full.display()))?;
            let mut script = LinkerScript::parse(&text).map_err(|e| e.to_string())?;
            if let Some(address) = manifest.link.checksum {
                script.checksum = Some((address, Space::Code));
            }
            Some(script)
        }
        None => None,
    };

    // Read every source, failing early with a useful message.
    let mut sources = Vec::new();
    for relative in &manifest.build.sources {
        let path = manifest.resolve(relative);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        sources.push((relative.clone(), text, path));
    }

    // Assemble every source, then link them together.
    let mut units = Vec::new();
    let mut messages = Vec::new();
    for (name, text, path) in &sources {
        let mut options = Options::new(name);
        if let Some(parent) = path.parent() {
            options.include_paths.push(parent.to_path_buf());
        }
        options.include_paths.push(manifest.root.clone());
        let unit = assemble(text, &options);
        if !unit.is_ok() {
            messages.push(unit.render_diagnostics());
        }
        units.push(unit);
    }
    if !messages.is_empty() {
        return Err(messages.join("\n"));
    }
    let objects: Vec<mcs51_asm::ObjectFile> = units.iter().map(|u| u.object.clone()).collect();
    let options = LinkOptions {
        script: script.unwrap_or_else(LinkerScript::default_8051),
        ..Default::default()
    };
    let output = link(&objects, &options);
    if !output.is_ok() {
        return Err(output.render_diagnostics());
    }
    let mut symbols: Vec<SymbolEntry> = output
        .symbols
        .iter()
        .map(|(name, sym)| SymbolEntry {
            name: name.clone(),
            value: sym.value,
            section: sym.section.clone(),
        })
        .collect();
    symbols.sort_by_key(|entry| entry.value);
    Ok(Firmware {
        code: output.code.clone(),
        symbols,
        sections: output.sections.clone(),
        assembly: String::new(),
        map: Some(output.map_file(&objects)),
        units,
    })
}

fn cmd_build(args: &[String]) -> Result<(), String> {
    let manifest = load_manifest(args)?;
    let quiet = flag(args, "--quiet");
    let firmware = build_project(&manifest)?;

    let out_dir = manifest.out_dir();
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    let stem = &manifest.project.name;

    let hex_path = out_dir.join(format!("{stem}.ihx"));
    let bin_path = out_dir.join(format!("{stem}.bin"));
    std::fs::write(&hex_path, firmware.code.to_intel_hex())
        .map_err(|e| format!("cannot write {}: {e}", hex_path.display()))?;
    let binary = firmware.code.to_binary(0xFF).unwrap_or_default();
    std::fs::write(&bin_path, &binary)
        .map_err(|e| format!("cannot write {}: {e}", bin_path.display()))?;

    if manifest.build.listing {
        let listing = render_listing(&firmware);
        let path = out_dir.join(format!("{stem}.lst"));
        std::fs::write(&path, listing)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    if manifest.build.map
        && let Some(map) = &firmware.map
    {
        let path = out_dir.join(format!("{stem}.map"));
        std::fs::write(&path, map).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }

    if !quiet {
        println!(
            "mcs51-rs build: {} ({})",
            manifest.project.name, manifest.project.device
        );
        if !manifest.project.description.is_empty() {
            println!("  {}", manifest.project.description);
        }
        if manifest.path.exists() {
            println!(
                "  manifest {}",
                relative_to(&manifest.root, &manifest.path).display()
            );
        }
        println!();
        print!("{}", size_summary(&firmware));
        println!("\nartifacts");
        for path in [&hex_path, &bin_path] {
            println!("  {}", relative_to(&manifest.root, path).display());
        }
        if manifest.build.listing {
            println!(
                "  {}",
                relative_to(&manifest.root, &out_dir.join(format!("{stem}.lst"))).display()
            );
        }
        if manifest.build.map {
            println!(
                "  {}",
                relative_to(&manifest.root, &out_dir.join(format!("{stem}.map"))).display()
            );
        }
    }
    Ok(())
}

/// Machine cycles one UART byte takes on the wire: 10 bit times.
fn uart_tx_cycles(cpu_hz: u32, baud: u32) -> u32 {
    let cycles_per_bit = (cpu_hz / 12) / baud.max(1);
    cycles_per_bit.saturating_mul(10)
}

fn cmd_run(args: &[String]) -> Result<(), String> {
    let manifest = load_manifest(args)?;
    let firmware = build_project(&manifest)?;

    let instructions = option(args, "--instructions")
        .and_then(|v| v.parse().ok())
        .unwrap_or(manifest.sim.instructions);
    let input = option(args, "--input").unwrap_or_else(|| manifest.sim.input.clone());

    let mut machine = Machine::from_image(&firmware.code);
    machine.peripherals_mut().uart.tx_cycles =
        uart_tx_cycles(manifest.sim.cpu_hz, manifest.sim.uart_baud);
    if !input.is_empty() {
        machine.feed_uart(input.as_bytes());
    }

    println!("mcs51-rs run: {}", manifest.project.name);
    println!(
        "  {} Hz, UART {} baud, budget {} instructions\n",
        manifest.sim.cpu_hz, manifest.sim.uart_baud, instructions
    );

    let reason = machine.run(instructions);
    print!("{}", machine.console_string());
    if !machine.console().is_empty() && !machine.console_string().ends_with('\n') {
        println!();
    }
    println!("\n--- {reason:?} ---");
    let snapshot = machine.snapshot();
    println!(
        "cycles {}  instructions {}  PC {:#06X}  SP {:#04X}  A {:#04X}  DPTR {:#06X}",
        snapshot.cycles, snapshot.instructions, snapshot.pc, snapshot.sp, snapshot.a, snapshot.dptr
    );
    println!(
        "UART tx {} bytes  timer0 {:#06X}  timer1 {:#06X}  TCON {:#04X}",
        snapshot.tx_bytes, snapshot.timer0, snapshot.timer1, snapshot.tcon
    );
    if matches!(reason, StopReason::StepLimit) {
        println!("\n(the instruction budget ran out; raise it with --instructions)");
    }
    Ok(())
}

fn render_listing(firmware: &Firmware) -> String {
    // Rust 工程已经自带 .lst（由生成器写出），这里不重复。
    if firmware.units.is_empty() {
        return firmware.assembly.clone();
    }
    let mut out = String::new();
    for unit in &firmware.units {
        let _ = writeln!(out, "; ===== {} =====", unit.object.name);
        for line in &unit.listing {
            let bytes = line
                .bytes
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ");
            match line.addr {
                Some(addr) if !line.bytes.is_empty() => {
                    let _ = writeln!(out, "{addr:04X}  {bytes:<12} {}", line.text.trim_end());
                }
                _ => {
                    let _ = writeln!(out, "      {:<12} {}", "", line.text.trim_end());
                }
            }
        }
        out.push('\n');
    }
    out
}

fn size_summary(firmware: &Firmware) -> String {
    let mut out = String::new();
    let report = firmware.code.size_report();
    let _ = writeln!(out, "memory");
    let _ = writeln!(
        out,
        "  CODE   {}",
        match (report.low, report.high) {
            (Some(low), Some(high)) => format!("{low:#06X}..{high:#06X}   {} bytes", report.bytes),
            _ => "empty".into(),
        }
    );
    let _ = writeln!(
        out,
        "  free   {} bytes inside 0x0000..0x8000",
        0x8000u32 - report.bytes as u32
    );
    let _ = writeln!(out, "\nsections");
    for section in &firmware.sections {
        let _ = writeln!(
            out,
            "  {:<8} {:<6} {:#06X} {:>6} bytes",
            section.name,
            section.space.name(),
            section.addr,
            section.len
        );
    }
    // Only real labels in a placed section are interesting; the many `.equ`
    // constants that 8051.inc defines are not.
    let mut ranked: Vec<&SymbolEntry> = firmware
        .symbols
        .iter()
        .filter(|entry| entry.section.as_deref() == Some("CODE"))
        .collect();
    ranked.sort_by_key(|entry| std::cmp::Reverse(entry.value));
    if !ranked.is_empty() {
        let _ = writeln!(out, "\nsymbols (highest addresses first)");
        for entry in ranked.iter().take(14) {
            let _ = writeln!(out, "  {:#06X}  {}", entry.value, entry.name);
        }
    }
    out
}

fn cmd_asm(args: &[String]) -> Result<(), String> {
    let path = positionals(args)
        .first()
        .copied()
        .ok_or_else(|| "usage: mcs51 asm <file.asm>".to_string())?;
    let path = PathBuf::from(path);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut options = Options::new(path.display().to_string());
    if let Some(parent) = path.parent() {
        options.include_paths.push(parent.to_path_buf());
    }
    options.include_paths.push(PathBuf::from("."));
    let output = assemble(&text, &options);
    for diagnostic in &output.diagnostics {
        eprintln!("{diagnostic}");
    }
    for line in &output.listing {
        let bytes = line
            .bytes
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        match line.addr {
            Some(addr) if !line.bytes.is_empty() => {
                println!("{addr:04X}  {bytes:<12} {}", line.text.trim_end());
            }
            _ => println!("      {:<12} {}", "", line.text.trim_end()),
        }
    }
    println!("\n{}", output.object.summary());
    if !output.is_ok() {
        return Err("assembly failed".into());
    }
    Ok(())
}

fn cmd_disasm(args: &[String]) -> Result<(), String> {
    let path = positionals(args)
        .first()
        .copied()
        .ok_or_else(|| "usage: mcs51 disasm <file>".to_string())?;
    let base: u16 = option(args, "--base")
        .and_then(|v| parse_number(&v))
        .unwrap_or(0) as u16;
    let count: usize = option(args, "--count")
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);
    let image = read_image(Path::new(path))?;

    let mut addr = base;
    let mut emitted = 0usize;
    while emitted < count {
        let Some(byte) = image.get(addr) else {
            if addr == u16::MAX {
                break;
            }
            addr += 1;
            continue;
        };
        let length = instruction_length(byte) as usize;
        let bytes: Vec<u8> = (0..length)
            .map(|i| image.get(addr.wrapping_add(i as u16)).unwrap_or(0xFF))
            .collect();
        match decode_at(&bytes, 0) {
            Ok(instruction) => {
                println!("{addr:04X}  {:<9} {}", hex(&bytes), instruction);
                addr = addr.wrapping_add(length as u16);
            }
            Err(_) => {
                println!("{addr:04X}  {byte:02X}        ???");
                addr = addr.wrapping_add(1);
            }
        }
        emitted += 1;
    }
    Ok(())
}

fn cmd_size(args: &[String]) -> Result<(), String> {
    let path = positionals(args)
        .first()
        .copied()
        .ok_or_else(|| "usage: mcs51 size <file>".to_string())?;
    let image = read_image(Path::new(path))?;
    println!("{}", image.size_report());
    for (start, data) in image.runs() {
        println!(
            "  {start:#06X}..{:#06X}  {} bytes",
            start as u32 + data.len() as u32 - 1,
            data.len()
        );
    }
    Ok(())
}

fn cmd_hex2bin(args: &[String]) -> Result<(), String> {
    let files = positionals(args);
    let (input, output) = files
        .split_first()
        .and_then(|(a, rest)| rest.first().map(|b| (*a, *b)))
        .ok_or_else(|| "usage: mcs51 hex2bin <in.ihx> <out.bin>".to_string())?;
    let image = read_image(Path::new(input))?;
    let binary = image
        .to_binary(0xFF)
        .ok_or_else(|| "the image is empty".to_string())?;
    std::fs::write(output, &binary).map_err(|e| format!("cannot write {output}: {e}"))?;
    println!("{output}: {} bytes", binary.len());
    Ok(())
}

fn cmd_bin2hex(args: &[String]) -> Result<(), String> {
    let files = positionals(args);
    let (input, output) = files
        .split_first()
        .and_then(|(a, rest)| rest.first().map(|b| (*a, *b)))
        .ok_or_else(|| "usage: mcs51 bin2hex <in.bin> <out.ihx> [--base addr]".to_string())?;
    let data = std::fs::read(input).map_err(|e| format!("cannot read {input}: {e}"))?;
    let base = option(args, "--base")
        .and_then(|v| parse_number(&v))
        .unwrap_or(0) as u16;
    let image = Image::from_binary(base, &data);
    std::fs::write(output, image.to_intel_hex())
        .map_err(|e| format!("cannot write {output}: {e}"))?;
    println!(
        "{output}: {} bytes at {base:#06X}",
        image.size_report().bytes
    );
    Ok(())
}

fn cmd_hexdump(args: &[String]) -> Result<(), String> {
    let path = positionals(args)
        .first()
        .copied()
        .ok_or_else(|| "usage: mcs51 hexdump <file>".to_string())?;
    let image = read_image(Path::new(path))?;
    print!("{}", image.to_hex_dump());
    Ok(())
}

fn cmd_isa(args: &[String]) -> Result<(), String> {
    let wanted: Option<u8> = positionals(args)
        .first()
        .and_then(|v| parse_number(v))
        .map(|v| v as u8);

    match wanted {
        Some(opcode) => match mcs51_isa::opcode_def(opcode) {
            Some(def) => {
                println!("opcode {opcode:#04X}");
                println!("  mnemonic {}", def.mnemonic);
                println!("  operands {:?}", def.args);
                println!("  length   {} bytes", def.len());
                println!("  cycles   {}", def.cycles);
                println!("  flags    {:#04X}", def.flags);
                println!(
                    "  opcodes  {}",
                    (0..def.variant_count())
                        .map(|n| format!("{:#04X}", def.variant(n)))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            None => {
                return Err(format!("{opcode:#04X} is not a defined 8051 opcode"));
            }
        },
        None => {
            let mut grouped: std::collections::BTreeMap<Mnemonic, Vec<String>> = Default::default();
            for def in mcs51_isa::OPCODES {
                for n in 0..def.variant_count() {
                    grouped
                        .entry(def.mnemonic)
                        .or_default()
                        .push(format!("{:#04X}", def.variant(n)));
                }
            }
            println!(
                "8051 instruction set: {} opcodes, {} undefined byte(s)\n",
                mcs51_isa::OPCODES
                    .iter()
                    .map(|d| d.variant_count() as usize)
                    .sum::<usize>(),
                mcs51_isa::UNDEFINED_OPCODES.len()
            );
            for (mnemonic, opcodes) in grouped {
                println!("  {:<6} {}", mnemonic.as_str(), opcodes.join(" "));
            }
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_number(text: &str) -> Option<u32> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).ok();
    }
    text.parse().ok()
}

fn relative_to(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_manifest() {
        let manifest = Manifest::parse(
            r#"
# a project
[project]
name = "demo"
device = "AT89C51"

[build]
sources = ["src/a.asm", "src/b.asm"]
out = "out"

[link]
checksum = 0x7FFE

[sim]
instructions = 1234
input = "hi"
cpu_hz = 12000000
uart_baud = 115200
"#,
            "/tmp/demo",
        )
        .unwrap();
        assert_eq!(manifest.project.name, "demo");
        assert_eq!(manifest.build.sources, vec!["src/a.asm", "src/b.asm"]);
        assert_eq!(manifest.build.out, "out");
        assert_eq!(manifest.link.checksum, Some(0x7FFE));
        assert_eq!(manifest.sim.instructions, 1234);
        assert_eq!(manifest.sim.input, "hi");
        assert_eq!(manifest.sim.cpu_hz, 12_000_000);
        assert_eq!(manifest.sim.uart_baud, 115_200);
        assert_eq!(
            manifest.resolve("src/a.asm"),
            PathBuf::from("/tmp/demo/src/a.asm")
        );
    }

    #[test]
    fn manifest_defaults_are_usable() {
        let manifest = Manifest::parse("", "/tmp/x").unwrap();
        assert_eq!(manifest.build.sources, vec!["src/main.asm"]);
        assert_eq!(manifest.build.language, "asm");
        // 模拟器需要一个晶体频率才能算串口位宽，默认必须是个合理的值。
        assert_eq!(manifest.sim.cpu_hz, 11_059_200);
        assert_eq!(manifest.sim.uart_baud, 9600);
        assert!(manifest.sim.instructions > 0);
    }

    #[test]
    fn rejects_malformed_manifests() {
        assert!(Manifest::parse("[project\n", "/tmp").is_err());
        assert!(Manifest::parse("name\n", "/tmp").is_err());
    }

    #[test]
    fn the_template_project_builds_and_runs() {
        let manifest = Manifest::parse(manifest::TEMPLATE_MANIFEST, "/tmp/template").unwrap();
        assert_eq!(manifest.project.name, "blink");
        assert_eq!(manifest.sim.cpu_hz, 11_059_200);

        // 模板应用是裸机的，必须能单独汇编 + 链接 + 跑起来。
        let output = assemble(manifest::TEMPLATE_APP, &Options::new("main.asm"));
        assert!(
            output.is_ok(),
            "template app failed to assemble:\n{}",
            output.render_diagnostics()
        );
        let linked = link(&[output.object.clone()], &LinkOptions::default());
        assert!(linked.is_ok(), "{}", linked.render_diagnostics());

        let mut machine = Machine::from_image(&linked.code);
        machine.halt_on_self_jump = false;
        machine.peripherals_mut().log_ports = true;
        machine.run_cycles(2_000_000);

        // P1.0 必须真的在翻转，而且周期符合 0.5 秒左右的延时。
        let p1: Vec<u8> = machine
            .peripherals()
            .port_log
            .iter()
            .filter(|(address, _)| *address == 0x90)
            .map(|(_, value)| *value)
            .collect();
        let toggles: Vec<usize> = (1..p1.len())
            .filter(|i| (p1[*i - 1] ^ p1[*i]) & 0x01 != 0)
            .collect();
        assert!(
            toggles.len() >= 2,
            "template firmware never toggled P1.0: {p1:02X?}"
        );
        assert!(toggles.len() >= 2, "need at least two toggles");
    }
}

// ---------------------------------------------------------------------------
// Flashing
// ---------------------------------------------------------------------------

/// The resolved settings for one download.
struct FlashPlan {
    port: String,
    family: StcFamily,
    baud: u32,
    handshake_baud: u32,
    reset: ResetMode,
    #[allow(dead_code)]
    option: Option<u8>,
    write_options: bool,
    flash_size: usize,
    /// How long to wait for the chip to answer the synchronisation pulse.
    sync_timeout: std::time::Duration,
    /// Where the image came from: a built project, or a file on disk.
    image: String,
    image_name: String,
}

fn reset_mode(text: &str) -> Result<ResetMode, String> {
    Ok(match text.to_ascii_lowercase().as_str() {
        "manual" | "none" | "" => ResetMode::Manual,
        "dtr" => ResetMode::Dtr,
        "rts" => ResetMode::Rts,
        "dtr_inverted" | "dtr-inverted" => ResetMode::DtrInverted,
        "rts_inverted" | "rts-inverted" => ResetMode::RtsInverted,
        other => {
            return Err(format!(
                "unknown reset mode `{other}` (manual, dtr, rts, dtr_inverted, rts_inverted)"
            ));
        }
    })
}

/// Work out where the image comes from and what settings to use.
fn flash_plan(args: &[String], manifest: &Manifest) -> Result<FlashPlan, String> {
    // A bare `mcs51 flash <file.ihx>` downloads a file; anything else builds
    // the project first.
    let positional = positionals(args)
        .into_iter()
        .find(|arg| {
            let path = Path::new(arg.as_str());
            path.extension().is_some_and(|e| {
                e.eq_ignore_ascii_case("ihx")
                    || e.eq_ignore_ascii_case("hex")
                    || e.eq_ignore_ascii_case("bin")
            })
        })
        .cloned();

    let (image, image_name) = match positional {
        Some(file) => {
            let path = PathBuf::from(&file);
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            (text, file)
        }
        None => {
            let firmware = build_project(manifest)?;
            let name = format!("{}.ihx", manifest.project.name);
            (firmware.code.to_intel_hex(), name)
        }
    };

    let family = match option_any(args, &["--protocol", "-P"]) {
        Some(text) => StcFamily::parse(&text)
            .ok_or_else(|| format!("unknown protocol `{text}` (stc89, stc12)"))?,
        None => StcFamily::parse(&manifest.flash.protocol).unwrap_or(StcFamily::Stc89),
    };

    let port = option_any(args, &["--port", "-p"])
        .or_else(|| {
            if manifest.flash.port.is_empty() {
                None
            } else {
                Some(manifest.flash.port.clone())
            }
        })
        .unwrap_or_default();

    Ok(FlashPlan {
        port,
        family,
        baud: option_any(args, &["--baud", "-b"])
            .and_then(|v| parse_number(&v))
            .unwrap_or(manifest.flash.baud),
        handshake_baud: option(args, "--handshake")
            .and_then(|v| parse_number(&v))
            .unwrap_or(manifest.flash.handshake_baud),
        reset: match option_any(args, &["--reset", "-r"]) {
            Some(text) => reset_mode(&text)?,
            None => reset_mode(&manifest.flash.reset)?,
        },
        option: match option(args, "--option") {
            Some(text) => {
                Some(parse_number(&text).ok_or_else(|| format!("bad option byte `{text}`"))? as u8)
            }
            None => manifest.flash.option,
        },
        write_options: !flag(args, "--no-options") && manifest.flash.write_options,
        sync_timeout: std::time::Duration::from_secs(
            option(args, "--timeout")
                .and_then(|v| parse_number(&v))
                .unwrap_or(20)
                .max(1) as u64,
        ),
        flash_size: option(args, "--flash-size")
            .and_then(|v| parse_number(&v))
            .map(|v| v as usize)
            .unwrap_or(manifest.flash.flash_size),
        image,
        image_name,
    })
}

fn print_flash_plan(plan: &FlashPlan, image: &[u8]) {
    println!("  image       {} ({} bytes)", plan.image_name, image.len());
    println!("  protocol    {}", plan.family.as_str());
    if plan.port.is_empty() {
        println!("  port        <not set; pass --port>");
    } else {
        println!("  port        {}", plan.port);
    }
    println!(
        "  baud        {} (handshake {}), parity {}",
        plan.baud,
        plan.handshake_baud,
        plan.family.parity()
    );
    println!("  reset       {:?}", plan.reset);
    println!("  等待应答    {} 秒", plan.sync_timeout.as_secs());
    println!(
        "  选项字节    {}",
        if plan.write_options {
            match plan.option {
                Some(byte) => format!("write {byte:#04X}"),
                None => "keep the chip's".to_string(),
            }
        } else {
            "leave alone".to_string()
        }
    );
}

fn stc_options(plan: &FlashPlan) -> StcOptions {
    StcOptions {
        sync_timeout: plan.sync_timeout,
        family: plan.family,
        handshake_baud: plan.handshake_baud,
        transfer_baud: plan.baud,
        reset: plan.reset.clone(),
        write_options: plan.write_options,
        option_override: plan.option,
    }
}

fn cmd_flash(args: &[String]) -> Result<(), String> {
    let quiet = flag(args, "--quiet");
    let manifest = load_manifest(args)?;
    let plan = flash_plan(args, &manifest)?;
    let data =
        mcs51_flash::flatten_image(&plan.image, plan.flash_size).map_err(|e| e.to_string())?;

    if !quiet {
        println!("mcs51-rs flash: {}", manifest.project.name);
        print_flash_plan(&plan, &data);
    }

    if flag(args, "--dry-run") {
        println!("\ndry run: nothing was sent to the port");
        return Ok(());
    }

    // `--simulate` runs the identical protocol against a model of the chip, so
    // the whole download path can be exercised without hardware.
    if flag(args, "--simulate") {
        let chip = std::rc::Rc::new(std::cell::RefCell::new(mcs51_flash::mock::MockStc::new(
            plan.family,
            plan.flash_size,
            match plan.family {
                StcFamily::Stc89 => 0xF002,
                StcFamily::Stc12 => 0xF047,
            },
            11_059_200.0,
        )));
        let transport = mcs51_flash::LoopbackTransport::shared(std::rc::Rc::clone(&chip), 2400);
        let mut progress = if quiet {
            Progress::silent()
        } else {
            Progress::new()
        };
        let report = mcs51_flash::flash_over_transport(
            transport,
            stc_options(&plan),
            &plan.image,
            plan.flash_size,
            &mut progress,
        )
        .map_err(|e| e.to_string())?;

        let chip = chip.borrow();
        let mut mismatch = None;
        for (index, expected) in data.iter().enumerate() {
            if chip.flash[index] != *expected {
                mismatch = Some((index, *expected, chip.flash[index]));
                break;
            }
        }
        match mismatch {
            Some((address, expected, actual)) => {
                return Err(format!(
                    "模拟校验失败 @ {address:#06X}：写入 {expected:#04X}，读回 {actual:#04X}"
                ));
            }
            None => {
                println!(
                    "\n模拟的 {} 已接受 {} 字节，逐字节校验通过",
                    report.model, report.bytes
                );
            }
        }
        return Ok(());
    }

    if plan.port.is_empty() {
        return Err("没有串口：用 --port 指定，或在 mcs51.toml 的 [flash] port 里写死".into());
    }

    let transport = mcs51_flash::SerialTransport::open(&plan.port, plan.handshake_baud)
        .map_err(|e| e.to_string())?;
    if !quiet {
        println!("\n已打开 {}", transport.describe());
    }
    let mut progress = if quiet {
        Progress::silent()
    } else {
        Progress::new()
    };
    let report = mcs51_flash::flash_over_transport(
        transport,
        stc_options(&plan),
        &plan.image,
        plan.flash_size,
        &mut progress,
    )
    .map_err(|e| e.to_string())?;

    println!(
        "\n{} 烧录完成：{} 字节，{:#06X}..{:#06X}，用时 {:.1} 秒",
        report.model,
        report.bytes,
        report.low,
        report.high,
        report.elapsed.as_secs_f64()
    );
    println!("芯片已复位，正在运行新固件");
    Ok(())
}

fn cmd_probe(args: &[String]) -> Result<(), String> {
    let manifest = load_manifest(args)?;
    let port = option_any(args, &["--port", "-p"])
        .or_else(|| {
            if manifest.flash.port.is_empty() {
                None
            } else {
                Some(manifest.flash.port.clone())
            }
        })
        .ok_or_else(|| "没有串口：用 -p / --port 指定".to_string())?;
    let family = match option_any(args, &["--protocol", "-P"]) {
        Some(text) => {
            StcFamily::parse(&text).ok_or_else(|| format!("unknown protocol `{text}`"))?
        }
        None => StcFamily::parse(&manifest.flash.protocol).unwrap_or(StcFamily::Stc89),
    };
    let options = StcOptions {
        family,
        handshake_baud: option(args, "--handshake")
            .and_then(|v| parse_number(&v))
            .unwrap_or(manifest.flash.handshake_baud),
        sync_timeout: std::time::Duration::from_secs(
            option(args, "--timeout")
                .and_then(|v| parse_number(&v))
                .unwrap_or(20)
                .max(1) as u64,
        ),
        reset: match option_any(args, &["--reset", "-r"]) {
            Some(text) => reset_mode(&text)?,
            None => reset_mode(&manifest.flash.reset)?,
        },
        ..Default::default()
    };

    let transport = mcs51_flash::SerialTransport::open(&port, options.handshake_baud)
        .map_err(|e| e.to_string())?;
    let mut progress = Progress::new();
    let mut flasher = mcs51_flash::StcFlasher::new(transport, options);
    let info = flasher.connect(&mut progress).map_err(|e| e.to_string())?;
    let _ = flasher.disconnect();
    println!("{}", info.model);
    println!("  型号 ID     {:#06X}", info.magic);
    println!("  BSL 版本    {}", info.bsl_version);
    println!("  实测主频    {:.3} MHz", info.clock_mhz());
    println!(
        "  选项字节    {}",
        mcs51_flash::Stc89Options::new(info.option_byte)
    );
    Ok(())
}

fn cmd_devices(_args: &[String]) -> Result<(), String> {
    let ports = mcs51_flash::serial_ports();
    if ports.is_empty() {
        println!("没有找到串口");
        return Ok(());
    }
    println!("可用串口");
    for port in ports {
        println!("  {port}");
    }
    Ok(())
}
