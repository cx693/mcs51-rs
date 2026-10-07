# 流水灯 + 呼吸灯（Rust 裸机版）—— STC89C52RC

* **P3.5 / P3.6**：轮流点亮的流水灯，每只 250ms；
* **P3.7**：用 **Timer 0** 做软件 PWM 的**呼吸灯**，约 6 秒一个来回。

应用代码是 **Rust**（`src/main.rs`），Rust 程序在构建期生成 8051 机器码。

**180 字节，不用内核、不用中断** —— 主循环轮询 `TF0`。

```
src/main.rs  ──►  mcs51-codegen 生成 8051 汇编  ──►  mcs51-asm  ──►  mcs51-ld  ──►  .ihx
（真正的 Rust）      （类型安全 API）              （汇编器）      （链接器）     （烧录）
```

> **同一个 Timer 0 怎么能同时做 PWM 和流水灯？**
> 主循环每一个 tick（0.2ms）推进一次 PWM 相位驱动 P3.7，
> 每 48 个 tick（10ms）推一格占空比，每 1200 个 tick（250ms）换一只流水灯。
> 一个时基，三件事 —— 8051 上只有一个 Timer 0，必须这么分。
>
> **想要汇编版？** [`../led-flow/`](../led-flow/) 是同一套灯的汇编实现（93 字节）。

---

## 0. 先说清楚：这不是「Rust 跑在 8051 上」

`rustc` 依赖 LLVM，而 **LLVM 没有 MCS-51 后端**：

```
$ rustc --print target-list | grep -i 8051
  （无）
```

8 位平台里只有 `avr-none` 和 `msp430-none-elf` 有 LLVM 后端 —— 因为有人给它们写了后端。
MCS-51 至今没有，GCC 也从来没有合并过。SDCC 之所以存在，正是因为通用编译器
在这个架构上做不好。

所以这个程序在**主机上运行**，在构建期生成 8051 机器码。但**工程结构是真的**：

| 你得到的 | 说明 |
|---|---|
| ✅ 真正的 cargo 工程 | `cargo build` / `cargo test` / rustc 类型检查 / IDE 补全跳转 |
| ✅ 引脚是编译期常量 | `P35` 拼错是**编译错误**，不是烧进去才发现 |
| ✅ 普通 Rust 模块 | `board.rs` / `leds.rs`，换板子只改一个文件 |
| ✅ 目标机子程序复用 | `app.routine(...)` 只生成一份代码，调用点发 `LCALL` |
| ✅ 生成的汇编可读 | 产物里的 `.asm`，随时能查 Rust 到底生成了什么 |
| ⚠️ 目标机循环要显式写 | `t.forever(...)`，因为 Rust 的 `for`/`while` 在**主机上**展开 |
| ⚠️ 跑在芯片上的是 8051 机器码 | 不是 Rust 运行时 |

---

## 1. 硬件接线

```
     VCC ──►|── LED1 ──[330Ω]── P3.5
     VCC ──►|── LED2 ──[330Ω]── P3.6
     VCC ──►|── LED3 ──[330Ω]── P3.7

     USB-TTL TXD ── P3.0 (RXD)      ← 下载用（裸机版不往串口输出）
     USB-TTL RXD ── P3.1 (TXD)
     USB-TTL GND ── GND
     USB-TTL VCC ── VCC (5V)
     USB-TTL DTR ──┬─ 100nF ── RST   （可选，自动复位）
                   └─ 10kΩ  ── VCC
```

**为什么是 P3.5 / P3.6 / P3.7**：

| 引脚 | 第二功能 | 能不能用 |
|---|---|---|
| P3.0 / P3.1 | RXD / TXD | **不行** —— 下载要用 |
| P3.2 / P3.3 | INT0 / INT1 | 可以，但留给中断 |
| P3.4 | T0 计数输入 | 可以，但留给计数 |
| **P3.5** | T1 | 不用定时器 1 时是干净 IO |
| **P3.6** | WR | 不接外部 RAM 时是干净 IO |
| **P3.7** | RD | 不接外部 RAM 时是干净 IO |

生成器用的是 `SETB P35` / `CLR P35` 这种**位操作**，不是 `MOV P3,#imm` ——
位操作只动目标引脚，不会把 P3.0/P3.1 拉低。有测试专门盯这一点。

---

## 2. 构建、下载

```bash
mcs51 build examples/led-flow-rs                  # 编译（110 字节）
mcs51 flash examples/led-flow-rs --simulate       # 自检下载链路
mcs51 flash examples/led-flow-rs -p /dev/ttyUSB0  # 真实下载
#   ↑ 看到 "现在给板子断电，再重新上电" 时把板子断电再上电
```

产物在 `examples/led-flow-rs/build/`：

| 文件 | 说明 |
|---|---|
| `led-flow-rs.ihx` | Intel HEX，下载用 |
| `led-flow-rs.bin` | 原始二进制 |
| `led-flow-rs.map` | 链接 MAP |
| `led-flow-rs.lst` | 汇编清单 |
| **`led-flow-rs.asm`** | **Rust 生成的目标机源码** —— 出问题先看这个 |

---

## 3. 工程结构：`main.rs` + 模块

应用入口就是 **`src/main.rs`**（Cargo 默认约定，没有 `lib.rs` 中转）：

```
examples/led-flow-rs/
├── Cargo.toml
├── mcs51.toml              ← 板子/下载参数
└── src/
    ├── main.rs             ← 入口 + Timer 0 主循环
    ├── board.rs            ← 硬件参数（晶振/时基/PWM/引脚）集中在这
    │                          寄存器地址来自芯片库 `mcs51-chips`，不手写
    ├── breath.rs           ← 呼吸灯模块（软件 PWM）
    └── leds.rs             ← 流水灯模块（含目标机子程序）
```

`main.rs` 不写任何具体指令：

```rust
mod board;
mod leds;

pub fn bare() -> Firmware {
    let mut app = App::bare_metal(board::board("led-flow-rs"));
    leds::chase(&mut app);          // 装成主程序
    app.finish().expect("生成裸机固件失败")
}
```

`board.rs` 把所有硬件数字收在一处 —— 换板子只改这个文件：

```rust
pub const CRYSTAL_HZ: u32 = 11_059_200;   // 11.0592MHz 是 8051 的标准晶振
pub const TICK_CYCLES: u32 = 184;         // Timer 0 每约 0.2ms 溢出一次
pub const PWM_PERIOD: u8 = 50;            // 50 个 tick = 10ms，即 100Hz
pub const BREATH_PIN: Bit = P37;          // 呼吸灯
pub const CHASE_LEDS: [Bit; 2] = [P35, P36];   // 顺序就是流水顺序
pub const DWELL_MS: u32 = 250;
```

`leds.rs` 是一个自洽的模块，对外只暴露两样东西：

```rust
/// 目标机子程序：熄灭全部 LED。所有调用点共用同一份机器码。
pub fn all_off(app: &mut App) -> Routine {
    app.routine("leds_all_off", |t| {
        for pin in LEDS { t.setb(pin); }
    })
}

/// 把流水灯装成裸机工程的主程序。
pub fn chase(app: &mut App) {
    let off = all_off(app);
    app.main(move |t| {
        t.call_routine(&off);          // LCALL，不是内联
        t.forever(|t| {
            for pin in LEDS { t.clr(pin); t.delay_ms(DWELL_MS); t.setb(pin); }
        });
    });
}
```

---

## 4. 生成的代码长什么样

裸机版的**全部**机器码就是这些（110 字节）：

```asm
        .org  0x0000
        LJMP  __main                 ; 复位向量
        .org  0x0003
        RETI                         ; 中断向量表：全部填 RETI，防跑飞
        ...                          ; INT0/Timer0/INT1/Timer1/串口/Timer2

        .org  0x0030
__main:
        MOV   SP, #0x30              ; 复位后 SP=0x07，必须重设
        LCALL __rt_leds_all_off
____main_0_forever:
        CLR   P35
        MOV   A, #250                ; 忙等 250ms
        LCALL __delay_ms
        SETB  P35
        CLR   P36
        MOV   A, #250
        LCALL __delay_ms
        SETB  P36
        CLR   P37
        MOV   A, #250
        LCALL __delay_ms
        SETB  P37
        SJMP  ____main_0_forever
__main_exit:
        SJMP  __main_exit            ; 主程序返回：停在这里

__rt_leds_all_off:
        SETB  P35
        SETB  P36
        SETB  P37
        RET
__delay_ms:                          ; 按 11.0592MHz 标定，1ms ≈ 921 机器周期
        ...
```

注意 `MOV A, #250` —— 毫秒数直接是延时子程序的入参，**不需要**在目标机上换算。

---

## 5. 关键约束：闭包体要用花括号

```rust
t.forever(|t| t.clr(P35));        // ❌ 编译错误
t.forever(|t| { t.clr(P35); });   // ✅
t.forever(|t| { t.clr(P35).setb(P36); });  // ✅ 链式也能用
```

原因：链式调用返回 `&mut Task`，而闭包需要返回 `()`，Rust 不会自动丢弃这个值。
这是编译期就报的错，不会跑到芯片上才出问题。

---

## 6. 什么是「目标机子程序」，为什么要它

| | Rust 普通函数 | `Routine` |
|---|---|---|
| 何时跑 | **构建期**（主机） | **运行期**（芯片） |
| 代码 | 每次调用**内联一份** | 全工程**只有一份** |
| 调用点 | 无 | `LCALL __rt_xxx` |

```rust
// ❌ 调用 3 次就有 3 份机器码
fn all_off(t: &mut Task) { for pin in LEDS { t.setb(pin); } }

// ✅ 目标机上只有一份
let all_off = app.routine("leds_all_off", |t| { for pin in LEDS { t.setb(pin); } });
t.call_routine(&all_off);
```

本工程只有一个调用点，差别不明显；但驱动、协议栈、公共算法这类会被多处调用的
代码必须用子程序，否则 Flash 会被重复代码吃光。
`crates/mcs51-codegen/tests/routines.rs` 有测试量这件事。

---

## 7. 改点什么

### 改流水速度

`src/board.rs` 里的 `DWELL_MS`。改成 `100` 就是每只 100ms。

`delay_ms` 单次上限 255ms；要更久就多调几次。

### 改成来回跑（跑马灯）

```rust
pub const LEDS: [Pin; 4] = [P35, P36, P37, P36];   // 5→6→7→6→5
```

因为 `LEDS` 只是主机侧的数据，改数组就改了生成的代码。

### 换成别的引脚

改 `src/board.rs` 的 `LEDS` 数组即可，`8051.inc` 里定义了 `P0_0`~`P37` 全部 32 个位名。

### 用原生汇编指令

eDSL 还没包装的指令可以直接发：

```rust
t.raw("MOV  A, #0x42");
t.raw_commented("读一下定时器 0 高字节", "MOV  A, TH0");
```

---

## 8. 测试

`cargo test -p led-flow-rs` 会**真的执行 `src/main.rs`**，读回它写出的 `.ihx`，
装进模拟器断言行为 —— 测的就是 `mcs51 flash` 会烧进芯片的那份东西：

| 测试 | 断言什么 |
|---|---|
| `the_chaser_alternates_every_quarter_second` | P3.5/P3.6 必须 5→6→5→6，间隔约 250ms |
| `the_breathing_led_ramps_up_and_down` | P3.7 的占空比必须 0 → 1 → 0（暗→亮→暗） |
| `the_two_leds_never_interfere` | 任何时刻最多一只流水灯亮 |
| `the_serial_pins_are_never_pulled_low` | 全程 P3.0/P3.1 保持高电平 |
| `the_bare_firmware_is_small_and_uses_timer0_without_interrupts` | 无内核符号、真的用了 Timer 0、没装中断处理程序 |

---

## 9. 排查

| 现象 | 原因 / 处理 |
|---|---|
| 灯不亮 | ① LED 极性接反（长脚朝 VCC）；② 限流电阻/杜邦线接触不良；③ 看 `mcs51 probe` 的选项字节，确认没配成外部晶振 |
| 只有一个灯常亮 | 另外两只的限流电阻或接线有问题 |
| 灯亮但很暗 | 限流电阻太大（换 330Ω），或 IO 被别的东西拉低 |
| 下载后不跑 | 用 `mcs51 probe` 看选项字节；确认 12 时钟模式（bit0 = 1） |
| `error: expected (), found &mut Task` | 闭包体忘了花括号，见第 5 节 |
| `delay_ms(N) 超过单次上限 255ms` | 拆成多次调用 |
| `子程序 xxx 被定义了两次` | 同一个 `app.routine("同名", ...)` 调了两次 |

晶振是多少都无所谓 —— 裸机版不用串口，下载也不受影响（STC 的 BSL 会自己
测量真实主频）。当前按 11.0592MHz 标定忙等延时，换晶振延时会有同比例偏差。
