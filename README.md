# mcs51-rs — 完全用 Rust 实现的 8051 裸机工具链

`mcs51-rs` 是一套**完全用 Rust 编写**的 Intel MCS-51（8051）工具链。
它把 SDCC 工具链里跟 8051 后端相关的部分（汇编器 `sdas8051`、链接器 `sdld`、
`packihx`/`makebin`、`ucsim`）用 Rust 重写，再加上一个芯片标准库、
一个 Rust → 8051 的代码生成器，以及 STC 串口下载协议。

**生成的是纯裸机固件**：复位向量 + 中断向量表 + 你的代码。
没有调度器、没有系统节拍、没有运行时库 —— 8051 上那点资源不值得养一个内核。

**零外部依赖**：不需要 SDCC、不需要厂商汇编器、不需要硬件。汇编 → 链接 → Intel HEX →
在模拟器里跑起来验证，整条链路都在一个 `cargo test` 里闭环。

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

## 1. 为什么不是「Rust 直接编译到 8051」

Rust 目前没有 8051 后端（LLVM 没有 MCS-51 target），所以「用 Rust 生成 8051 固件」
唯一可行的路线是：**用 Rust 实现整条 8051 后端工具链，再让 Rust 在构建期生成 8051 代码**。

本项目正是这条路线：

| SDCC 组件 | 本项目的 Rust 实现 | 说明 |
|---|---|---|
| `sdas8051`（汇编器） | `mcs51-asm` | 两遍汇编、表达式、重定位目标文件 |
| `sdld`（链接器）+ `.lkr` | `mcs51-ld` | 链接脚本、符号解析、重定位、MAP 文件、校验和 |
| `packihx` / `makebin` / `sdobjcopy` | `mcs51-ihex` | Intel HEX / 二进制 / 稀疏镜像 |
| `ucsim`（模拟器） | `mcs51-sim` | 基于 `i8051` CPU 核 + Timer0/1、UART、端口 |
| `Makefile` / `sdcc` 驱动 | `mcs51-build`（`mcs51` 命令行） | 工程清单、构建、尺寸报告、下载 |
| STC-ISP.exe / `stcgal` / `stcflash` | `mcs51-flash` | STC BSL 串口下载协议，Rust 复现 |
| 手写汇编 / SDCC 的 C | `mcs51-codegen` | **用 Rust 写应用**，构建期生成 8051 机器码；带 GPIO/定时器/串口 HAL |
| Keil `reg52.h` / `STC89C5xRC.h` | `mcs51-chips` | **芯片标准库**：SFR 与位的定义，带中文说明 |

指令集本身（`mcs51-isa`）没有对应的 SDCC 组件，它是整条链路的**单一事实来源**：
汇编器从它编码，反汇编器从它解码，模拟器从它取周期数，尺寸报告从它取元数据。
并且它是 `#![no_std]` 的——同一份代码既能在主机上汇编固件，也能在目标机上运行时反汇编。

---

## 2. 快速开始

### 2.1 构建工具链

```bash
git clone <this repo> mcs51-rs && cd mcs51-rs
cargo build --release
cargo test --workspace          # 174 个测试
```

> 当前工作区用 `cargo-local.sh` 封装了构建（因为本机环境的 `CARGO_HOME` /
> `CARGO_TARGET_DIR` 指向工作区外）。正常环境直接 `cargo build` 即可。

### 2.2 命令行

```
mcs51 new <dir>                  脚手架：生成 mcs51.toml + src/main.asm
mcs51 build [dir]                汇编 → 链接 → 打包（.ihx/.bin/.lst/.map）
mcs51 run [dir]                  构建后在模拟器里运行，打印串口输出与任务状态
mcs51 flash [dir|file.ihx]       通过串口下载到真实芯片（STC BSL 协议）
mcs51 probe -p <PORT>            读芯片型号/主频/BSL 版本，不写入
mcs51 devices                    列出系统串口
mcs51 asm <file.asm>             单文件汇编并输出清单
mcs51 disasm <file>              反汇编 HEX / BIN
mcs51 size <file>                镜像尺寸报告
mcs51 hex2bin / bin2hex          格式转换
mcs51 hexdump <file>             十六进制转储
mcs51 isa [opcode]               查询指令集（操作码、长度、周期、影响标志位）
```

### 2.3 工程清单 `mcs51.toml`

```toml
[project]
name = "blink"
device = "AT89C51"

[build]
sources = ["src/main.asm"]
out = "build"

[sim]
instructions = 400000
cpu_hz = 11059200     # 模拟器按它算机器周期和串口位宽
uart_baud = 9600

[flash]
protocol = "stc89"

[link]
checksum = 0x7FFE     # 可选：镜像校验和落在这个地址
```

`[sim]` 只影响模拟器怎么跑固件——固件本身不读这些值，所以改了不会影响生成结果。

---

## 3. 裸机程序长什么样

没有内核，程序结构就是最原始的 8051 样子：

```asm
        .area CODE (ABS)
        .org  0x0000
        LJMP  main                   ; 复位向量

        .org  0x0003                 ; 中断向量表：本工程不用中断，
        RETI                         ; 但必须填上，否则干扰触发时会跑飞到 0xFF
        .org  0x000B
        RETI                         ; Timer0
        ; ... 其余向量同理

        .org  0x0030
main:
        MOV   SP, #0x30              ; 复位后 SP=0x07，必须重设
loop:
        CPL   P1_0
        ACALL delay
        SJMP  loop

delay:  MOV   R7, #250
inner:  DJNZ  R7, inner
        RET
```

`mcs51 build` 什么都不替你决定：它是汇编器 + 链接器 + 打包器。
时序、栈、寄存器、中断，全在你自己手里。

**用 Rust 写也一样**，只是把指令换成方法调用（见 §5）：

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

## 4. 工程案例：STC89C52RC 流水灯

`examples/led-flow/` 是最适合**第一次上真芯片**的工程：三只 LED 接在
P3.5/P3.6/P3.7 上依次点亮，每只约 250ms，代码只有 **93 字节**。

```bash
mcs51 build examples/led-flow                  # 93 字节，0x0000..0x005C
mcs51 flash examples/led-flow -p /dev/ttyUSB0  # 看到提示时给板子断电重上电
```

接线（低电平点亮）：

```
VCC ──►|── LED1 ──[330Ω]── P3.5
VCC ──►|── LED2 ──[330Ω]── P3.6
VCC ──►|── LED3 ──[330Ω]── P3.7
USB-TTL TXD/RXD/GND/VCC ── P3.0/P3.1/GND/VCC
```

选 P3.5/6/7 是因为 P3.0/P3.1 是下载用的串口脚、P3.2/P3.3 是中断脚、
P3.4 是计数脚；而 P3.5(T1)/P3.6(WR)/P3.7(RD) 只要不用定时器和外部 RAM
就是干净的普通 IO。

接线、改法、排查都写在 [`examples/led-flow/README.md`](examples/led-flow/README.md) 里（中文）。

> **晶振这件事值得单独说**：8051 的串口波特率靠定时器整数分频，
> 不是每个波特率都能精确达到。11.0592 MHz 晶振能让 9600 精确整除（误差 0%），
> 12 MHz 晶振下 9600 只能到 10417（误差 8.5%）。
> 这个换算得你自己在代码里做（见 `examples/blink/src/main.asm` 的 `uart_init`）；
> 下载本身**不受影响** —— STC 的 BSL 会先测量芯片真实主频再反推波特率。

---

## 5. 用 Rust 写应用（`mcs51-codegen`）

应用代码可以写成 **Rust**，在构建期生成 8051 机器码：

应用入口就是 **`src/main.rs`**，逻辑拆进普通 Rust 模块
（`board.rs` / `leds.rs` / `console.rs`）：

```rust
// src/main.rs —— 只做组装，不写具体指令
mod board;
mod breath;
mod leds;

pub fn bare() -> Firmware {
    let mut app = App::new(board::board("led-flow-rs"));
    let breath = breath::Breath::new(app.statics(4));   // 目标机静态变量
    let chaser = leds::Chaser::new(app.statics(2));

    // 定时器：说"每 200 微秒溢出一次"，重载值在主机上算好
    let tick = Timer::timer0()
        .micros(CPU_HZ, 200)
        .compensate(16);

    app.main(move |t| {
        chaser.init(t);
        breath.init(t);
        tick.begin(t);                      // 读-改-写 TMOD + 装初值 + 启动
        t.forever(|t| { /* 主循环 */ });
    });
    app.finish().expect("生成固件失败")
}
```

```rust
// src/leds.rs —— 模块对外暴露一个目标机子程序和装配函数
pub fn all_off(app: &mut App) -> Routine {
    app.routine("leds_all_off", |t| {          // 目标机上只生成一份代码
        for pin in CHASE_LEDS { t.setb(pin); }
    })
}
```

**`Routine` 和 Rust 普通函数是两样东西**，这是真项目和大 demo 的分水岭：

| | Rust 普通函数 | `Routine` |
|---|---|---|
| 何时跑 | 构建期（主机） | 运行期（芯片） |
| 代码 | 每次调用**内联一份** | 全工程**只有一份** |
| 调用点 | 无 | `LCALL __rt_xxx` |

测过：内联版本 `SETB P3_5` 出现 3 次，子程序版本只出现 1 次且固件更小。

### 5.1 外设用 HAL 写，不必逐条配 SFR

8051 裸机代码里真正烦人的不是指令，而是**配寄存器的那堆琐事**：
`TMOD` 的高 4 位归 Timer 1、低 4 位归 Timer 0，改一个必须读-改-写；
波特率要按 `fosc / (384 * baud)` 反推 `TH1`；`PCON` 不可位寻址，只能整字节改；
16 位定时器最多数 65536 个机器周期（11.0592MHz 下约 71ms），想定 500ms
得自己数溢出次数。这些算术**在主机上算一次就够了**。

所以 `mcs51-codegen` 提供了一层薄薄的 HAL：

| 类型 | 替你做掉的事 |
|---|---|
| `Gpio` | 记住极性 —— 之后只写 `on()` / `off()` / `toggle()` |
| `Timer` | 微秒/毫秒 → 重载值；`TMOD` 读-改-写；`wait()` 把"等溢出+清标志+重装"合成一步 |
| `Ticker` | 周期超过定时器量程时自动拆成多次溢出，并**均分**使总时长准确 |
| `Console` | 波特率 → `TH1`；`SCON`/`PCON`/Timer 1 一次配好；打印子程序一次装好 |

**超量程周期是个陷阱**：`Timer::millis(cpu_hz, 500)` 在 11.0592MHz 下要 460800
个机器周期，装不下。早期版本会**静默钳位**成 71ms —— 程序照跑、串口照输出，
只是周期错了七倍。现在这是一条构建错误，会告诉你改用 `every_ms`：

```
error: Timer 周期要 460800 个机器周期，超过 16 位定时器的量程 65536。
       11.0592MHz 下一个机器周期约 1.085us，量程约 71ms。
       更长的周期用 `Timer::timer0().every_ms(cpu_hz, ms)`，它会自动数溢出次数。
```

同一个程序，[`examples/blink-rs`](examples/blink-rs/)（Rust + HAL，**269 字节**）
对着 [`examples/blink`](examples/blink/)（纯汇编，152 字节）。
外设配置从六十来行汇编变成**三行**：

```rust
let console = Console::install(&mut app, CPU_HZ, 9600)   // 串口 + 发字节/发串子程序
    .with_hex(&mut app);                                 // 再加十六进制打印
let led = Gpio::active_low(P10);                          // 引脚 + 极性
let tick = Timer::timer0().every_ms(CPU_HZ, 500);          // 周期 500ms
let counter = app.statics(1);                              // 目标机变量

app.main(move |t| {
    console.begin(t);            // Timer 1 模式 2 + TH1 + SCON + PCON + TR1
    tick.begin(t);               // 读-改-写 TMOD + 装初值 + SETB TR0
    t.mov_direct_imm(counter, 0);

    t.forever(|t| {
        tick.wait(t);            // 等 500ms：轮询 TF0 + 清标志 + 重装，一步到位
        led.toggle(t);
        console.print(t, "t=");
        t.mov_a_direct(counter);
        console.print_hex8(t);   // A → 两位十六进制
        console.println(t, "");
        t.inc_direct(counter);
    });
});
```

`Console` 的子程序是**按需生成**的 —— `with_hex` 只加十六进制（约 25 字节），
`with_dec` 只加十进制（约 45 字节），都不用就只占 20 字节。8051 的 Flash 不该
拿来装没人调用的库函数。

HAL 不隐藏任何东西：它生成的就是上面这种汇编，`build/*.asm` 里随时能读。

```bash
mcs51 build examples/led-flow-rs           # 185 字节（流水灯 + 呼吸灯）
mcs51 build examples/blink-rs              # 269 字节（blink + 串口报数）
mcs51 run   examples/blink-rs              # 串口上能看到 t=00 t=01 ...
mcs51 flash examples/blink-rs -p /dev/ttyUSB0
```

生成出来的汇编（`build/*.asm`，可读、可追溯）：

```asm
        .org  0x0000
        LJMP  __main                 ; 复位向量
        .org  0x0003
        RETI                         ; 中断向量表
        ...
        .org  0x0030
__main:
        MOV   SP, #0x30
        MOV   0x40, #0                  ; 呼吸灯相位
        MOV   0x41, #0                  ; 占空比
        ...
__led_0_forever:
        CLR   P35
        MOV   A, #250                ; 忙等 250ms
        LCALL __delay_ms
        SETB  P35
        SJMP  __led_0_forever
```

**得到的**：真正的 cargo 工程、rustc 类型检查、IDE 补全跳转、引脚是编译期常量
（`P35` 拼错是编译错误）、`App::statics()` 替你把内部 RAM 分配算好
（越界是构建错误，不是运行时踩内存）。

**代价**：跑在芯片上的仍然是 8051 机器码，不是 Rust；目标机的循环要写
`t.forever(...)`，因为 Rust 的 `for`/`while` 在主机上展开。

> **为什么不能直接 `rustc --target mcs51`？**
> LLVM 没有 MCS-51 后端（`rustc --print target-list` 里没有 8051；
> 8 位平台只有 `avr-none` 和 `msp430-none-elf`，因为有人写了后端）。
> 写一个 LLVM 后端要补 ISel / 寄存器分配 / 栈帧布局 / 调用约定，
> 还要 fork LLVM 并维护自己的 Rust 工具链发行版。
> 完整拆解见 [`docs/rust-on-8051.md`](docs/rust-on-8051.md)。

---

## 6. 芯片标准库（`mcs51-chips`）

Keil 的 `reg52.h` / `STC89C5xRC.h` 是 8051 世界事实上的「芯片标准库」，
这个 crate 是它们的 Rust 版 —— **连中文说明一起搬过来了**：

```rust
use mcs51_chips::{reg52, stc89c5xrc, find};

reg52::TMOD.addr        // 0x89
reg52::TR0.addr         // 0x8C，位地址从 SFR 推出来，不手写
reg52::TF0.doc          // "定时器 0 溢出标志。功能及操作同 TF1"
stc89c5xrc::P45.doc     // "P4.5（与 ALE 复用；ISP 下载需勾选"ALE 脚用作 P4.5 口"）"

let chip = find("STC89C52RC").unwrap();
chip.flash_bytes         // 8192
chip.sfr_named("ISP_DATA").unwrap().addr   // 0xE2，沿继承链找到 STC 扩展
```

**规模和覆盖**（`STC89C52RC` 的合并视图）：**41 个寄存器 + 110 个位**。

**汇编里现在也能直接写厂商名字**：

```asm
        MOV   TMOD, #0x01        ; reg52
        SETB  TR0
        MOV   ISP_DATA, A        ; STC 扩展
        SETB  P45                ; STC 的 P4 口
```

分层是这样的 —— 芯片库只管「有哪些寄存器」，指令集和代码生成在别的 crate：

| crate | 管什么 |
|---|---|
| `mcs51-isa` | **指令集**：操作码、编码、通用 SFR 名字表（给反汇编用） |
| `mcs51-chips` | **具体芯片**：这颗片子上有哪些寄存器、每个位叫什么、说明是什么 |
| `mcs51-codegen` | **代码生成**：把 Rust 描述变成 8051 机器码 |

位地址**不手写**：可位寻址的 SFR 地址都是 8 的倍数且 ≥ 0x80，位地址就是
`SFR 地址 + 位序号`。用 `Bit::of` 推出来，写错序号是不可能的 ——
13 条测试盯着这条恒等式、地址冲突、名字能不能直接进汇编、以及逐条核对
Keil 头文件里的地址。

**加一颗新芯片**只需要四步（见 `crates/mcs51-chips/src/lib.rs` 的模块文档）：
照 `reg52.rs` 新建一个模块、把 `base` 指向基础款、注册进 `CHIPS`、跑测试。

---

## 7. 下载到硬件

生成的 Intel HEX 可以直接用任何 8051 编程器或 STC-ISP 烧写；本工具链还自带
`mcs51 flash`，通过串口直接下载到 STC 芯片内置的 BSL，不需要外部编程器。

```
$ mcs51 devices                            # 看串口
$ mcs51 flash examples/blink --simulate    # 不接硬件自检整条下载链路
$ mcs51 flash examples/blink -p /dev/ttyUSB0 -r dtr
```

`mcs51-flash` 是按开源实现 [`stcflash`](https://github.com/cx693/StcFlash)
（`stcgal` 的二次开发）**逐条复现**的 STC ISP 协议：

| 环节 | STC89/90 | STC12/15/8 |
|---|---|---|
| 校验位 / 包校验和 | 无 / 1 字节 | **偶校验** / **2 字节** |
| 包头包尾 | `46 B9 6A`…`16`（芯片方向 `68`） | 同上 |
| 切波特率 | `8F` 试 → `8E` 切 → 4 次 `80` 乒乓 | `50` 起 → `8F` 试 → `8E` 切（回 `84`） |
| 擦除 | `84` + 块数 + `33×6` | `84` + 长度 + 倒数魔数守卫 |
| 写块 | 128 字节/块，回 `80`+校验和 | 首块 `22` 其余 `02`，回 `02 54` |
| 收尾 / 选项 / 运行 | — / `8D` / `82` | BSL≥7.2 加 `07` 收尾 / `8D` / `82` |

主频不是猜的：芯片在状态包里上报"一个位时间有几个自己的时钟"，取 8 个样本反推
`Fosc`，再据此算 BRT 寄存器。`mcs51 probe` 会把读到的型号、主频、BSL 版本、
选项(熔丝)字节都打出来。

**验证方式**：协议代码只跟 `Transport` trait 打交道，该 trait 有真实串口、
内存回环、以及**连到模拟器里真实 8051 固件**三种实现。所以
`tests/flash.rs` 里有一条测试会**用本项目自己的工具链编出 1000+ 字节的真实固件，
走完整下载流程，再要求落进"Flash"的字节和镜像逐字节相同**。

接线上电、协议细节、排查表见 [`docs/flashing.md`](docs/flashing.md)。

> **真机验证**：已在一块 **STC89C52RC**（BSL 6.6C，11.0592MHz 晶振，
> CH340 适配器）上实测通过 —— 1063 字节 / 5.7 秒下载完成，
> 烧完后串口读回的心跳**正好每秒一行**（`mcs51-rs rust app, uptime N s`），
> 选项字节读回原值写回未被改动。详见 [`docs/flashing.md`](docs/flashing.md)。
>
> STC12/15/8 那一支分支只有模拟器验证，没上过真机。
>
> 另外，STC89 系列**没有 IAP**，所以"应用自己升级自己"在这类芯片上做不到，
> 必须走内置 BSL；只有带 IAP 的型号（STC12/15/8 的 IAP 系列）才可以。

---

## 8. 工具链细节

### 8.1 指令集 `mcs51-isa`

* 完整 8051 操作码表：**111 个操作码族 / 255 个已定义操作码**，只有 `0xA5` 未定义
* 每个操作码记录：操作数形状、编码位置、机器周期数、PSW 影响标志
* `encode()` / `decode()` 使用定长栈缓冲，`#![no_std]`、`#![forbid(unsafe_code)]`
* 地址计算遵循硬件规则：
  * `SJMP/JC/DJNZ…` 的 `rel` 相对于**指令末尾**
  * `AJMP/ACALL` 的目标必须落在**下一条指令所在的 2 KiB 页**内（编码期校验）

### 8.2 汇编器 `mcs51-asm`

* 两遍（可迭代到收敛）汇编，支持前向引用
* 指令/伪指令：`.area .org .equ .set .bit .sfr .db .byte .dw .word .ds .space
  .align .globl .extern .entry .include .end`
* 表达式：`+ - * / % << >> & | ^ ~ ( )`、`$`（当前地址）、`'A'`、`low()`/`high()`、
  `0x1F` / `1Fh` / `1010b` 三种进制写法
* 局部标签 `.loop:` 自动按最近的全局标签作用域化
* 重定位类型：`ABS8 / HIGH8 / ABS16 / ADDR11 / REL8`
* **同段内相对跳转在汇编期就解析**（位移与段最终落址无关），减少链接器工作量
* 内建 8051 器件符号表：不写 `.include` 也能用 `MOV SP,#0x30`、`SETB TR0`
* 诊断带文件名、行号、列号与源码摘录

### 8.3 链接器 `mcs51-ld`

链接脚本是行式格式：

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

* 按脚本顺序放置各输入段，段内 `.org` 作为钉死地址
* 区域溢出、地址重叠、未定义符号、`REL8` 越界、`ADDR11` 跨页全部报错
* 输出：CODE 镜像（Intel HEX/BIN）、XDATA/IDATA 数据镜像、MAP 文件、可选校验和字节

### 8.4 模拟器 `mcs51-sim`

CPU 核来自独立的 [`i8051`](https://crates.io/crates/i8051) crate，本项目提供它周围的一切：

* SFR 块（通过 `PortMapper` 接管所有 `0x80..0xFF` 读写）
* Timer 0/1：模式 0（13 位）、模式 1（16 位）、模式 2（8 位自动重载）、门控与计数模式
* UART：模式 0–3 解码、`TI`/`RI` 语义、收发 FIFO、可配置每字节周期数
* 端口锁存器与外部引脚、`PCON` 空闲/掉电模式、中断唤醒
* **按指令精确计周期**（周期数直接来自 `mcs51-isa` 的操作码表）
* 跟踪环、断点式 `run_until` / `run_until_console` / `run_cycles`、快照

### 8.5 构建系统 `mcs51-build`

`mcs51.toml` → 汇编 → 链接 → `.ihx` / `.bin` / `.lst` / `.map`，并打印尺寸报告。
没有清单文件时也能直接用，源文件默认 `src/main.asm`。

---

## 9. 正确性是怎么保证的

这是整个项目最关键的部分：**手写的 255 项操作码表，只要错一格就是灾难**。

### 9.1 与独立模拟器交叉验证（exhaustive oracle）

`i8051` 这个 crate 自带一份**独立编写**的 MCS-51 操作码表和译码器。
`crates/mcs51-isa/tests/oracle.rs` 把它当作 oracle：

* 遍历 **所有 256 个操作码 × 6×6 组操作数字节 = 9216 个组合**
* 对每一种组合，用 `i8051` 和 `mcs51-isa` 分别译码，要求
  **助记符、操作数值、指令长度逐项一致**
* 要求**恰好 255 个已定义操作码**，只有 `0xA5` 未定义
* 对每个能译码的组合，要求**重新编码后字节完全一致**（encode/decode 往返）
* 验证相对跳转 / 页内跳转的目标地址与 oracle 一致

### 9.2 汇编器 ↔ 模拟器往返

* `crates/mcs51-asm/tests/assemble.rs` 中，遍历操作码表把**每一条指令**渲染成汇编源文本，
  重新汇编，再要求整个字节流能被 `mcs51-isa` 和 `i8051` 一致地译码回去
* 汇编出的每条指令都用 `i8051` 译码器复核语义一致

### 9.3 端到端功能验证

`crates/mcs51-build/tests/examples.rs` 直接汇编 `examples/` 里的**真实源码**，
在模拟器里跑，然后断言行为：

* `led-flow`：点灯顺序必须是 5 → 6 → 7 并循环；每只亮约 250ms；
  **全程 P3.0/P3.1 保持高电平**（那是下载用的串口脚，被拉低下次就下不进去）

`crates/mcs51-codegen/tests/` 验证 Rust 侧生成的固件行为
（子程序只生成一份、呼吸灯占空比真的从暗到亮再回来、
`App::statics` 绝不落在 SFR 区）。

`crates/mcs51-flash/tests/flash.rs` 把一份真实固件推过**模拟的 STC bootloader**，
再逐字节比对落进 flash 的内容。

### 9.4 测试驱动发现的真实 bug

这些 bug 单看代码都看不出来，全部是**行为断言**逼出来的。列在这里是因为它们
比任何"测试覆盖率"数字都更能说明验证策略的价值。

| # | 问题 | 现象 | 定位方式 |
|---|---|---|---|
| 1 | `OpDef::len()` 把各操作数长度**求和**而非取最远结束位置 | 所有 3 字节指令长度错误 | oracle 往返测试 |
| 2 | `.area` 二次进入把位置计数器**清零** | 一个文件里多次切段后代码互相覆盖 | 逐条跟踪 PC |
| 3 | `.dw` 表项按小端读取（8051 `MOV DPTR` 是大端） | 16 位地址高低字节颠倒，跳到 0x3303 | 追踪跳转落点 |
| 5 | 模拟器 `port_index` 用**区间模式**匹配 P0‑P3 | `PCON` 被当成 P0 锁存器读成 0xFF | 追踪显示 `reason=PowerDown` |
| 13 | 串口波特率重载值用**整除截断**而非四舍五入 | 12MHz 下 9600 实际跑到 10417（+8.5%）；11.0592MHz 下 19200 更离谱，跑到 14400（+25%），真机上串口全是乱码 | 写示例工程文档时手算波特率表，发现和自己写的代码对不上 |
| 14 | 帧解析里 `payload = total - 3 - ck`，正确是 `total - 4 - ck` | 所有芯片应答都超时，下载完全走不通 | 模拟 BSL 逐字节比对 |
| 15 | 汇编器的字符串转义不支持 `\xNN`，把 `\x0D` 当成字面文本 `x0D` | Rust 生成器输出的换行在串口里打成了 `x0D` 三个字符 | 写示例文档时逐字节核对串口输出 |
| 16 | `Timer::millis(500ms)` 超过 16 位定时器量程，代码里**静默钳位**成 71ms | 灯以 7Hz 闪、串口每秒刷 14 行，但"有输出"所以看着像正常；烧到真机上才发现不对 | 手工核对换算时发现 460800 > 65536。原来的测试只断言"有输出"，抓不到周期错 —— 现在直接断言机器码里的 `TH0/TL0` |

第 14 条尤其典型：它只在"芯片的应答包刚好跨过长度字段"时才会发生，
300 个字节里只坏 1 个字节。没有字节级的输出断言，这个 bug 会一直潜伏下去。

## 10. 目录结构

```
.
├── Cargo.toml                工作区
├── cargo-local.sh            沙箱内的构建封装（CARGO_HOME/CARGO_TARGET_DIR）
├── crates/
│   ├── mcs51-isa/            指令集：操作码表、编解码、SFR、内存模型（no_std）
│   ├── mcs51-asm/            汇编器 + 内建 8051.inc
│   ├── mcs51-ld/             链接器 + 链接脚本
│   ├── mcs51-ihex/           Intel HEX / BIN / 稀疏镜像
│   ├── mcs51-sim/            系统模拟器（Timer / UART / 端口）
│   ├── mcs51-chips/          芯片标准库：SFR/位定义（reg52.h、STC 头文件的 Rust 版）
│   ├── mcs51-codegen/        用 Rust 写应用（eDSL + 外设 HAL → 8051 代码生成）
│   ├── mcs51-flash/          STC BSL 串口下载协议 + 模拟芯片
│   └── mcs51-build/          `mcs51` 命令行驱动
├── examples/
│   ├── blink/                汇编裸机：翻引脚 + 轮询串口报数（152 字节）
│   ├── blink-rs/             **同一个程序用 Rust + HAL 写**（269 字节）
│   ├── led-flow/             汇编裸机：三路流水灯（93 字节）
│   └── led-flow-rs/          **Rust 裸机**：流水灯 + Timer 0 呼吸灯（185 字节）
└── docs/
    ├── architecture.md       架构与验证方法
    ├── flashing.md           下载到硬件：接线、协议、排查
    └── rust-on-8051.md       为什么 Rust 不能直接编到 8051，以及三条路线
```

规模：约 **15 000 行 Rust + 900 行 8051 汇编**，**172 个测试**。

---

## 11. 与真实硬件的关系

生成的 Intel HEX 可以**直接烧写**到 8051 系列单片机上（STC89、AT89C51、P89V51 等）。
前提是把 `mcs51.toml` 里的 `cpu_hz` / `uart_baud` 改成目标板实际值——这两个值决定了
Timer0 节拍重载值和 Timer1 波特率重载值。

模拟器与真实硬件的已知差异（都会在文档中标注）：

* 外部中断 `INT0/INT1` 的引脚电平由 `Peripherals::set_pins` 驱动，默认拉高
* 未实现 Timer 2（8052）与 PCA
* 未实现 `MOVX` 的 RD/WR 时序细节，XDATA 是零等待内存
* 空闲模式的唤醒按「每个机器周期检查一次中断标志」建模

下载侧已在一块 STC89C52RC 上实测通过（BSL 6.6C / CH340 / 115200）；
STC12/15/8 分支、STC15/8 的频率校准选项字节和 USB 直连型号仍未在真机验证。
详见 [`docs/flashing.md`](docs/flashing.md)。

---

## 12. 许可

MIT OR Apache-2.0。
指令集交叉验证用到的 [`i8051`](https://crates.io/crates/i8051) crate 为
MIT OR Apache-2.0，仅作为 `dev-dependency` 使用。
