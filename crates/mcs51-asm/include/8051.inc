; ---------------------------------------------------------------------------
; 8051.inc -- standard Intel MCS-51 special function registers and bit names
;
; Shipped inside mcs51-asm, so `.include "8051.inc"` always resolves even when
; no header is present on disk. Values follow the original Intel 8051/8052
; datasheet memory map.
; ---------------------------------------------------------------------------

; --- Special function registers (direct addresses 0x80..0xFF) --------------
P0      .sfr    0x80
SP      .sfr    0x81
DPL     .sfr    0x82
DPH     .sfr    0x83
PCON    .sfr    0x87
TCON    .sfr    0x88
TMOD    .sfr    0x89
TL0     .sfr    0x8A
TL1     .sfr    0x8B
TH0     .sfr    0x8C
TH1     .sfr    0x8D
P1      .sfr    0x90
SCON    .sfr    0x98
SBUF    .sfr    0x99
P2      .sfr    0xA0
IE      .sfr    0xA8
P3      .sfr    0xB0
IP      .sfr    0xB8
T2CON   .sfr    0xC8
T2MOD   .sfr    0xC9
RCAP2L  .sfr    0xCA
RCAP2H  .sfr    0xCB
TL2     .sfr    0xCC
TH2     .sfr    0xCD
PSW     .sfr    0xD0
ACC     .sfr    0xE0
B       .sfr    0xF0

; --- Bit addresses ---------------------------------------------------------
; Port 0
P0_0    .bit    0x80
P0_1    .bit    0x81
P0_2    .bit    0x82
P0_3    .bit    0x83
P0_4    .bit    0x84
P0_5    .bit    0x85
P0_6    .bit    0x86
P0_7    .bit    0x87

; Timer/counter control
IT0     .bit    0x88
IE0     .bit    0x89
IT1     .bit    0x8A
IE1     .bit    0x8B
TR0     .bit    0x8C
TF0     .bit    0x8D
TR1     .bit    0x8E
TF1     .bit    0x8F

; Port 1
P1_0    .bit    0x90
P1_1    .bit    0x91
P1_2    .bit    0x92
P1_3    .bit    0x93
P1_4    .bit    0x94
P1_5    .bit    0x95
P1_6    .bit    0x96
P1_7    .bit    0x97

; Serial port control
RI      .bit    0x98
TI      .bit    0x99
RB8     .bit    0x9A
TB8     .bit    0x9B
REN     .bit    0x9C
SM2     .bit    0x9D
SM1     .bit    0x9E
SM0     .bit    0x9F

; Port 2
P2_0    .bit    0xA0
P2_1    .bit    0xA1
P2_2    .bit    0xA2
P2_3    .bit    0xA3
P2_4    .bit    0xA4
P2_5    .bit    0xA5
P2_6    .bit    0xA6
P2_7    .bit    0xA7

; Interrupt enable
EX0     .bit    0xA8
ET0     .bit    0xA9
EX1     .bit    0xAA
ET1     .bit    0xAB
ES      .bit    0xAC
ET2     .bit    0xAD
EA      .bit    0xAF

; Port 3, pinned out both ways: P3_n for plain I/O and the alternate
; function name for the same pin.
P3_0    .bit    0xB0
P3_1    .bit    0xB1
P3_2    .bit    0xB2
P3_3    .bit    0xB3
P3_4    .bit    0xB4
P3_5    .bit    0xB5
P3_6    .bit    0xB6
P3_7    .bit    0xB7

RXD     .bit    0xB0
TXD     .bit    0xB1
INT0    .bit    0xB2
INT1    .bit    0xB3
T0      .bit    0xB4
T1      .bit    0xB5
WR      .bit    0xB6
RD      .bit    0xB7

; Interrupt priority
PX0     .bit    0xB8
PT0     .bit    0xB9
PX1     .bit    0xBA
PT1     .bit    0xBB
PS      .bit    0xBC
PT2     .bit    0xBD

; Timer 2 control
CPRL2   .bit    0xC8
CT2     .bit    0xC9
TR2     .bit    0xCA
EXEN2   .bit    0xCB
TCLK    .bit    0xCC
RCLK    .bit    0xCD
EXF2    .bit    0xCE
TF2     .bit    0xCF

; Program status word
P       .bit    0xD0
OV      .bit    0xD1
RS0     .bit    0xD2
RS1     .bit    0xD3
F0      .bit    0xD4
AC      .bit    0xD5
CY      .bit    0xD6

; --- Useful bit masks ------------------------------------------------------
IE_EA   .equ    0x80
IE_ES   .equ    0x10
IE_ET1  .equ    0x08
IE_EX1  .equ    0x04
IE_ET0  .equ    0x02
IE_EX0  .equ    0x01

TMOD_T1_GATE   .equ    0x80
TMOD_T1_CT     .equ    0x40
TMOD_T1_MODE1  .equ    0x10
TMOD_T0_GATE   .equ    0x08
TMOD_T0_CT     .equ    0x04
TMOD_T0_MODE2  .equ    0x02

TMOD_T0_TIMER_MODE0 .equ 0x00
TMOD_T0_TIMER_MODE1 .equ 0x01
TMOD_T0_TIMER_MODE2 .equ 0x02
TMOD_T0_TIMER_MODE3 .equ 0x03

PCON_IDL .equ 0x01
PCON_PD  .equ 0x02

SCON_MODE0 .equ 0x00
SCON_MODE1 .equ 0x40
SCON_MODE2 .equ 0x80
SCON_MODE3 .equ 0xC0
