# 为什么 Rust 不能直接编译到 8051 —— 以及三条可行路线

这份文档回答一个问题：**「我们已经有汇编器、链接器、ISA 表了，封装一下接进 LLVM 不就行了？」**

结论是：**不行，而且这不是工作量的问题，是定位的问题。** 下面把账算清楚。

---

## 1. 事实核对

```console
$ rustc --print target-list | grep -iE "8051|mcs51"
  （无）

$ clang --print-targets | grep -iE "8051|mcs51"
  （无）

$ rustc --print target-list | grep -iE "avr|msp430"
avr-none
msp430-none-elf
```

8 位平台里只有 AVR 和 MSP430 有 LLVM 后端 —— 因为**有人给它们写了后端**。
MCS-51 至今没有，GCC 也从来没有合并过（3.x 时代那个 `gcc-mcs51` 是私有 fork）。

SDCC 从 1990 年做到今天，仍然用手写的寄存器分配器 + 巨型窥孔优化器，
而不是套用某个通用框架。这不是历史包袱，是**这个架构的特性决定的**。

---

## 2. 核心误解：LLVM 后端不会「用」我们的框架，它会**取代**它

我们已有的东西，在 LLVM 的分层里对应这两个位置：

| 本仓库的 crate | LLVM 里对应的层 | LLVM 是否已经有 |
|---|---|---|
| `mcs51-isa`（操作码表、编码/解码） | MC 层（指令编码） | 要写，但这是**最机械的 10%** |
| `mcs51-asm`（汇编器） | MC 层的 AsmParser / AsmPrinter | ✅ LLVM 自带框架 |
| `mcs51-ld`（链接器） | lld | ✅ LLVM 自带 |
| `mcs51-sim`（模拟器） | 无对应，是验证手段 | —— |

也就是说：**「封装我们的框架」给不了 LLVM 任何它没有的东西**。
一个 LLVM 后端真正缺的是下面这 8 块，我们一块都没有：

| # | 组件 | 干什么 | 我们有吗 |
|---|---|---|---|
| 1 | `TargetMachine` / `Subtarget` | 注册目标、特性开关 | ✗ |
| 2 | `DataLayout` | 指针宽度、对齐 | ✗ |
| 3 | `RegisterInfo` | 寄存器集合与语义 | ✗ |
| 4 | `CallingConv` | 参数传递、谁保存谁 | ✗ |
| 5 | **Instruction Selection** | IR → 机器指令 | ✗ |
| 6 | **Legalization** | 把 i16 运算拆成 8 位序列 | ✗ |
| 7 | **Register Allocation** | 虚拟寄存器 → 物理寄存器 | ✗ |
| 8 | **Frame Lowering** | 序言/尾声、溢出槽 | ✗ |

5、7、8 是**全部难点所在**。

---

## 3. 8051 为什么特别难

LLVM 的模型假设：寄存器大致正交、栈帧可以用 SP/FP 相对寻址。8051 把这两条同时打破：

### 3.1 累加器 + 二地址指令

所有算术都是 `A = A op operand`：

```asm
ADD   A, R0        ; 破坏 A
```

LLVM 的寄存器分配器要靠 coalescing 消掉复制，但 8051 上**复制本身也要占指令**，
而且累加器 A 是唯一的算术目标 —— 任何 `x + y` 都要先挪进 A、算完再挪出去。

### 3.2 没有 SP 相对寻址

局部变量必须落在 IDATA，用**8 位直接地址**访问；或者用 `@R0`/`@R1` 间接。
LLVM 的 frame lowering 假设 `[SP+offset]` 或 `[FP+offset]`，
而 8051 只能给它一个**编译期固定的绝对地址**。

### 3.3 三个重叠的地址空间

| 空间 | 地址范围 | 访问方式 |
|---|---|---|
| DATA / IDATA | 0x00–0xFF | `MOV A, direct` / `MOV A, @Ri` |
| SFR | 0x80–0xFF | `MOV A, direct`（**与 DATA 高 128 字节重叠**） |
| CODE | 0x0000–0xFFFF | `MOVC A, @A+DPTR` |
| XDATA | 0x0000–0xFFFF | `MOVX A, @DPTR` / `@Ri` |
| BIT | 0x00–0xFF | `SETB bit` / `JB bit, rel` |

同一个 `0x90`，是 P1 口还是间接寻址的 RAM，取决于用哪条指令。每次 load/store 都要选对。

### 3.4 R0–R7 有 4 组

靠 `PSW.RS0` / `RS1` 切换。同一个「R0」有 4 个物理地址。跨组调用要手动切，
而且 `PUSH 0x00` 永远指 bank 0。

### 3.5 DPTR 是唯一能访问 CODE/XDATA 的 16 位寄存器

单点资源，且是完整的独立地址空间。

### 3.6 位寻址空间

没有 LLVM 对应物。SDCC 用 `__bit` 存储类硬扛。

### 3.7 其它

* `MUL AB` / `DIV AB` 固定 `A×B → A:B`
* 没有通用条件码，只有 C 和特定指令设置的个别标志
* 中断用 8 字节间隔的固定向量表

---

## 4. 还有一道工程上的坎：你得 fork LLVM

即使你写出了后端，它也不在 rustc 用的那个 LLVM 里（本机内置 **LLVM 22.1.8**）。
两条路：

| 路线 | 代价 |
|---|---|
| **推进 LLVM 主线** | 参考 AVR 后端的历程：多年评审。且至今无人对 MCS-51 开这个头 |
| **Fork** | 维护一个打了补丁的 LLVM + 一个用它的 rustc + 目标的 `core` / `compiler_builtins` / ABI |

第二条意味着：**你交付的不再是一个 cargo 项目，而是一套自己维护的 Rust 工具链发行版。**

---

## 5. 三条真实可行的路

| | 方案 | 做法 | 工期 | 跑在芯片上的是 |
|---|---|---|---|---|
| **A** | **Rust eDSL 生成器** | Rust 在主机跑，类型安全 API 生成 8051 汇编，复用现有 asm/ld/sim | **1–2 天** | 8051 机器码 |
| **B** | **Rust 子集编译器** | `syn` 解析 Rust 子集 → 自研 IR → 8051 codegen（照 SDCC 的 AST→iCode→asm 路子） | **数周** | **真正的 Rust 语义** |
| **C** | **LLVM 后端** | fork LLVM，写 TableGen + ISel + RA + FrameLowering + rustc target spec | **数月～数年** | 真正的 Rust |

### A 已经实现（`crates/mcs51-codegen`）

```
src/main.rs  ──►  mcs51-codegen  ──►  mcs51-asm  ──►  mcs51-ld  ──►  .ihx
（真正的 Rust）   生成 8051 汇编      汇编器         链接器       烧录
```

**得到**：`cargo build` / `cargo test` / rustc 类型检查 / IDE 补全 / 引脚是编译期常量 /
毫秒→节拍在主机算好。

**代价**：目标机的循环要写成 `t.forever(|t| { ... })`，
因为 Rust 的 `for`/`while` 是在**主机上**展开的，不会变成目标机的跳转。

### B 是唯一能在不 fork LLVM 的前提下让真 Rust 跑在片上的路

8051 的那些坑对**通用**后端是灾难，但对一个**受控子集**反而好办 ——
可以像 SDCC 那样只把 A 当累加器、用简单栈式模型，不做通用寄存器分配。

建议的子集边界：

| 支持 | 不支持 |
|---|---|
| `fn`、直接调用 | 泛型、trait、闭包 |
| `u8` / `u16` / `bool` | 浮点 |
| `let`、赋值、`if`/`else`、`while`、`loop`、`break`/`continue` | `match`（可后续加） |
| 数组（IDATA 定长）、常量索引 + 变量索引 | 动态分配、切片 |
| 端口访问用 `volatile` 语义的内建类型 | 引用、所有权机制（简化为值语义） |
| 直接调用约定（固定 bank 0） | 递归（8051 栈太小，可限制） |

工作量分解（粗估）：

| 模块 | 规模 |
|---|---|
| `syn` 前端 + 语义检查 | ~800 行 |
| IR 定义与 lowering | ~1000 行 |
| 8051 codegen（含寄存器分配） | ~1500 行 |
| 常量折叠 + 窥孔优化 | ~500 行 |
| 测试（含模拟器验证） | ~800 行 |

### A 和 B 不冲突

A 立即可用，且 B 可以直接复用 A 已经验证过的：

* 引脚 / 寄存器 / 标签的抽象
* `mcs51-asm` → `mcs51-ld` → `mcs51-sim` 的完整验证链路
* 「生成的代码必须逐字节可验证」的测试方法

---

## 6. 本仓库的选择与理由

选了 **A**，理由：

1. **立即有价值**：用户要的是「用 `main.rs` 写应用」，A 今天就能给。
2. **风险低**：不 fork 工具链，不引入需要长期维护的分支。
3. **为 B 铺路**：A 的抽象层和验证链路正是 B 需要的基础设施。
4. **诚实**：A 的能力边界（Rust 在构建期跑）在文档里写清楚了，没有过度承诺。

如果后续要做 B，从 `crates/mcs51-codegen` 的 `Board` / 抽象层继续往上建即可。
如果要做 C，先准备好 fork LLVM 和维护一套 Rust 工具链发行版的长期投入。

---

## 7. 参考

* [LLVM 官方文档：Writing an LLVM Backend](https://llvm.org/docs/WritingAnLLVMBackend.html)
* [SDCC 手册](https://sdcc.sourceforge.net/doc/sdccman.pdf) —— 看它为什么自己写寄存器分配
* `rustc --print target-list`、`clang --print-targets` 自己验一遍
