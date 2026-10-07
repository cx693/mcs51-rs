# mcs51-rs — an 8051 bare-metal toolchain written entirely in Rust

[简体中文](README.md) | **English**

`mcs51-rs` is an Intel MCS-51 (8051) toolchain **written entirely in Rust**.
It rewrites, in Rust, the parts of the SDCC toolchain that belong to the 8051
backend — the assembler `sdas8051`, the linker `sdld`, `packihx`/`makebin`,
`ucsim` — and adds a chip standard library, a Rust → 8051 code generator, and
the STC serial download protocol on top.

**What you get is pure bare-metal firmware**: reset vector + interrupt vector
table + your code. No scheduler, no systick, no runtime library — the meagre
resources of an 8051 are not worth feeding a kernel.

**Zero external dependencies**: no SDCC, no vendor assembler, no hardware.
Assemble → link → Intel HEX → run it in the simulator to verify — the whole
chain closes inside a single `cargo test`.

> 🎬 **Video walkthrough (Chinese)**: [【在Rust上开发8051单片机的模版！】](https://www.bilibili.com/video/BV1GPpT6PEFP/?share_source=copy_web&vd_source=cd84a64418dd304d8fed74a9172a37da) (Bilibili)

```
$ mcs51 new blink && cd blink
$ mcs51 build
mcs51-rs build: blink (8051)
  manifest mcs51.toml

memory
  CODE   0x0000..0x0097   152 bytes
  free   32616 bytes inside 0x0000..0x8000
...
$ mcs51 run
t=00
t=01
t=02
```

---

## 1. Why not "compile Rust straight to the 8051"

Rust has no 8051 backend today (LLVM has no MCS-51 target), so the only viable
route to "generate 8051 firmware with Rust" is: **implement the whole 8051
backend toolchain in Rust, then have Rust generate 8051 code at build time**.

That is exactly the route this project takes:

| SDCC component | This project's Rust implementation | Notes |
|---|---|---|
| `sdas8051` (assembler) | `mcs51-asm` | two-pass assembly, expressions, relocatable object files |
| `sdld` (linker) + `.lkr` | `mcs51-ld` | linker script, symbol resolution, relocation, MAP file, checksum |
| `packihx` / `makebin` / `sdobjcopy` | `mcs51-ihex` | Intel HEX / binary / sparse image |
| `ucsim` (simulator) | `mcs51-sim` | built on the `i8051` CPU core + Timer0/1, UART, ports |
| `Makefile` / `sdcc` driver | `mcs51-build` (the `mcs51` CLI) | project manifest, build, size report, download |
| STC-ISP.exe / `stcgal` / `stcflash` | `mcs51-flash` | the STC BSL serial download protocol, reproduced in Rust |
| hand-written assembly / SDCC's C | `mcs51-codegen` | **write applications in Rust**, generate 8051 machine code at build time; ships a GPIO/timer/serial HAL |
| Keil's `reg52.h` / `STC89C5xRC.h` | `mcs51-chips` | **chip standard library**: SFR and bit definitions, with the Chinese descriptions carried over |

The instruction set itself (`mcs51-isa`) has no SDCC counterpart — it is the
**single source of truth** for the whole chain: the assembler encodes from it,
the disassembler decodes from it, the simulator takes cycle counts from it, and
the size report takes its metadata from it. And it is `#![no_std]` — the same
code both assembles firmware on the host and disassembles at runtime on the
target.

---

## 2. Quick start

### 2.1 Build the toolchain

```bash
git clone <this repo> mcs51-rs && cd mcs51-rs
cargo build --release
cargo test --workspace          # 174 tests
```

> The current workspace wraps builds in `cargo-local.sh` (because this
> machine's `CARGO_HOME` / `CARGO_TARGET_DIR` point outside the workspace).
> In a normal environment plain `cargo build` is all you need.

### 2.2 Command line

```
mcs51 new <dir>                  scaffold: generate mcs51.toml + src/main.asm
mcs51 build [dir]                assemble → link → package (.ihx/.bin/.lst/.map)
mcs51 run [dir]                  build, then run in the simulator, printing serial output and task state
mcs51 flash [dir|file.ihx]       download to a real chip over serial (STC BSL protocol)
mcs51 probe -p <PORT>            read chip model/clock/BSL version, without writing
mcs51 devices                    list system serial ports
mcs51 asm <file.asm>             assemble a single file and emit a listing
mcs51 disasm <file>              disassemble HEX / BIN
mcs51 size <file>                image size report
mcs51 hex2bin / bin2hex          format conversion
mcs51 hexdump <file>             hex dump
mcs51 isa [opcode]               query the instruction set (opcode, length, cycles, flags affected)
```

### 2.3 Project manifest `mcs51.toml`

```toml
[project]
name = "blink"
device = "AT89C51"

[build]
sources = ["src/main.asm"]
out = "build"

[sim]
instructions = 400000
cpu_hz = 11059200     # the simulator uses this for machine cycles and serial bit width
uart_baud = 9600

[flash]
protocol = "stc89"

[link]
checksum = 0x7FFE     # optional: place an image checksum at this address
```

`[sim]` only affects how the simulator runs your firmware — the firmware itself
never reads these values, so changing them does not change what is generated.

---

## 3. What a bare-metal program looks like

There is no kernel, so the program structure is the plainest possible 8051:

```asm
        .area CODE (ABS)
        .org  0x0000
        LJMP  main                   ; reset vector

        .org  0x0003                 ; interrupt vector table: this project uses no
        RETI                         ; interrupts, but the slots must be filled or an
        .org  0x000B                 ; unexpected IRQ runs off to 0xFF
        RETI                         ; Timer0
        ; ... the remaining vectors likewise

        .org  0x0030
main:
        MOV   SP, #0x30              ; after reset SP=0x07, it must be reset
loop:
        CPL   P1_0
        ACALL delay
        SJMP  loop

delay:  MOV   R7, #250
inner:  DJNZ  R7, inner
        RET
```

`mcs51 build` decides nothing for you: it is an assembler + linker + packager.
Timing, stack, registers, interrupts — all of it stays in your hands.

**Writing in Rust works the same way**, you just replace instructions with
method calls (see §5):

```rust
let mut app = App::new(board);
app.main(|t| {
    t.forever(|t| {
        t.cpl(P10);
        t.delay_ms(500);
    });
});
app.finish()?.write_to("build")?;
```

---

## 4. Case study: an STC89C52RC LED chaser

`examples/led-flow/` is the project best suited to **your first run on real
silicon**: three LEDs on P3.5/P3.6/P3.7 light up in turn, roughly 250 ms each,
in only **93 bytes** of code.

```bash
mcs51 build examples/led-flow                  # 93 bytes, 0x0000..0x005C
mcs51 flash examples/led-flow -p /dev/ttyUSB0  # power-cycle the board when prompted
```

Wiring (active low):

```
VCC ──►|── LED1 ──[330Ω]── P3.5
VCC ──►|── LED2 ──[330Ω]── P3.6
VCC ──►|── LED3 ──[330Ω]── P3.7
USB-TTL TXD/RXD/GND/VCC ── P3.0/P3.1/GND/VCC
```

P3.5/6/7 are chosen because P3.0/P3.1 are the serial pins used for download,
P3.2/P3.3 are the interrupt pins and P3.4 is the counter pin — whereas
P3.5(T1)/P3.6(WR)/P3.7(RD) are plain clean I/O as long as you use neither the
timers nor external RAM.

Wiring, modifications and troubleshooting are all documented in
[`examples/led-flow/README.md`](examples/led-flow/README.md) (Chinese).

> **The crystal deserves its own paragraph**: an 8051 derives its serial baud
> rate from a timer by integer division, so not every baud rate can be hit
> exactly. An 11.0592 MHz crystal divides 9600 exactly (0% error), while at
> 12 MHz 9600 only reaches 10417 (8.5% error). You have to do that arithmetic
> yourself in code (see `uart_init` in `examples/blink/src/main.asm`).
> The download itself is **unaffected** — STC's BSL first measures the chip's
> real clock and works the baud rate back from it.

---

## 5. Writing applications in Rust (`mcs51-codegen`)

Application code can be written in **Rust**, generating 8051 machine code at
build time:

The application entry point is **`src/main.rs`**, with logic split into ordinary
Rust modules (`board.rs` / `leds.rs` / `console.rs`):

```rust
// src/main.rs — only assembly, no concrete instructions
mod board;
mod breath;
mod leds;

pub fn bare() -> Firmware {
    let mut app = App::new(board::board("led-flow-rs"));
    let breath = breath::Breath::new(app.statics(4));   // target-side statics
    let chaser = leds::Chaser::new(app.statics(2));

    // Timer: say "overflow every 200 microseconds", the reload value is computed on the host
    let tick = Timer::timer0()
        .micros(CPU_HZ, 200)
        .compensate(16);

    app.main(move |t| {
        chaser.init(t);
        breath.init(t);
        tick.begin(t);                      // read-modify-write TMOD + load reload + start
        t.forever(|t| { /* main loop */ });
    });
    app.finish().expect("firmware generation failed")
}
```

```rust
// src/leds.rs — the module exposes one target-side subroutine and an assembly function
pub fn all_off(app: &mut App) -> Routine {
    app.routine("leds_all_off", |t| {          // only one copy of the code on the target
        for pin in CHASE_LEDS { t.setb(pin); }
    })
}
```

**`Routine` and an ordinary Rust function are two different things**, and that
distinction is the watershed between a real project and a big demo:

| | ordinary Rust function | `Routine` |
|---|---|---|
| When it runs | build time (host) | runtime (chip) |
| Code | **inlined once per call site** | **exactly one copy** in the whole project |
| Call sites | none | `LCALL __rt_xxx` |

Measured: the inlined version emits `SETB P3_5` three times, the subroutine
version emits it once and produces smaller firmware.

### 5.1 Write peripherals with the HAL instead of configuring SFRs one by one

The genuinely annoying part of bare-metal 8051 code is not the instructions but
**the bookkeeping around registers**: the high nibble of `TMOD` belongs to
Timer 1 and the low nibble to Timer 0, so changing one means read-modify-write;
the baud rate requires backing `TH1` out of `fosc / (384 * baud)`; `PCON` is not
bit-addressable so it can only be modified as a whole byte; a 16-bit timer
counts at most 65536 machine cycles (about 71 ms at 11.0592 MHz), so a 500 ms
period means counting overflows yourself. All of that arithmetic **only needs
to be done once, on the host**.

So `mcs51-codegen` provides a thin HAL:

| Type | What it does for you |
|---|---|
| `Gpio` | remembers polarity — afterwards you only write `on()` / `off()` / `toggle()` |
| `Timer` | microseconds/milliseconds → reload value; `TMOD` read-modify-write; `wait()` fuses "wait for overflow + clear flag + reload" into one step |
| `Ticker` | splits periods longer than the timer range into multiple overflows automatically, and **distributes them evenly** so the total is accurate |
| `Console` | baud rate → `TH1`; configures `SCON`/`PCON`/Timer 1 in one go; installs the print subroutines once |

**An over-range period is a trap**: at 11.0592 MHz, `Timer::millis(cpu_hz, 500)`
needs 460800 machine cycles, which does not fit. Early versions **silently
clamped** it to 71 ms — the program still ran and the serial port still printed,
the period was just wrong by a factor of seven. Today it is a build error that
tells you to use `every_ms` instead:

```
error: the Timer period needs 460800 machine cycles, exceeding the 16-bit timer
       range of 65536. At 11.0592MHz one machine cycle is about 1.085us, so the
       range is about 71ms. For longer periods use
       `Timer::timer0().every_ms(cpu_hz, ms)`, which counts overflows for you.
```

The same program, [`examples/blink-rs`](examples/blink-rs/) (Rust + HAL,
**269 bytes**), against [`examples/blink`](examples/blink/) (pure assembly, 152
bytes). Peripheral setup shrinks from sixty-odd lines of assembly to **three
lines**:

```rust
let console = Console::install(&mut app, CPU_HZ, 9600)   // serial + send-byte/send-string subroutines
    .with_hex(&mut app);                                 // plus hexadecimal printing
let led = Gpio::active_low(P10);                          // pin + polarity
let tick = Timer::timer0().every_ms(CPU_HZ, 500);          // period 500ms
let counter = app.statics(1);                              // target-side variable

app.main(move |t| {
    console.begin(t);            // Timer 1 mode 2 + TH1 + SCON + PCON + TR1
    tick.begin(t);               // read-modify-write TMOD + load reload + SETB TR0
    t.mov_direct_imm(counter, 0);

    t.forever(|t| {
        tick.wait(t);            // wait 500ms: poll TF0 + clear flag + reload, in one step
        led.toggle(t);
        console.print(t, "t=");
        t.mov_a_direct(counter);
        console.print_hex8(t);   // A → two hex digits
        console.println(t, "");
        t.inc_direct(counter);
    });
});
```

`Console`'s subroutines are **generated on demand** — `with_hex` adds only
hexadecimal (about 25 bytes), `with_dec` adds only decimal (about 45 bytes), and
using neither costs just 20 bytes. 8051 flash should not be spent on library
functions nobody calls.

The HAL hides nothing: what it generates is exactly the assembly above, readable
at any time in `build/*.asm`.

```bash
mcs51 build examples/led-flow-rs           # 185 bytes (LED chaser + breathing LED)
mcs51 build examples/blink-rs              # 269 bytes (blink + serial counter)
mcs51 run   examples/blink-rs              # t=00 t=01 ... show up on the serial port
mcs51 flash examples/blink-rs -p /dev/ttyUSB0
```

The generated assembly (`build/*.asm`, readable and traceable):

```asm
        .org  0x0000
        LJMP  __main                 ; reset vector
        .org  0x0003
        RETI                         ; interrupt vector table
        ...
        .org  0x0030
__main:
        MOV   SP, #0x30
        MOV   0x40, #0                  ; breathing LED phase
        MOV   0x41, #0                  ; duty cycle
        ...
__led_0_forever:
        CLR   P35
        MOV   A, #250                ; busy-wait 250ms
        LCALL __delay_ms
        SETB  P35
        SJMP  __led_0_forever
```

**What you gain**: a real cargo project, rustc type checking, IDE completion and
go-to-definition, pins as compile-time constants (a typo in `P35` is a compile
error), and `App::statics()` working out your internal RAM allocation for you
(an overflow is a build error, not a runtime memory stomp).

**The cost**: what runs on the chip is still 8051 machine code, not Rust; loops
on the target must be written as `t.forever(...)`, because Rust's `for`/`while`
unroll on the host.

> **Why can't we just `rustc --target mcs51`?**
> LLVM has no MCS-51 backend (`rustc --print target-list` has no 8051; the only
> 8-bit platforms are `avr-none` and `msp430-none-elf`, and those exist because
> somebody wrote the backend). Writing an LLVM backend means supplying ISel,
> register allocation, stack frame layout and calling conventions, plus forking
> LLVM and maintaining your own Rust toolchain distribution.
> See [`docs/rust-on-8051.md`](docs/rust-on-8051.md) for the full breakdown.

---

## 6. Chip standard library (`mcs51-chips`)

Keil's `reg52.h` / `STC89C5xRC.h` are the de facto "chip standard library" of
the 8051 world; this crate is their Rust equivalent — **Chinese descriptions
included**:

```rust
use mcs51_chips::{reg52, stc89c5xrc, find};

reg52::TMOD.addr        // 0x89
reg52::TR0.addr         // 0x8C, the bit address is derived from the SFR, never hand-written
reg52::TF0.doc          // "定时器 0 溢出标志。功能及操作同 TF1"
stc89c5xrc::P45.doc     // "P4.5（与 ALE 复用；ISP 下载需勾选"ALE 脚用作 P4.5 口"）"

let chip = find("STC89C52RC").unwrap();
chip.flash_bytes         // 8192
chip.sfr_named("ISP_DATA").unwrap().addr   // 0xE2, follows the inheritance chain to the STC extension
```

**Scale and coverage** (merged view for `STC89C52RC`): **41 registers + 110
bits**.

**Vendor names now work directly in assembly too**:

```asm
        MOV   TMOD, #0x01        ; reg52
        SETB  TR0
        MOV   ISP_DATA, A        ; STC extension
        SETB  P45                ; STC's P4 port
```

The layering is this — the chip library only knows "which registers exist",
while the instruction set and code generation live in other crates:

| crate | what it owns |
|---|---|
| `mcs51-isa` | **instruction set**: opcodes, encodings, the generic SFR name table (used by the disassembler) |
| `mcs51-chips` | **a specific chip**: which registers this part has, what each bit is called, what the description says |
| `mcs51-codegen` | **code generation**: turning Rust descriptions into 8051 machine code |

Bit addresses are **never hand-written**: bit-addressable SFR addresses are all
multiples of 8 and ≥ 0x80, so the bit address is simply `SFR address + bit
index`. Derive it with `Bit::of` and getting the index wrong is impossible —
13 tests watch this identity, address collisions, whether names can go straight
into assembly, and a line-by-line check of the addresses in Keil's headers.

**Adding a new chip** takes four steps (see the module docs in
`crates/mcs51-chips/src/lib.rs`): create a module following `reg52.rs`, point
`base` at the base part, register it in `CHIPS`, run the tests.

---

## 7. Downloading to hardware

The generated Intel HEX can be burned with any 8051 programmer or STC-ISP; the
toolchain also ships `mcs51 flash`, which downloads straight to the BSL built
into STC chips over a serial port, with no external programmer required.

```
$ mcs51 devices                            # list serial ports
$ mcs51 flash examples/blink --simulate    # exercise the whole download path without hardware
$ mcs51 flash examples/blink -p /dev/ttyUSB0 -r dtr
```

`mcs51-flash` **reproduces the STC ISP protocol command by command** after the
open-source implementation [`stcflash`](https://github.com/cx693/StcFlash)
(a fork of `stcgal`):

| Stage | STC89/90 | STC12/15/8 |
|---|---|---|
| parity / packet checksum | none / 1 byte | **even parity** / **2 bytes** |
| framing | `46 B9 6A`…`16` (chip direction `68`) | same |
| baud switch | try `8F` → switch `8E` → 4× `80` ping-pong | start `50` → try `8F` → switch `8E` (replies `84`) |
| erase | `84` + block count + `33×6` | `84` + length + inverted magic guard |
| write block | 128 bytes/block, answer `80`+checksum | first block `22`, rest `02`, answer `02 54` |
| finalize / options / run | — / `8D` / `82` | BSL≥7.2 adds a `07` finalize / `8D` / `82` |

The clock is not guessed: the chip reports in its status packet "how many of its
own clocks make up one bit time", 8 samples are taken to derive `Fosc`, and the
BRT register is computed from that. `mcs51 probe` prints the model, clock, BSL
version and option (fuse) bytes it reads back.

**How it is verified**: the protocol code only talks to a `Transport` trait,
which has three implementations — a real serial port, an in-memory loopback, and
one **wired to real 8051 firmware inside the simulator**. So `tests/flash.rs`
contains a test that **builds 1000+ bytes of real firmware with this project's
own toolchain, runs the complete download flow, and then requires the bytes that
landed in "flash" to match the image byte for byte**.

Wiring, power-up, protocol details and a troubleshooting table are in
[`docs/flashing.md`](docs/flashing.md).

> **Verified on real hardware**: tested on an **STC89C52RC** (BSL 6.6C,
> 11.0592 MHz crystal, CH340 adapter) — 1063 bytes downloaded in 5.7 seconds,
> and the heartbeat read back over serial afterwards ticks **exactly one line
> per second** (`mcs51-rs rust app, uptime N s`), with the option bytes written
> back unchanged after being read. See [`docs/flashing.md`](docs/flashing.md).
>
> The STC12/15/8 branch is verified in the simulator only and has never been on
> real hardware.
>
> Also note that the STC89 family **has no IAP**, so "an application upgrading
> itself" is impossible on these parts and the built-in BSL is the only way in;
> only IAP-capable models (STC12/15/8 IAP series) can do it.

---

## 8. Toolchain details

### 8.1 Instruction set `mcs51-isa`

* The complete 8051 opcode table: **111 opcode families / 255 defined opcodes**, with only `0xA5` undefined
* Every opcode records: operand shape, encoding positions, machine cycles, PSW flags affected
* `encode()` / `decode()` use fixed-size stack buffers; `#![no_std]`, `#![forbid(unsafe_code)]`
* Address arithmetic follows the hardware rules:
  * `rel` for `SJMP/JC/DJNZ…` is relative to the **end of the instruction**
  * the target of `AJMP/ACALL` must fall in the **2 KiB page containing the next instruction** (checked at encode time)

### 8.2 Assembler `mcs51-asm`

* two-pass (iterating to convergence) assembly with forward references
* instructions/directives: `.area .org .equ .set .bit .sfr .db .byte .dw .word .ds .space
  .align .globl .extern .entry .include .end`
* expressions: `+ - * / % << >> & | ^ ~ ( )`, `$` (current address), `'A'`, `low()`/`high()`,
  and three numeric syntaxes `0x1F` / `1Fh` / `1010b`
* local labels `.loop:` are automatically scoped to the nearest global label
* relocation types: `ABS8 / HIGH8 / ABS16 / ADDR11 / REL8`
* **relative jumps within a section are resolved at assembly time** (the
  displacement does not depend on where the section finally lands), reducing
  linker work
* built-in 8051 device symbol table: `MOV SP,#0x30` and `SETB TR0` work without any `.include`
* diagnostics carry file name, line, column and a source excerpt

### 8.3 Linker `mcs51-ld`

The linker script is a line-oriented format:

```
memory CODE  0x0000 0x8000
memory DATA  0x0030 0x50
memory XDATA xdata 0x0000 0x8000

section HOME  -> CODE
section CODE  -> CODE
section DATA  -> DATA
section XDATA -> XDATA

entry start
checksum 0x7FFE
```

* input sections are placed in script order, with `.org` inside a section pinning an address
* region overflow, address overlap, undefined symbols, `REL8` out of range and `ADDR11` crossing a page all error out
* output: CODE image (Intel HEX/BIN), XDATA/IDATA data image, MAP file, optional checksum byte

### 8.4 Simulator `mcs51-sim`

The CPU core comes from the separate [`i8051`](https://crates.io/crates/i8051)
crate; this project provides everything around it:

* the SFR block (all `0x80..0xFF` reads and writes go through a `PortMapper`)
* Timer 0/1: mode 0 (13-bit), mode 1 (16-bit), mode 2 (8-bit auto-reload), gating and counter mode
* UART: modes 0–3 decoding, `TI`/`RI` semantics, receive/transmit FIFOs, configurable cycles per byte
* port latches and external pins, `PCON` idle/power-down modes, wake-up on interrupt
* **cycle-accurate per instruction** (cycle counts come straight from `mcs51-isa`'s opcode table)
* trace ring, breakpoint-style `run_until` / `run_until_console` / `run_cycles`, snapshots

### 8.5 Build system `mcs51-build`

`mcs51.toml` → assemble → link → `.ihx` / `.bin` / `.lst` / `.map`, then print a
size report. It also works without a manifest, defaulting the source file to
`src/main.asm`.

---

## 9. How correctness is guaranteed

This is the most important part of the whole project: **a hand-written 255-entry
opcode table is a disaster if a single cell is wrong**.

### 9.1 Cross-validation against an independent simulator (exhaustive oracle)

The `i8051` crate ships an **independently written** MCS-51 opcode table and
decoder. `crates/mcs51-isa/tests/oracle.rs` uses it as an oracle:

* it walks **all 256 opcodes × 6×6 groups of operand bytes = 9216 combinations**
* for each combination, `i8051` and `mcs51-isa` decode it independently and must
  agree **item by item on mnemonic, operand values and instruction length**
* it requires **exactly 255 defined opcodes**, with only `0xA5` undefined
* for every decodable combination it requires **re-encoding to produce identical bytes** (encode/decode round trip)
* it verifies that relative and page-internal jump targets agree with the oracle

### 9.2 Assembler ↔ simulator round trip

* in `crates/mcs51-asm/tests/assemble.rs`, the opcode table is walked and
  **every instruction** is rendered as assembly text, reassembled, and the whole
  byte stream must then decode back consistently under both `mcs51-isa` and `i8051`
* every assembled instruction is re-checked for semantic agreement with the `i8051` decoder

### 9.3 End-to-end functional verification

`crates/mcs51-build/tests/examples.rs` assembles the **real sources** in
`examples/`, runs them in the simulator and asserts on behaviour:

* `led-flow`: the lighting order must be 5 → 6 → 7 and repeat; each LED stays on
  for about 250 ms; **P3.0/P3.1 stay high throughout** (those are the serial
  download pins — pull them low and the next download will not get in)

`crates/mcs51-codegen/tests/` verifies the behaviour of the firmware generated
on the Rust side (a subroutine is emitted only once, the breathing LED's duty
cycle really goes from dark to bright and back, and `App::statics` never lands
in the SFR region).

`crates/mcs51-flash/tests/flash.rs` pushes a real firmware image through the
**simulated STC bootloader** and then compares what lands in flash byte for byte.

### 9.4 Real bugs found by the tests

None of these bugs is visible by reading the code; every one of them was forced
out by **behavioural assertions**. They are listed here because they say more
about the value of the verification strategy than any "test coverage" number.

| # | Problem | Symptom | How it was found |
|---|---|---|---|
| 1 | `OpDef::len()` **summed** operand lengths instead of taking the furthest end position | every 3-byte instruction had the wrong length | oracle round trip |
| 2 | re-entering `.area` **zeroed** the location counter | code overwrote itself after switching sections several times in one file | PC traced instruction by instruction |
| 3 | `.dw` table entries were read little-endian (8051 `MOV DPTR` is big-endian) | high and low bytes of 16-bit addresses swapped, jumping to 0x3303 | jump targets traced |
| 5 | the simulator's `port_index` matched P0‑P3 with a **range pattern** | `PCON` was read as the P0 latch and returned 0xFF | the trace showed `reason=PowerDown` |
| 13 | the serial baud reload value **truncated** instead of rounding | at 12MHz, 9600 actually ran at 10417 (+8.5%); at 11.0592MHz, 19200 was worse still at 14400 (+25%), and the real chip's serial output was pure garbage | hand-computing a baud table while writing the example docs, and finding it disagreed with the code |
| 14 | frame parsing used `payload = total - 3 - ck`, correct is `total - 4 - ck` | every chip reply timed out and downloads never worked at all | byte-by-byte comparison against the simulated BSL |
| 15 | the assembler's string escapes did not support `\xNN` and treated `\x0D` as the literal text `x0D` | newlines emitted by the Rust generator printed over serial as the three characters `x0D` | checking the serial output byte by byte while writing the example docs |
| 16 | `Timer::millis(500ms)` exceeded the 16-bit timer range and the code **silently clamped** it to 71ms | the LED blinked at 7Hz and serial printed 14 lines a second, but since "there was output" it looked fine; only burning it onto real hardware revealed the problem | found by hand-checking the arithmetic: 460800 > 65536. The old test only asserted "there is output", which cannot catch a wrong period — now it asserts `TH0/TL0` in the machine code directly |

Item 14 is especially typical: it only happens when "the chip's reply packet
happens to straddle the length field", corrupting 1 byte in 300. Without
byte-level output assertions this bug would have stayed dormant forever.

## 10. Directory layout

```
.
├── Cargo.toml                workspace
├── cargo-local.sh            build wrapper for sandboxed environments (CARGO_HOME/CARGO_TARGET_DIR)
├── crates/
│   ├── mcs51-isa/            instruction set: opcode table, encode/decode, SFRs, memory model (no_std)
│   ├── mcs51-asm/            assembler + built-in 8051.inc
│   ├── mcs51-ld/             linker + linker scripts
│   ├── mcs51-ihex/           Intel HEX / BIN / sparse images
│   ├── mcs51-sim/            system simulator (Timer / UART / ports)
│   ├── mcs51-chips/          chip standard library: SFR/bit definitions (Rust port of reg52.h, STC headers)
│   ├── mcs51-codegen/        write applications in Rust (eDSL + peripheral HAL → 8051 code generation)
│   ├── mcs51-flash/          STC BSL serial download protocol + simulated chip
│   └── mcs51-build/          the `mcs51` CLI driver
├── examples/
│   ├── blink/                bare-metal assembly: toggle a pin + poll serial counter (152 bytes)
│   ├── blink-rs/             **the same program written in Rust + HAL** (269 bytes)
│   ├── led-flow/             bare-metal assembly: three-way LED chaser (93 bytes)
│   └── led-flow-rs/          **Rust bare metal**: LED chaser + Timer 0 breathing LED (185 bytes)
└── docs/
    ├── architecture.md       architecture and verification method
    ├── flashing.md           downloading to hardware: wiring, protocol, troubleshooting
    └── rust-on-8051.md       why Rust cannot compile straight to the 8051, and three possible routes
```

Size: about **15 000 lines of Rust + 900 lines of 8051 assembly**, **172 tests**.

---

## 11. Relationship to real hardware

The generated Intel HEX can be **burned directly** onto 8051-family parts
(STC89, AT89C51, P89V51, …). The prerequisite is changing `cpu_hz` /
`uart_baud` in `mcs51.toml` to your board's actual values — those two decide the
Timer0 tick reload value and the Timer1 baud reload value.

Known differences between the simulator and real hardware (all noted in the docs):

* the pin levels for the external interrupts `INT0/INT1` are driven by `Peripherals::set_pins` and default high
* Timer 2 (8052) and the PCA are not implemented
* the RD/WR timing details of `MOVX` are not implemented; XDATA is zero-wait-state memory
* wake-up from idle mode is modelled as "check the interrupt flags once per machine cycle"

The download side has been verified on a real STC89C52RC (BSL 6.6C / CH340 /
115200); the STC12/15/8 branch, the frequency-calibration option bytes of the
STC15/8 and the USB-direct models are still unverified on hardware.
See [`docs/flashing.md`](docs/flashing.md).

---

## 12. License

MIT OR Apache-2.0.
The [`i8051`](https://crates.io/crates/i8051) crate used for instruction set
cross-validation is MIT OR Apache-2.0 and is used only as a `dev-dependency`.
