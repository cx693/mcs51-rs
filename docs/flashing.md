# 下载到硬件 / Flashing real hardware

## 0. 先回答一个问题：现在生成的固件能直接下载吗？

**能，而且现在有两条路：**

1. **任何现成的 8051 编程器 / STC-ISP 都能直接烧。**
   我们生成的是标准 Intel HEX（`.ihx`），起始地址 0x0000，和 SDCC 产物格式完全一致。
   STC-ISP（厂商 Windows 工具）、`stcgal`、`stcflash`、通用编程器都能直接吃。

2. **本工具链现在自带 `mcs51 flash`**，通过串口直接下载到 STC 单片机内置的
   BSL（Boot Strap Loader），不需要外部编程器：

   ```
   $ mcs51 devices                       # 看有哪些串口
   $ mcs51 flash examples/blink --simulate    # 不接硬件先自检整条下载链路
   $ mcs51 flash examples/blink -p /dev/ttyUSB0 -r dtr
   ```

**两点必须注意：**

* **STC89 系列没有 IAP**（不能自己擦写自己），所以必须用芯片内置的 BSL —— 也就是
  STC-ISP 走的那条路。本项目的 `mcs51 flash` 复现的就是这个协议。
  想要"应用自己升级自己"，只有带 IAP 的型号（STC12/15/8 的 IAP 系列、STC8A/8H）才行。
* 首次给一块新板子下载时，请当成一次冒烟测试：先 `mcs51 probe` 确认能读到芯片型号，
  再 `mcs51 flash`。

---

## 1. 硬件接线

```
USB-TTL (CH340/CP2102/FT232)          MCU
      3.3V/5V  ─────────────────────  VCC
      TXD      ─────────────────────  RXD (P3.0)
      RXD      ─────────────────────  TXD (P3.1)
      GND      ─────────────────────  GND
      DTR ──┬── 100nF ── RST          (可选：自动复位)
            └── 10kΩ ── VCC
```

* 只接 TXD/RXD/GND 就能工作，但每次下载前需要**手动断电再上电**（`-r manual`）。
* 加上 DTR→RST 的电容耦合就能用 `-r dtr` 自动复位，省掉手动插拔。
* **注意电平**：STC89 是 5V 器件，用 3.3V 的 CH340 时如果下载不稳定，
  换 5V 供电的适配器，或者给 RXD 加电平转换。

---

## 2. 命令

```
mcs51 flash [工程目录 | 文件.ihx]
    -p, --port <PORT>        串口，例如 /dev/ttyUSB0、COM3
    -P, --protocol <FAMILY>  stc89（默认）或 stc12
    -b, --baud <BAUD>        传输波特率，默认 115200
        --handshake <BAUD>   握手波特率，默认 2400（BSL 只在这个速率上听）
    -r, --reset <MODE>       manual | dtr | rts | dtr_inverted | rts_inverted
        --option <BYTE>      要写入的选项(熔丝)字节，默认保持芯片原值
        --no-options         完全不写选项字节
        --flash-size <BYTES> 目标 Flash 大小，用来提前拒绝过大的镜像
        --dry-run            只打印计划，不碰串口
        --simulate           下载到模拟芯片并逐字节校验

mcs51 probe -p <PORT>        连接芯片并打印型号/主频/BSL 版本/选项字节，不写任何东西
mcs51 devices                列出系统里的串口
```

### 典型输出

```
$ mcs51 flash examples/blink --simulate
mcs51-rs flash: blink
  image       blink.ihx (1006 bytes)
  protocol    stc89
  baud        115200 (handshake 2400), parity none
  reset       Manual
  options     keep the chip's
[reset]
  cycle the power on the target now
[handshake]
  model STC89C52RC
  target frequency 11.059 MHz
  BSL version 7.2D
  options 0xFF (12-clock, BSL pin detect off, EEPROM erase off, clock gain high, ALE on, XRAM on, watchdog off)
[baud]
[erase]
[program]
[options]
[run]

simulated STC89C52RC accepted 1006 bytes and verified clean
```

进度信息走 **stderr**，结果走 **stdout**，所以 `mcs51 flash ... > log.txt` 依然干净。

---

## 3. 工程里固化板子参数

`mcs51.toml` 加了 `[flash]` 段，这样一条 `mcs51 flash` 就能下载：

```toml
[flash]
port = "/dev/ttyUSB0"     # 也可以留空，用 -p 传
protocol = "stc89"        # stc89 / stc12
baud = 115200
handshake_baud = 2400
reset = "dtr"             # manual / dtr / rts / dtr_inverted / rts_inverted
write_options = true
# option = 0xFF           ; 想固定熔丝就填，否则保持芯片原值
flash_size = 8192         # STC89C52RC 是 8K
```

---

## 4. 协议实现说明

STC 的 ISP 协议厂商没有公开文档，本项目是按开源实现
[`stcflash`](https://github.com/cx693/StcFlash)（`stcgal` 的二次开发）**逐条复现**的：

| 环节 | STC89/90 | STC12/15/8 |
|---|---|---|
| 串口校验位 | 无 | **偶校验** |
| 包校验和 | 1 字节 | **2 字节** |
| 包头 | `46 B9 6A`（主机）/ `46 B9 68`（芯片） | 同上 |
| 包尾 | `16` | 同上 |
| 长度字段 | `载荷 + 5` | `载荷 + 6` |
| 同步 | 反复发 `7F` 直到芯片回状态包 | 同上 |
| 切波特率 | `8F`（试）→ `8E`（切）→ 4 次 `80` 乒乓 | `50`（起）→ `8F`（试）→ `8E`（切，回 `84`） |
| 擦除 | `84` + 块数 + `33×6` | `84` + 长度 + 从 `0x80` 倒数的**魔数守卫** |
| 写块 | 128 字节/块，回 `80` + 数据校验和 | 128 字节/块，首块 `22`、其后 `02`，回 `02 54` |
| 收尾 | — | BSL ≥ 7.2 需要 `07` 写完成包 |
| 选项字节 | `8D` | `8D` |
| 运行新程序 | `82` | `82` |

### 与参考实现有意不同的一处

参考实现把**最后一个不满 128 字节的块用 `0x00` 补齐**。本项目改用 `0xFF` 补齐：

* 校验和因此改为对**补齐后的整块**计算（和参考实现的行为一致，参考实现里
  `sum(packet[7:])` 本来也是整块）；
* 好处是镜像末尾之后仍是"擦除态"，读回来和未烧写过一样，而不是一片 0x00。

### 波特率计算

```
STC89:   BRT = 65536 - round(Fosc / (Baud × 32))      ; 6T 模式用 16
         BRT_CKSUM = (2 × (256 - BRT)) & 0xFF          ; 16 位回绕语义
         IAP 等待: <5MHz 0x83, <10MHz 0x82, <20MHz 0x81, 否则 0x80
         delay = 0xA0

STC12:   BRT = 256 - round(Fosc / (Baud × 16))        ; 必须在 2..255
         BRT_CKSUM = (2 × (256 - BRT)) & 0xFF
         IAP 等待: <1/2/3/6/12/20/24MHz → 0x87/86/85/84/83/82/81/80
         delay = 0x80
```

主频不是猜的：芯片在状态包里上报"一个位时间里有几个自己的时钟"，
取 8 组样本平均后反推 `Fosc`（这正是参考实现的做法）。

---

## 5. 怎么验证的（没有硬件也能验）

`mcs51-flash` 的所有协议代码只跟 `Transport` trait 打交道，从不直接碰串口。
这个 trait 有三个实现：

| 实现 | 用途 |
|---|---|
| `SerialTransport` | 真实串口（`serial` feature，默认开） |
| `LoopbackTransport` | 直连一个设备模型，无 I/O |
| `SimTransport` | 连到 `mcs51-sim` 里跑的**真实 8051 固件** |

测试里最重要的是这一条：

```rust
// crates/mcs51-flash/tests/flash.rs
fn flashes_the_real_firmware_byte_for_byte() {
    // 用本项目自己的汇编器 + 链接器编出真实固件
    let firmware = build_bare_metal(BLINK);
    // 走完整下载流程：同步 → 握手 → 换波特率 → 擦除 → 写块 → 选项 → 运行
    let report = flash_over_transport(transport, options, &firmware.to_intel_hex(), 8192, ...)?;
    // 落进"Flash"的字节必须和镜像逐字节相同
    assert_eq!(&chip.borrow().flash[..expected.len()], &expected[..]);
}
```

再加 `mcs51 flash --simulate`，用户自己也能一条命令自检整条下载链路。

### 真机验证记录

**2025 年实测通过**（不是模拟器）：

| 项目 | 实测值 |
|---|---|
| 芯片 | **STC89C52RC**，model id `0xF002` |
| BSL 版本 | **6.6C** |
| 晶振 | 实测 **11.030 MHz**（标称 11.0592 MHz，采样反推误差 0.26%） |
| 时钟模式 | 12 时钟（选项字节 bit0 = 1） |
| 选项字节 | `0xFD`，**下载后保持不变**（`write_options` 读回原值写回） |
| 串口适配器 | CH340，`/dev/cu.wchusbserial21220` |
| 下载 | 传输 115200 波特率，用时 3.4–6.8 秒（含等操作员断电重上电） |
| 同步 | 第 1.2–2.8 秒应答（发了 29–74 次 `0x7F`） |
| 复烧 | 同一块板连烧两次都成功，选项字节始终读回 `0xFD` 不变 |
| 固件 | `examples/led-flow-rs`（Rust 生成，裸机，180 字节） |

烧完后从串口读回的实测输出：

```
$ stty -f /dev/cu.wchusbserial21220 9600 cs8 -cstopb -parenb raw
$ cat /dev/cu.wchusbserial21220          # 读 7 秒
mcs51-rs rust app, uptime 16 s
mcs51-rs rust app, uptime 17 s
...
mcs51-rs rust app, uptime 23 s
```

7 秒收到 8 行 —— **正好 1 行/秒**，说明 Timer 0 时基、重载值、
12 时钟假设和串口波特率全部正确。

### 已验证 / 未验证

**已验证**（136 个测试 + 上面的真机记录）：
* 包的组帧、长度字段、两种校验和、偶校验切换
* 握手/切波特率/擦除/写块/选项/运行 的完整时序
* 波特率寄存器算得对不对（拿 11.0592 MHz + 115200 反推验证）
* 镜像过大、镜像损坏、芯片不应答 的错误路径
* 真实固件 1000+ 字节逐字节落盘一致

**未验证**：
* **STC12/15/8 那一支没有在真机上跑过** —— 只有 STC89C52RC（BSL 6.6C）实测通过。
  两套协议只差校验位、校验和宽度和几条命令，但没跑过就是没跑过。
* STC15/STC8 的选项字节布局（`trim` 频率校准等）没有实现，只做了能下载的最小集。
* USB 直连型号（`StcUsb15Protocol`）没有实现。
* BSL 7.2+ 的写完成包（`0x07`）没有条件实测 —— 手上的片子是 6.6C，走不到那条分支。

---

## 6. 排查

| 现象 | 原因 / 处理 |
|---|---|
| `the chip never answered the synchronisation pulse` | 没进 BSL 模式：断电重上电，或换 `-r dtr` / `-r rts`。也可能是 TXD/RXD 接反。 |
| `baud check answered with ... expected 8F` | 主频推算不准或波特率太高；降到 `-b 57600` 或 `-b 19200` 重试。 |
| `block N failed its checksum` | 线路干扰。降低波特率，缩短杜邦线，或加 5V 供电。 |
| 3.3V CH340 下载不稳定 | STC89 是 5V 器件，换 5V 适配器或加电平转换。 |
| `no serial port` | 用 `mcs51 devices` 看端口名，或在 `[flash] port` 里填。 |
| 下载后程序不跑 | 选项字节把 6T/看门狗/ALE 配错了。用 `mcs51 probe` 看当前值，必要时用 `--option` 显式写回。 |
