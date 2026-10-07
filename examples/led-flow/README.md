# 流水灯 —— STC89C52RC 入门工程（裸机版）

> **真机验证**：同类固件已在一块 STC89C52RC（BSL 6.6C，11.0592MHz，CH340）上
> 实测烧录通过。本裸机版与 Rust 版共用同一套下载链路。

三只 LED 接在 **P3.5 / P3.6 / P3.7** 上依次点亮，每只亮约 250ms，循环往复。
代码只有 **93 字节**，启动路径最短 —— 第一次给新芯片烧录，
建议先烧这个工程确认「工具链 → 下载 → 芯片运行」这条链路是通的。

---

## 1. 硬件接线

### LED 部分（低电平点亮，最常见的共阳接法）

```
        ┌──────────────────────────────────────────────┐
        │  VCC (5V)                                    │
        │   │                                          │
        │   ├──►|── LED1 ──[330Ω]── P3.5  (第 8 脚)    │
        │   ├──►|── LED2 ──[330Ω]── P3.6  (第 9 脚)    │
        │   └──►|── LED3 ──[330Ω]── P3.7  (第 10 脚)   │
        └──────────────────────────────────────────────┘
```

* `►|` 是 LED，**长脚（阳极）朝 VCC**，短脚（阴极）经限流电阻接单片机。
* 电阻 330Ω ~ 1kΩ 都可以。8051 的 IO 灌电流能力（约 10~20mA）比拉电流强，
  所以「IO 拉低点亮」是最稳的接法。
* **如果你想接成高电平点亮**（IO → 电阻 → LED → GND），
  把程序里的 `CLR P3_x` 和 `SETB P3_x` 对调即可，见本文末尾。

### 下载 / 串口部分

```
     USB-TTL (CH340/CP2102)          STC89C52RC
         TXD  ─────────────────────►  P3.0  RXD  (第 10 脚)
         RXD  ◄─────────────────────  P3.1  TXD  (第 11 脚)
         GND  ──────────────────────  GND
         VCC  ──────────────────────  VCC (5V)
         DTR ──┬── 100nF ──────────►  RST  (第 9 脚)
               └── 10kΩ  ──────────►  VCC
```

* 只接 TXD / RXD / GND / VCC 就能下载，但每次下载前要**手动断电再上电**。
* 焊上 DTR 那个 RC 网络后，可以用 `-r dtr` 自动复位，省掉插拔。
* **STC89 是 5V 器件**：3.3V 的 CH340 有时能下进去但不稳，建议用 5V 供电的适配器。

### 为什么是 P3.5 / P3.6 / P3.7

| 引脚 | 第二功能 | 能不能拿来点灯 |
|---|---|---|
| P3.0 | RXD | **不行** —— 下载程序要用 |
| P3.1 | TXD | **不行** —— 下载程序要用 |
| P3.2 | INT0 | 可以，但常留给外部中断 |
| P3.3 | INT1 | 可以，但常留给外部中断 |
| P3.4 | T0 计数输入 | 可以，但常留给计数 |
| **P3.5** | T1 | **可以** —— 不用定时器 1 时是干净的 IO |
| **P3.6** | WR | **可以** —— 不接外部 RAM 时是干净的 IO |
| **P3.7** | RD | **可以** —— 不接外部 RAM 时是干净的 IO |

本工程不使用任何中断和外部存储器，所以 P3.5/P3.6/P3.7 是纯 GPIO。

---

## 2. 构建

```bash
# 在工作区根目录执行
mcs51 build examples/led-flow
```

产物在 `examples/led-flow/build/`：

| 文件 | 说明 |
|---|---|
| `led-flow.ihx` | Intel HEX，用来下载/烧录 |
| `led-flow.bin` | 原始二进制，给通用编程器用 |
| `led-flow.lst` | 汇编清单：每个地址上的机器码 + 对应源码 |
| `led-flow.map` | 链接 MAP：段布局、符号地址、内存占用 |

想直接看代码生成的机器码：

```bash
mcs51 disasm examples/led-flow/build/led-flow.ihx
```

---

## 3. 下载到芯片

### 3.1 先确认能看到串口

```bash
mcs51 devices
```

Linux 一般是 `/dev/ttyUSB0`，Windows 是 `COM3` 这类名字，macOS 是 `/dev/cu.usbserial-*`。

### 3.2 不接硬件先自检整条下载链路

```bash
mcs51 flash examples/led-flow --simulate
```

这一步会用一个模拟的 STC89C52RC 走完**完全相同**的协议流程
（同步 → 握手 → 换波特率 → 擦除 → 写块 → 写选项 → 运行），
并逐字节校验落盘结果。它通过而真实下载失败，问题就一定在线路或芯片状态上。

### 3.3 真实下载

```bash
# 方式一：手动复位（最简单，先试这个）
mcs51 flash examples/led-flow -p /dev/ttyUSB0
#   ↑ 看到 "cycle the power on the target now" 时，把板子断电再上电

# 方式二：DTR 自动复位（需要焊了那个 RC 网络）
mcs51 flash examples/led-flow -p /dev/ttyUSB0 -r dtr
```

正常输出：

```
mcs51-rs flash: led-flow
  image       led-flow.ihx (93 bytes)
  protocol    stc89
  port        /dev/ttyUSB0
  baud        115200 (handshake 2400), parity none
  reset       Manual
  options     keep the chip's
[reset]
  cycle the power on the target now
[handshake]
  model STC89C52RC
  target frequency 11.059 MHz
  BSL version 7.2D
  options 0xFF (12-clock, ...)
[baud]
[erase]
[program]
[options]
[run]

STC89C52RC programmed: 93 bytes, 0x0000..0x005C, in 1.8 s
the chip has been reset and is running the new firmware
```

看到 `programmed` 之后，三个 LED 就应该开始流水了。

### 3.4 只想看看芯片认不认

```bash
mcs51 probe -p /dev/ttyUSB0
```

只读型号 / 主频 / BSL 版本 / 选项字节，**不写任何东西**。

---

## 4. 预期现象

上电后三只 LED 依次点亮，每只约 250ms，形成循环流水：

```
LED1 ████░░░░░░░░░░░░░░░░░░░░  →  LED2 ░░░░████░░░░░░░░░░░░░░░░  →  LED3 ░░░░░░░░████░░░░░░░░░░░░
     └─250ms─┘
```

任何时候**只有一只亮**。

---

## 5. 改点什么

### 改流水速度

在 `src/main.asm` 的 `delay` 里改外层循环次数 `R5`：

| `MOV R5, #n` | 每只灯亮的时间（12MHz） |
|---|---|
| `#1` | 约 125ms |
| `#2` | 约 250ms（当前值） |
| `#4` | 约 500ms |
| `#8` | 约 1s |

晶振是 11.0592MHz 时同参数约慢 8%，肉眼无差别。
（裸机版不用串口，所以晶振是多少都无所谓 —— 下载也不受影响，
STC 的 BSL 会自己测量真实主频。）

### 改成来回跑（跑马灯 / 呼吸式）

把 `chase` 换成 5→6→7→6 的顺序即可：

```asm
chase:
        CLR   P35
        ACALL delay
        SETB  P35
        CLR   P36
        ACALL delay
        SETB  P36
        CLR   P37
        ACALL delay
        SETB  P37
        CLR   P36
        ACALL delay
        SETB  P36
        SJMP  chase
```

### 改成高电平点亮

把 `leds_off` 里的 `SETB` 换成 `CLR`，把 `chase` 里的 `CLR`/`SETB` 对调。例如：

```asm
leds_off:
        CLR   P35        ; 高电平点亮时，输出低 = 灭
        CLR   P36
        CLR   P37
        RET
```

### 换到别的引脚

只要把 `P35` / `P36` / `P37` 换成 `P1_0` / `P1_1` / `P1_2` 之类的位名即可。
`8051.inc` 里已经定义了 `P0_0`~`P37` 全部 32 个引脚位名。

---

## 6. 排查

| 现象 | 原因 / 处理 |
|---|---|
| `the chip never answered the synchronisation pulse` | 芯片没进 BSL：断电重上电时立刻执行命令；或换 `-r dtr` / `-r rts`；也可能是 TXD/RXD 接反了 |
| `baud check answered with ... expected 8F` | 主频推算不准或波特率太高：`-b 57600` 或 `-b 19200` 重试 |
| `block N failed its checksum` | 线路干扰：降低波特率、缩短杜邦线、确认 5V 供电 |
| 下载成功但灯不亮 | ① LED 极性接反；② 看 `mcs51 probe` 的选项字节，确认没把芯片配成外部晶振但板上没晶振；③ 用的是 11.0592MHz 晶振也没关系，程序不依赖精确时钟 |
| 只有一个灯常亮 | 另外两只的限流电阻或杜邦线接触不良 |
| 灯亮但很暗 | 限流电阻太大（换 330Ω），或 IO 被别的东西拉低了 |

---

## 7. 代码导读

`src/main.asm` 的结构：

| 位置 | 内容 |
|---|---|
| `0x0000` | 复位向量：`LJMP main` |
| `0x0003`~`0x002B` | 中断向量表：全部填 `RETI`（本工程不用中断，但不能留空） |
| `0x0030` | `main`：设栈指针 → 全灭 → 进主循环 |
| — | `chase`：5→6→7 循环 |
| — | `leds_off`：三只全灭 |
| — | `delay`：三层嵌套空循环，约 250ms |

两个值得注意的写法：

1. **`MOV SP, #0x30`** —— 8051 复位后 `SP = 0x07`，如果不重设，
   第一次压栈就会覆盖寄存器组 1（地址 0x08~0x0F）。本程序设成 0x30，
   给堆栈留出 0x30~0x7F 的空间。

2. **用 `SETB` / `CLR` 而不是 `MOV P3, #立即数`** ——
   位操作只改目标引脚，不会影响 P3 上其它位。特别是 P3.0/P3.1 是串口脚，
   写完程序后如果它们被拉低，下次下载就会失败。
   测试 `the_bare_metal_example_leaves_the_serial_pins_alone` 专门盯着这一点。

---
---

## 8. 想看点更复杂的

* [`../led-flow-rs/`](../led-flow-rs/) —— 同样的灯，但**用 Rust 写**，
  而且 P3.7 是 Timer 0 软件 PWM 的呼吸灯（180 字节）
* [`../blink/`](../blink/) —— 汇编裸机：翻引脚 + 轮询串口报数（152 字节）
