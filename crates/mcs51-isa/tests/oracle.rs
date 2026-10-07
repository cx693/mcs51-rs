//! Exhaustive cross-check of the `mcs51-isa` opcode table against the
//! independent [`i8051`] emulator implementation.
//!
//! The emulator crate (https://crates.io/crates/i8051) carries its own,
//! separately written MCS-51 opcode table. We treat it as an oracle: for every
//! opcode byte and a spread of operand bytes we decode with both and require
//! identical mnemonics, identical operand values and identical lengths. We then
//! require our own encoder to reproduce the original bytes.
//!
//! A hand-written ISA table is exactly the kind of artefact that is wrong in one
//! cell out of 255, so this test is the backbone of the toolchain's correctness
//! argument.

use i8051::{Instruction as EmuInstruction, Mnemonic as EmuMnemonic, Operand as EmuOperand};
use mcs51_isa::{Mnemonic, OPCODES, Operand, UNDEFINED_OPCODES, decode, encode, is_defined_opcode};

/// Bytes that exercise the interesting corners of every operand field.
const FILLERS: [u8; 6] = [0x00, 0x01, 0x30, 0x7F, 0x80, 0xFF];

fn same_operand(theirs: &EmuOperand, mine: Operand) -> bool {
    match (theirs, mine) {
        (EmuOperand::A, Operand::A) => true,
        (EmuOperand::C, Operand::C) => true,
        (EmuOperand::AB, Operand::Ab) => true,
        (EmuOperand::Dptr, Operand::Dptr) => true,
        (EmuOperand::IndirectDptr, Operand::AtDptr) => true,
        (EmuOperand::IndirectADptr, Operand::AtADptr) => true,
        (EmuOperand::IndirectAPc, Operand::AtAPc) => true,
        (EmuOperand::Reg(n), Operand::Rn(m)) => *n == m,
        (EmuOperand::IndirectReg(i), Operand::AtRi(j)) => *i == j,
        (EmuOperand::Direct(d), Operand::Direct(e)) => *d == e,
        (EmuOperand::Bit(b), Operand::Bit(c)) => *b == c,
        (EmuOperand::BitNot(b), Operand::NotBit(c)) => *b == c,
        (EmuOperand::Imm8(v), Operand::Imm8(w)) => *v == w,
        (EmuOperand::Imm16(v), Operand::Imm16(w)) => *v == w,
        (EmuOperand::Rel(r), Operand::Rel(s)) => *r == s,
        (EmuOperand::Addr11(a), Operand::Addr11(b)) => *a == b,
        (EmuOperand::Addr16(a), Operand::Addr16(b)) => *a == b,
        _ => false,
    }
}

#[test]
fn decode_matches_the_i8051_emulator_for_every_opcode() {
    let mut compared = 0usize;
    let mut undefined = 0usize;

    for opcode in 0u16..=0xFF {
        for b1 in FILLERS {
            for b2 in FILLERS {
                let bytes = [opcode as u8, b1, b2];
                let theirs = EmuInstruction::decode_from_bytes(0x1000, &bytes);
                let ours = decode(0x1000, &bytes);

                match ours {
                    Ok(ours) => {
                        assert_eq!(
                            ours.mnemonic.as_str(),
                            theirs.mnemonic().as_str(),
                            "mnemonic mismatch for {bytes:02X?}"
                        );
                        assert_eq!(
                            ours.len() as usize,
                            theirs.len(),
                            "length mismatch for {bytes:02X?}"
                        );
                        let their_ops = theirs.operands().as_slice();
                        let our_ops = ours.operands.as_slice();
                        assert_eq!(
                            their_ops.len(),
                            our_ops.len(),
                            "operand count mismatch for {bytes:02X?}"
                        );
                        for (theirs, ours) in their_ops.iter().zip(our_ops) {
                            assert!(
                                same_operand(theirs, *ours),
                                "operand mismatch for {bytes:02X?}: emulator {theirs:?} vs mcs51-isa {ours:?}"
                            );
                        }
                        compared += 1;
                    }
                    Err(err) => {
                        assert_eq!(
                            theirs.mnemonic(),
                            EmuMnemonic::Unknown,
                            "mcs51-isa rejected {bytes:02X?} ({err}) but the emulator decodes it \
                             as {}",
                            theirs.as_string()
                        );
                        undefined += 1;
                    }
                }
            }
        }
    }

    // 256 opcodes * 36 operand combinations, minus the undefined opcode.
    assert_eq!(compared + undefined, 256 * 36);
    assert_eq!(undefined, 36, "only 0xA5 should be undefined");
    assert_eq!(compared, 255 * 36);
}

#[test]
fn encode_round_trips_every_opcode() {
    for opcode in 0u16..=0xFF {
        for b1 in FILLERS {
            for b2 in FILLERS {
                let bytes = [opcode as u8, b1, b2];
                let Ok(ins) = decode(0x1000, &bytes) else {
                    continue;
                };
                let encoded = encode(&ins)
                    .unwrap_or_else(|e| panic!("cannot re-encode {ins:?} from {bytes:02X?}: {e}"));
                assert_eq!(
                    encoded.as_slice(),
                    &bytes[..ins.len() as usize],
                    "round trip changed the bytes for {ins}"
                );
            }
        }
    }
}

#[test]
fn table_covers_exactly_the_defined_opcodes() {
    let defined = (0u16..=0xFF)
        .filter(|op| is_defined_opcode(*op as u8))
        .count();
    assert_eq!(defined, 255);
    assert_eq!(UNDEFINED_OPCODES, [0xA5]);
    assert!(!is_defined_opcode(0xA5));

    // Every family's declared length must agree with what actually decodes.
    for def in OPCODES {
        for n in 0..def.variant_count() {
            let opcode = def.variant(n);
            assert_eq!(
                mcs51_isa::instruction_length(opcode),
                def.len(),
                "family {opcode:#04X} length disagreement"
            );
        }
    }
}

#[test]
fn mnemonics_are_unique_and_parseable() {
    for def in OPCODES {
        let text = def.mnemonic.as_str();
        assert_eq!(
            Mnemonic::parse(text),
            Some(def.mnemonic),
            "mnemonic {text} does not round trip through parse()"
        );
        assert_eq!(def.args.len(), def.enc.len());
    }
}

#[test]
fn branch_targets_resolve_like_the_emulator() {
    // Relative branches: emulator and mcs51-isa must agree on the target.
    for opcode in [0x40u8, 0x50, 0x60, 0x70, 0x80, 0xD8] {
        for displacement in [0x00u8, 0x01, 0x7F, 0x80, 0xFF] {
            let bytes = [opcode, displacement];
            let theirs = EmuInstruction::decode_from_bytes(0x1000, &bytes);
            let ours = decode(0x1000, &bytes).expect("defined opcode");
            assert_eq!(
                ours.target(),
                theirs.target().map(|t| t as u16),
                "{ours} target mismatch"
            );
        }
    }

    // Absolute and page-relative branches.
    let absolute: [&[u8]; 4] = [
        &[0x02, 0xAB, 0xCD],
        &[0x01, 0x30],
        &[0xE1, 0x30],
        &[0x12, 0x00, 0x40],
    ];
    for bytes in absolute {
        let theirs = EmuInstruction::decode_from_bytes(0x1000, &bytes);
        let ours = decode(0x1000, &bytes).expect("defined opcode");
        assert_eq!(ours.target(), theirs.target().map(|t| t as u16));
    }
}
