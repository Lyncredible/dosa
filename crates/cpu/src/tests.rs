use super::*;

fn program(code: &[u8]) -> Cpu {
    program_for(CpuProfile::default(), code)
}

fn program_for(profile: CpuProfile, code: &[u8]) -> Cpu {
    let mut cpu = Cpu::with_profile(profile);
    cpu.regs.cs = 0x1200;
    cpu.regs.ds = 0x1300;
    cpu.regs.es = 0x1400;
    cpu.regs.ss = 0x1500;
    cpu.regs.ip = 0x100;
    cpu.regs.sp = 0xfffe;
    cpu.load(cpu.regs.cs, cpu.regs.ip, code);
    cpu
}

fn advance(cpu: &mut Cpu, count: usize) {
    for _ in 0..count {
        assert_eq!(cpu.step().unwrap(), Step::Continue);
    }
}

#[test]
fn dos_call_leaves_registers_and_next_ip_available_to_host() {
    // MOV AX,4C2Ah / INT 21h
    let mut cpu = program(&[0xb8, 0x2a, 0x4c, 0xcd, 0x21]);
    advance(&mut cpu, 1);
    assert_eq!(cpu.step().unwrap(), Step::Interrupt(0x21));
    assert_eq!(cpu.regs.ax, 0x4c2a);
    assert_eq!(cpu.regs.ip, 0x105);
    assert_eq!(cpu.regs.sp, 0xfffe);
}

#[test]
fn alu_sets_flags_for_overflow_carry_and_zero() {
    // MOV AH,42h / MOV AL,7Fh / ADD AL,1 / STC / INC AL / SBB AL,80h
    let mut cpu = program(&[
        0xb4, 0x42, 0xb0, 0x7f, 0x04, 1, 0xf9, 0xfe, 0xc0, 0x1c, 0x80,
    ]);
    advance(&mut cpu, 3);
    assert_eq!(cpu.regs.ax, 0x4280);
    assert_eq!(cpu.regs.flags & (CF | OF | SF | AF), OF | SF | AF);
    advance(&mut cpu, 2);
    assert!(cpu.flag(CF));
    advance(&mut cpu, 1);
    assert_eq!(cpu.regs.ax, 0x4200);
    assert_eq!(cpu.regs.flags & (CF | OF | SF | AF | ZF | PF), ZF | PF);
}

#[test]
fn modrm_with_bp_defaults_to_ss_and_lea_ignores_the_segment() {
    // MOV BP,1234h / MOV AX,[BP+SI-2] / LEA DI,[BP+SI-2] / ES:MOV BX,[BP+SI-2]
    let mut cpu = program(&[
        0xbd, 0x34, 0x12, 0x8b, 0x42, 0xfe, 0x8d, 0x7a, 0xfe, 0x26, 0x8b, 0x5a, 0xfe,
    ]);
    cpu.regs.si = 3;
    cpu.write_u16(cpu.regs.ss, 0x1235, 0xbabe);
    cpu.write_u16(cpu.regs.es, 0x1235, 0x01ab);
    advance(&mut cpu, 4);
    assert_eq!(cpu.regs.ax, 0xbabe);
    assert_eq!(cpu.regs.di, 0x1235);
    assert_eq!(cpu.regs.bx, 0x01ab);
}

#[test]
fn repe_compare_stops_at_first_difference_in_reverse_order() {
    // STD / REPE CMPSB / HLT
    let mut cpu = program(&[0xfd, 0xf3, 0xa6, 0xf4]);
    cpu.regs.cx = 4;
    cpu.regs.si = 3;
    cpu.regs.di = 3;
    cpu.load(cpu.regs.ds, 0, b"abcd");
    cpu.load(cpu.regs.es, 0, b"axcd");
    advance(&mut cpu, 4);
    assert_eq!(cpu.regs.cx, 1);
    assert_eq!(cpu.regs.si, 0);
    assert_eq!(cpu.regs.di, 0);
    assert!(cpu.flag(CF));
    assert!(!cpu.flag(ZF));
    assert_eq!(cpu.step().unwrap(), Step::Halt);
}

#[test]
fn stack_far_call_and_interrupt_vectors_round_trip() {
    // CALL 4321h:100h / INT 60h / HLT
    let mut cpu = program(&[0x9a, 0, 1, 0x21, 0x43, 0xcd, 0x60, 0xf4]);
    // RETF at the far procedure. IRET at the interrupt vector.
    cpu.load(0x4321, 0x100, &[0xcb]);
    cpu.load(0x4321, 0x200, &[0xcf]);
    cpu.write_u16(0, 0x180, 0x200);
    cpu.write_u16(0, 0x182, 0x4321);
    cpu.regs.flags |= IF | CF;
    advance(&mut cpu, 1);
    assert_eq!(cpu.regs.cs, 0x4321);
    assert_eq!(cpu.regs.ip, 0x100);
    advance(&mut cpu, 1);
    assert_eq!(cpu.regs.sp, 0xfffe);
    assert_eq!(cpu.step().unwrap(), Step::Interrupt(0x60));
    cpu.enter_interrupt(0x60).unwrap();
    assert!(!cpu.flag(IF));
    assert_eq!(cpu.regs.cs, 0x4321);
    assert_eq!(cpu.regs.ip, 0x200);
    advance(&mut cpu, 1);
    assert!(cpu.flag(IF | CF));
    assert_eq!(cpu.regs.cs, 0x1200);
    assert_eq!(cpu.regs.sp, 0xfffe);
    assert_eq!(cpu.step().unwrap(), Step::Halt);
}

#[test]
fn wrapping_memory_and_32_bit_registers() {
    // MOV EAX,76543210h / MOV AL,ABh / MOV [DI],EAX / HLT
    let mut cpu = program(&[
        0x66, 0xb8, 0x10, 0x32, 0x54, 0x76, 0xb0, 0xab, 0x66, 0x89, 0x05, 0xf4,
    ]);
    cpu.regs.ds = 0xffff;
    cpu.regs.di = 0x000f;
    advance(&mut cpu, 3);
    assert_eq!(cpu.regs.ax, 0x32ab);
    assert_eq!(cpu.read_u8(0xffff, 0xf), 0xab);
    assert_eq!(cpu.read_u16(0, 0), 0x5432);
    assert_eq!(cpu.read_u8(0, 2), 0x76);
}

#[test]
fn port_requests_transfer_the_requested_width() {
    // IN AL,3Ch / OUT DX,AX
    let mut cpu = program(&[0xe4, 0x3c, 0xef]);
    cpu.regs.dx = 0x3c0;
    cpu.regs.ax = 0xab00;
    assert_eq!(
        cpu.step().unwrap(),
        Step::PortRead {
            port: 0x3c,
            width: 1
        }
    );
    cpu.complete_port_read(1, 0xf8);
    assert_eq!(
        cpu.step().unwrap(),
        Step::PortWrite {
            port: 0x3c0,
            width: 2,
            value: 0xabf8
        }
    );
}

#[test]
fn unsupported_instructions_retain_fault_address() {
    let mut cpu = program(&[0x0f, 0x0b]); // UD2
    let error = cpu.step().unwrap_err();
    assert_eq!(error.mnemonic, Mnemonic::Ud2);
    assert_eq!(error.ip, 0x100);
    assert_eq!(cpu.regs.ip, 0x100);
}

#[test]
fn rotate_by_a_full_operand_updates_carry() {
    // ROL AL,8 / CLC / ROR AL,8 / CLC / ROL AL,0
    let mut cpu = program(&[0xc0, 0xc0, 8, 0xf8, 0xc0, 0xc8, 8, 0xf8, 0xc0, 0xc0, 0]);
    cpu.regs.ax = 0x81;
    advance(&mut cpu, 1);
    assert!(cpu.flag(CF));
    advance(&mut cpu, 2);
    assert!(cpu.flag(CF));
    advance(&mut cpu, 2);
    assert!(!cpu.flag(CF));
    assert_eq!(cpu.regs.ax, 0x81);
}

#[derive(Clone)]
struct TestDevice(std::sync::Arc<std::sync::Mutex<[u8; 4]>>);

impl MemoryDevice for TestDevice {
    fn read(&self, offset: usize) -> u8 {
        self.0.lock().unwrap()[offset]
    }

    fn write(&self, offset: usize, value: u8) {
        self.0.lock().unwrap()[offset] = value;
    }
}

#[test]
fn memory_device_handles_string_and_boundary_accesses() {
    // MOVSW / MOVSW / MOV AX,[DI-3]
    let mut cpu = program(&[0xa5, 0xa5, 0x8b, 0x45, 0xfd]);
    let device = TestDevice(Default::default());
    cpu.regs.ds = 0xa000;
    cpu.regs.es = 0xa000;
    cpu.regs.si = 0;
    cpu.regs.di = 2;
    cpu.load(0xa000, 0, &[9, 8, 7, 6, 5, 4, 3]);
    assert!(cpu.map_memory(0xa0000, 4, Box::new(device.clone())));
    cpu.load(0xa000, 0, &[1, 2]);
    assert!(!cpu.map_memory(usize::MAX, 8, Box::new(device.clone())));
    advance(&mut cpu, 3);
    assert_eq!(*device.0.lock().unwrap(), [1, 2, 1, 2]);
    assert_eq!(cpu.regs.ax, 0x0102);
    cpu.write_u16(0xa000, 3, 0x8877);
    assert_eq!(cpu.read_u16(0xa000, 3), 0x8877);
    cpu.unmap_memory();
    assert_eq!(cpu.read_u16(0xa000, 3), 0x8806);
    assert_eq!(cpu.read_u16(0xa000, 0), 0x0809);
}

#[test]
fn wrapped_instruction_fetch_and_bus_access_find_the_mapping() {
    let mut cpu = Cpu::new();
    let device = TestDevice(Default::default());
    cpu.regs.cs = 0xffff;
    cpu.regs.ip = 0x000f;
    assert!(cpu.map_memory(0, 4, Box::new(device.clone())));
    cpu.write_u16(0xffff, 0x000f, 0x2ab0); // MOV AL,2Ah crosses A20.
    advance(&mut cpu, 1);
    assert_eq!(cpu.regs.ax, 0x2a);
    assert_eq!(cpu.regs.ip, 0x0011);
    assert_eq!(cpu.memory[0xfffff], 0xb0);
    assert_eq!(device.0.lock().unwrap()[0], 0x2a);
}

#[test]
fn cpu_profiles_reject_later_generations_before_execution() {
    // PUSHA was introduced with the 186.
    let mut early = program_for(CpuProfile::I8086, &[0x60]);
    let initial = early.regs;
    let error = early.step().unwrap_err();
    assert_eq!(error.mnemonic, Mnemonic::Pusha);
    assert_eq!(
        error.kind,
        CpuErrorKind::UnavailableOnProfile(CpuProfile::I8086)
    );
    assert_eq!(early.regs, initial);

    // PUSHA / PUSH FS, which requires a 386.
    let mut middle = program_for(CpuProfile::I286, &[0x60, 0x0f, 0xa0]);
    advance(&mut middle, 1);
    assert_eq!(middle.regs.sp, 0xffee);
    assert_eq!(
        middle.step().unwrap_err().kind,
        CpuErrorKind::UnavailableOnProfile(CpuProfile::I286)
    );
    assert_eq!(middle.regs.ip, 0x0101);
    assert_eq!(middle.regs.sp, 0xffee);

    let mut late = program(&[0x60, 0x0f, 0xa0, 0x0f, 0xa2]); // CPUID is newer than a 386.
    advance(&mut late, 2);
    assert_eq!(late.profile(), CpuProfile::I386);
    assert_eq!(
        late.step().unwrap_err().kind,
        CpuErrorKind::UnavailableOnProfile(CpuProfile::I386)
    );
}

#[test]
fn bit_tests_use_signed_memory_indices_and_report_the_previous_bit() {
    let mut cpu = program(&[
        0x0f, 0xa3, 0xc8, // BT AX,CX
        0x0f, 0xba, 0xf0, 31, // BTR AX,31
        0x0f, 0xab, 0x0e, 0, 2, // BTS WORD [0200h],CX
        0x66, 0x0f, 0xbb, 0x1e, 0, 2, // BTC DWORD [0200h],EBX
        0x0f, 0xba, 0x26, 0, 2, 33, // BT WORD [0200h],33
    ]);
    let other_flags = 2 | SF | ZF;
    cpu.regs.flags = other_flags;
    cpu.regs.ax = 0x8000;
    cpu.regs.cx = 0xffff;
    advance(&mut cpu, 1);
    assert_eq!(cpu.regs.ax, 0x8000);
    assert_eq!(cpu.regs.flags, other_flags | CF);
    advance(&mut cpu, 1);
    assert_eq!(cpu.regs.ax, 0);
    assert_eq!(cpu.regs.flags, other_flags | CF);

    cpu.regs.cx = 17;
    cpu.write_u16(cpu.regs.ds, 0x200, 2);
    advance(&mut cpu, 1);
    assert_eq!(cpu.read_u16(cpu.regs.ds, 0x200), 2);
    assert_eq!(cpu.read_u16(cpu.regs.ds, 0x202), 2);
    assert_eq!(cpu.regs.flags, other_flags);

    cpu.set_reg(Register::EBX, u32::MAX).unwrap();
    cpu.write_u16(cpu.regs.ds, 0x1fe, 0x8000);
    advance(&mut cpu, 1);
    assert_eq!(cpu.read_u16(cpu.regs.ds, 0x1fe), 0);
    assert_eq!(cpu.read_u16(cpu.regs.ds, 0x200), 2);
    assert_eq!(cpu.regs.flags, other_flags | CF);

    cpu.regs.flags = other_flags;
    advance(&mut cpu, 1);
    assert_eq!(cpu.read_u16(cpu.regs.ds, 0x200), 2);
    assert_eq!(cpu.regs.flags, other_flags | CF);

    let mut early = program_for(CpuProfile::I286, &[0x0f, 0xab, 0xc8]);
    let before = early.regs;
    assert_eq!(
        early.step().unwrap_err().kind,
        CpuErrorKind::UnavailableOnProfile(CpuProfile::I286)
    );
    assert_eq!(early.regs, before);
}

#[test]
fn double_shifts_use_the_source_and_preserve_state_for_a_zero_count() {
    let mut cpu = program(&[
        0x0f, 0xa4, 0xd8, 4, // SHLD AX,BX,4
        0x0f, 0xad, 0xd8, // SHRD AX,BX,CL
        0x66, 0x0f, 0xac, 0x1e, 0, 2, 1, // SHRD DWORD [0200h],EBX,1
        0x66, 0x0f, 0xa4, 0xd8, 33, // SHLD EAX,EBX,33
        0x66, 0x0f, 0xad, 0xd8, // SHRD EAX,EBX,CL
    ]);
    cpu.regs.ax = 0x8123;
    cpu.regs.bx = 0xf456;
    cpu.regs.flags = 2 | CF | SF | ZF;
    advance(&mut cpu, 1);
    assert_eq!(cpu.regs.ax, 0x123f);
    assert_eq!(cpu.regs.bx, 0xf456);
    assert_eq!(cpu.regs.flags, 2 | PF);

    cpu.regs.ax = 0x8123;
    cpu.regs.bx = 0x4567;
    cpu.regs.cx = 4;
    advance(&mut cpu, 1);
    assert_eq!(cpu.regs.ax, 0x7812);
    assert_eq!(cpu.regs.bx, 0x4567);
    assert_eq!(cpu.regs.flags, 2 | PF);

    cpu.set_reg(Register::EBX, 1).unwrap();
    cpu.load(cpu.regs.ds, 0x200, &[1, 0, 0, 0]);
    advance(&mut cpu, 1);
    assert_eq!(cpu.read_u16(cpu.regs.ds, 0x200), 0);
    assert_eq!(cpu.read_u16(cpu.regs.ds, 0x202), 0x8000);
    assert_eq!(cpu.get_reg(Register::EBX), Some(1));
    assert_eq!(cpu.regs.flags, 2 | OF | SF | PF | CF);

    cpu.set_reg(Register::EAX, 0x80000000).unwrap();
    cpu.set_reg(Register::EBX, 0).unwrap();
    advance(&mut cpu, 1);
    assert_eq!(cpu.get_reg(Register::EAX), Some(0));
    assert_eq!(cpu.regs.flags, 2 | OF | ZF | PF | CF);

    cpu.set_reg(Register::EAX, 0x12345678).unwrap();
    cpu.regs.cx = 32;
    let previous_flags = cpu.regs.flags;
    advance(&mut cpu, 1);
    assert_eq!(cpu.get_reg(Register::EAX), Some(0x12345678));
    assert_eq!(cpu.regs.flags, previous_flags);

    let mut early = program_for(CpuProfile::I286, &[0x0f, 0xad, 0xd8]);
    let before = early.regs;
    assert_eq!(
        early.step().unwrap_err().kind,
        CpuErrorKind::UnavailableOnProfile(CpuProfile::I286)
    );
    assert_eq!(early.regs, before);
}

#[test]
fn setcc_writes_a_single_byte_without_modifying_flags() {
    for (flags, outcomes) in [
        (2, 0xaaaa),
        (2 | OF | SF | ZF | PF | CF, 0x6555),
        (2 | SF, 0x59aa),
    ] {
        for condition in 0..16 {
            let opcode = 0x90 + condition;
            let mut cpu = program(&[0x0f, opcode, 0xc4, 0x0f, opcode, 0x06, 0, 2]);
            cpu.regs.flags = flags;
            cpu.regs.ax = 0xa55a;
            cpu.load(cpu.regs.ds, 0x1ff, &[0x55, 0xa5, 0xaa]);
            advance(&mut cpu, 2);

            let expected = u8::from(outcomes & (1 << condition) != 0);
            assert_eq!(cpu.regs.ax, u16::from_le_bytes([0x5a, expected]));
            assert_eq!(cpu.read_u8(cpu.regs.ds, 0x1ff), 0x55);
            assert_eq!(cpu.read_u8(cpu.regs.ds, 0x200), expected);
            assert_eq!(cpu.read_u8(cpu.regs.ds, 0x201), 0xaa);
            assert_eq!(cpu.regs.flags, flags);
        }
    }
    for profile in [CpuProfile::I8086, CpuProfile::I286] {
        let mut cpu = program_for(profile, &[0x0f, 0x95, 0xc4]);
        let before = cpu.regs;
        assert_eq!(
            cpu.step().unwrap_err().kind,
            CpuErrorKind::UnavailableOnProfile(profile)
        );
        assert_eq!(cpu.regs, before);
    }
}

#[test]
fn cpu_profiles_gate_prefixes_even_when_the_decoder_can_discard_them() {
    for profile in [CpuProfile::I8086, CpuProfile::I286, CpuProfile::I386] {
        // MOV AL,66h and REP NOP are valid on every profile. Each remaining
        // encoding has a segment, operand, or address prefix from the 386.
        for prefix in [0x64, 0x65, 0x66, 0x67] {
            let mut cpu = program_for(profile, &[0xb0, 0x66, 0xf3, 0x90, 0x26, prefix, 0x90]);
            advance(&mut cpu, 2);
            assert_eq!(cpu.regs.ax, 0x66);
            if profile == CpuProfile::I386 {
                advance(&mut cpu, 1);
            } else {
                assert_eq!(
                    cpu.step().unwrap_err().kind,
                    CpuErrorKind::UnavailableOnProfile(profile)
                );
                assert_eq!(cpu.regs.ip, 0x0104);
            }
        }
    }
}

#[test]
fn cpu_profiles_expose_different_real_mode_flags_and_stack_behavior() {
    for (profile, initial, after_popf, pushed_sp, shifted_ax) in [
        (CpuProfile::I8086, 0xf002, 0xffd7, 0xfffc, 0),
        (CpuProfile::I286, 0x0002, 0x0fd7, 0xfffe, 1),
        (CpuProfile::I386, 0x0002, 0x7fd7, 0xfffe, 1),
    ] {
        // PUSHF / POP BX / PUSH AX / POPF / PUSHF / POP DX / PUSH SP / SHL AX,CL
        let mut cpu = program_for(
            profile,
            &[0x9c, 0x5b, 0x50, 0x9d, 0x9c, 0x5a, 0x54, 0xd3, 0xe0],
        );
        cpu.regs.ax = 0xffff;
        cpu.regs.cx = 32;
        advance(&mut cpu, 7);
        assert_eq!(cpu.regs.bx, initial);
        assert_eq!(cpu.regs.dx, after_popf);
        assert_eq!(cpu.read_u16(cpu.regs.ss, cpu.regs.sp), pushed_sp);
        cpu.regs.ax = 1;
        advance(&mut cpu, 1);
        assert_eq!(cpu.regs.ax, shifted_ax);
    }
}

#[test]
fn supported_cpu_generation_can_still_report_unimplemented_instructions() {
    let mut cpu = program_for(CpuProfile::I286, &[0x62, 0x06, 0, 2]); // BOUND AX,[0200h]
    let error = cpu.step().unwrap_err();
    assert_eq!(error.mnemonic, Mnemonic::Bound);
    assert_eq!(error.kind, CpuErrorKind::Unsupported);
    assert_eq!(cpu.regs.ip, 0x0100);
}

#[test]
fn sti_delays_irqs_only_when_enabling_if() {
    // CLI / STI / NOP / STI / CLI
    let mut cpu = program(&[0xfa, 0xfb, 0x90, 0xfb, 0xfa]);
    advance(&mut cpu, 2);
    assert!(cpu.flag(IF));
    assert!(!cpu.can_accept_maskable_interrupt());
    advance(&mut cpu, 1);
    assert!(cpu.can_accept_maskable_interrupt());
    advance(&mut cpu, 1);
    assert!(cpu.can_accept_maskable_interrupt());
    advance(&mut cpu, 1);
    assert!(!cpu.can_accept_maskable_interrupt());
}

#[test]
fn mov_and_pop_ss_protect_the_next_stack_instruction() {
    // MOV SS,AX / MOV SP,0200h / POP SS / MOV SP,0100h
    let mut cpu = program(&[0x8e, 0xd0, 0xbc, 0x00, 0x02, 0x17, 0xbc, 0x00, 0x01]);
    cpu.regs.flags |= IF;
    cpu.regs.ax = 0x2000;
    cpu.write_u16(0x2000, 0x0200, 0x2400);

    advance(&mut cpu, 1);
    assert_eq!(cpu.regs.ss, 0x2000);
    assert!(!cpu.can_accept_maskable_interrupt());
    advance(&mut cpu, 1);
    assert_eq!(cpu.regs.sp, 0x0200);
    assert!(cpu.can_accept_maskable_interrupt());
    advance(&mut cpu, 1);
    assert_eq!(cpu.regs.ss, 0x2400);
    assert!(!cpu.can_accept_maskable_interrupt());
    advance(&mut cpu, 1);
    assert_eq!(cpu.regs.sp, 0x0100);
    assert!(cpu.can_accept_maskable_interrupt());
}

#[test]
fn hlt_stops_instruction_fetch_until_an_interrupt_returns_to_the_next_ip() {
    // STI / HLT / MOV AX,1234h
    let mut cpu = program(&[0xfb, 0xf4, 0xb8, 0x34, 0x12]);
    cpu.load(0x4321, 0x200, &[0xcf]); // IRET
    cpu.write_u16(0, 32, 0x200);
    cpu.write_u16(0, 34, 0x4321);

    advance(&mut cpu, 1);
    assert!(!cpu.can_accept_maskable_interrupt());
    assert_eq!(cpu.step().unwrap(), Step::Halt);
    assert!(cpu.can_accept_maskable_interrupt());
    assert!(cpu.is_halted());
    assert_eq!(cpu.step().unwrap(), Step::Halt);
    assert_eq!(cpu.regs.ip, 0x0102);
    assert_eq!(cpu.regs.ax, 0);

    cpu.enter_interrupt(8).unwrap();
    assert!(!cpu.is_halted());
    assert!(!cpu.can_accept_maskable_interrupt());
    assert_eq!(cpu.read_u16(cpu.regs.ss, cpu.regs.sp), 0x0102);
    advance(&mut cpu, 2);
    assert_eq!(cpu.regs.ax, 0x1234);
    assert_eq!(cpu.regs.sp, 0xfffe);
    assert!(cpu.can_accept_maskable_interrupt());
}

#[test]
fn protected_mode_descriptor_tables_round_trip_by_profile() {
    let bios = [
        0x0f, 1, 0x16, 0, 2, // LGDT [0200h]
        0x0f, 1, 0x06, 8, 2, // SGDT [0208h]
        0x0f, 1, 0x1e, 0, 2, // LIDT [0200h]
        0x0f, 1, 0x0e, 0x10, 2, // SIDT [0210h]
    ];
    for (profile, upper) in [(CpuProfile::I286, 0xff), (CpuProfile::I386, 0)] {
        let mut cpu = program_for(profile, &bios);
        cpu.load(cpu.regs.ds, 0x200, &[0x37, 0, 0x11, 0x22, 0x33, 0x44]);
        advance(&mut cpu, 4);
        for offset in [0x208, 0x210] {
            let bytes = std::array::from_fn::<_, 6, _>(|byte| {
                cpu.read_u8(cpu.regs.ds, offset + byte as u16)
            });
            assert_eq!(bytes, [0x37, 0, 0x11, 0x22, 0x33, upper]);
        }
    }
    let mut cpu = program_for(CpuProfile::I8086, &bios);
    assert_eq!(
        cpu.step().unwrap_err().kind,
        CpuErrorKind::UnavailableOnProfile(CpuProfile::I8086)
    );

    let mut cpu = program(&[
        0x66, 0x0f, 1, 0x16, 0, 2, // 32-bit LGDT [0200h]
        0x0f, 1, 0x06, 8, 2, // SGDT still stores all 32 base bits
    ]);
    cpu.load(cpu.regs.ds, 0x200, &[0x37, 0, 0x11, 0x22, 0x33, 0x44]);
    advance(&mut cpu, 2);
    assert_eq!(cpu.read_u8(cpu.regs.ds, 0x20d), 0x44);
}

#[test]
fn protected_mode_286_keeps_pe_set_and_enforces_segment_bounds() {
    let mut cpu = program_for(
        CpuProfile::I286,
        &[
            0xfa, 0x0f, 1, 0x16, 0, 2, // CLI; LGDT [0200h]
            0xb8, 1, 0, 0x0f, 1, 0xf0, // LMSW AX with PE set
            0xea, 0, 0, 8, 0, // JMP 0008:0000
        ],
    );
    cpu.load(cpu.regs.ds, 0x200, &[0x17, 0, 0, 0, 2, 0]);
    cpu.load(
        0x2000,
        0,
        &[
            0, 0, 0, 0, 0, 0, 0, 0, 0x20, 0, 0, 0x10, 2, 0x9a, 0, 0, // Code at 21000h.
            0x03, 0, 0, 0x20, 2, 0x92, 0, 0, // Data at 22000h, four bytes.
        ],
    );
    cpu.load(
        0x2100,
        0,
        &[
            0x31, 0xc0, 0x0f, 1, 0xf0, 0x0f, 1, 0xe0, // Try to clear PE and read it back.
            0xbb, 0x10, 0, 0x8e, 0xdb, // DS = 0010h.
            0xc6, 0x06, 3, 0, 0x7a, // Write the last valid byte.
            0xc7, 0x06, 3, 0, 0xcd, 0xab, // Word at same offset crosses the limit.
        ],
    );
    advance(&mut cpu, 11);
    assert!(cpu.protected_mode());
    assert_eq!(cpu.regs.ax & 1, 1);
    assert_eq!(cpu.regs.cs, 8);
    assert_eq!(cpu.regs.ds, 0x10);
    assert_eq!(cpu.read_u8(0x2200, 3), 0x7a);
    let fault_ip = cpu.regs.ip;
    assert!(cpu.step().is_err());
    assert_eq!(cpu.regs.ip, fault_ip);
    assert_eq!(cpu.read_u16(0x2200, 3), 0x007a);
}

#[test]
fn a20_masks_only_bit_twenty_and_handles_multibyte_access_at_the_boundary() {
    for profile in [CpuProfile::I286, CpuProfile::I386] {
        let mut cpu = Cpu::with_profile(profile);
        cpu.memory[0x100000] = 0x78;
        cpu.write_u16(0xffff, 0x0f, 0x1234);
        assert_eq!(cpu.memory[0xfffff], 0x34);
        assert_eq!(cpu.memory[0], 0x12);
        assert_eq!(cpu.read_u16(0xffff, 0x0f), 0x1234);

        cpu.memory[0x200000] = 0x45;
        cpu.memory[0x300000] = 0x67;
        assert_eq!(cpu.read_byte(0x200000), 0x45);
        assert_eq!(cpu.read_byte(0x300000), 0x45);

        cpu.set_a20(true);
        assert_eq!(cpu.read_u16(0xffff, 0x0f), 0x7834);
        assert_eq!(cpu.read_byte(0x300000), 0x67);
        cpu.write_u16(0xffff, 0x0f, 0xabcd);
        assert_eq!(cpu.memory[0xfffff], 0xcd);
        assert_eq!(cpu.memory[0x100000], 0xab);
        assert_eq!(cpu.memory[0], 0x12);
    }
    let mut cpu = Cpu::with_profile(CpuProfile::I8086);
    cpu.set_a20(true);
    assert!(!cpu.a20_enabled());
    cpu.write_u8(0xffff, 0x10, 0x55);
    assert_eq!(cpu.memory[0], 0x55);
}

#[test]
fn instruction_fetch_crosses_the_a20_gate_byte_by_byte() {
    for (enabled, expected) in [(false, 0x1234), (true, 0x5678)] {
        let mut cpu = Cpu::new();
        cpu.regs.cs = 0xffff;
        cpu.regs.ip = 0x000f;
        cpu.memory[0xfffff] = 0xb8;
        cpu.memory[..2].copy_from_slice(&[0x34, 0x12]);
        cpu.memory[0x100000..0x100002].copy_from_slice(&[0x78, 0x56]);
        cpu.set_a20(enabled);
        advance(&mut cpu, 1);
        assert_eq!(cpu.regs.ax, expected);
    }
}

#[test]
fn protected_mode_loads_descriptors_and_executes_from_extended_ram() {
    let mut cpu = program(&[
        0xfa, 0x0f, 1, 0x16, 0, 2, // CLI; LGDT.
        0xb8, 1, 0, 0x0f, 1, 0xf0, // Enable PE.
        0xea, 0, 0, 8, 0, // JMP 0008:0000.
    ]);
    cpu.set_a20(true);
    cpu.load(cpu.regs.ds, 0x200, &[0x17, 0, 0, 0, 0x30, 0]);
    cpu.memory[0x300008..0x300018].copy_from_slice(&[
        0xff, 0, 0, 0, 0x31, 0x9a, 0, 0, // Code at 310000h.
        0xff, 0, 0, 0, 0x24, 0x92, 0, 0, // Data at 240000h.
    ]);
    cpu.memory[0x310000..0x31000b].copy_from_slice(&[
        0xb8, 0x10, 0, 0x8e, 0xd8, // DS=0010h.
        0xc7, 0x06, 0x30, 0, 0xef, 0xbe,
    ]);
    advance(&mut cpu, 8);
    assert!(cpu.protected_mode());
    assert_eq!(&cpu.memory[0x240030..0x240032], &[0xef, 0xbe]);
    assert_eq!(cpu.read_byte(0x20000000), 0xff);
}

#[test]
fn i386_paging_restarts_a_cross_page_write_after_the_guest_installs_the_page() {
    let mut cpu = program(&[
        0xfa, 0x0f, 1, 0x16, 0, 2, 0x0f, 1, 0x1e, 6, 2, // CLI; LGDT; LIDT.
        0xb8, 1, 0, 0x0f, 1, 0xf0, // LMSW AX.
        0x66, 0xea, 0, 0x80, 1, 0, 8, 0, // JMP 0008:00018000h.
    ]);
    cpu.set_a20(true);
    cpu.load(
        cpu.regs.ds,
        0x200,
        &[0x17, 0, 0, 0x40, 1, 0, 0x77, 0, 0, 0x60, 1, 0],
    );
    cpu.load(
        0x1400,
        8,
        &[
            0xff, 0xff, 0, 0, 0, 0x9a, 0xcf, 0, // Flat 32-bit code.
            0xff, 0xff, 0, 0, 0, 0x92, 0xcf, 0, // Flat 32-bit data and stack.
        ],
    );
    cpu.load(0x1600, 14 * 8, &[0, 0x81, 8, 0, 0, 0x8e, 1, 0]);
    cpu.load(
        0x1800,
        0,
        &[
            0x66, 0xb8, 0x10, 0, 0x8e, 0xd8, 0x8e, 0xd0, // DS and SS.
            0xbc, 0, 0, 2, 0, // ESP=00020000h.
            0xb8, 0, 0, 5, 0, 0x0f, 0x22, 0xd8, // CR3=50000h.
            0x0f, 0x20, 0xc0, 0x0d, 0, 0, 0, 0x80, 0x0f, 0x22, 0xc0, // Enable PG.
            0xa1, 0, 0, 0x40, 0, // Read virtual 400000h, backed by physical 180000h.
            0xc7, 0x05, 0xfe, 0x0f, 0x40, 0, 0x11, 0x22, 0x33, 0x44, // Cross-page write.
            0xf4,
        ],
    );
    cpu.load(
        0x1800,
        0x100,
        &[
            0x0f, 0x20, 0xd3, // MOV EBX,CR2.
            0x59, // POP ECX (page-fault error code).
            0x8b, 0x14, 0x24, // MOV EDX,[ESP] (faulting EIP).
            0xc7, 0x05, 4, 0x20, 5, 0, 3, 0, 0x1a, 0,    // Map page 401000h.
            0xcf, // IRETD.
        ],
    );
    cpu.write_mem(0x50000, 4, 0x51003); // Identity map 0..4 MiB.
    cpu.write_mem(0x50004, 4, 0x52003);
    for page in 0..1024 {
        cpu.write_mem(0x51000 + page * 4, 4, ((page as u32) << 12) | 3);
    }
    cpu.write_mem(0x52000, 4, 0x180001); // Ring 0 can write a read-only page on the 386.
    cpu.write_mem(0x180000, 4, 0xdeed_cafe);

    let mut saw_fault = false;
    for _ in 0..96 {
        let step = cpu.step().unwrap();
        if cpu.instruction_pointer() == 0x18100 {
            assert!(!saw_fault);
            saw_fault = true;
            assert_eq!(cpu.stack_pointer(), 0x1fff0);
            assert_eq!(&cpu.memory[0x180ffe..0x181000], &[0, 0]);
        }
        if step == Step::Halt {
            break;
        }
    }
    assert!(saw_fault && cpu.is_halted());
    assert_eq!(cpu.get_reg(Register::EAX), Some(0xdeed_cafe));
    assert_eq!(cpu.get_reg(Register::EBX), Some(0x401000));
    assert_eq!(cpu.get_reg(Register::ECX), Some(2));
    assert_eq!(cpu.get_reg(Register::EDX), Some(0x18025));
    assert_eq!(cpu.stack_pointer(), 0x20000);
    assert_eq!(&cpu.memory[0x180ffe..0x181000], &[0x11, 0x22]);
    assert_eq!(&cpu.memory[0x1a0000..0x1a0002], &[0x33, 0x44]);
    assert_eq!(cpu.read_mem(0x52000, 4) & 0x60, 0x60);
    assert_eq!(cpu.read_mem(0x52004, 4) & 0x60, 0x60);
    assert_eq!(cpu.read_mem(0x50004, 4) & 0x20, 0x20);

    let snapshot = cpu.save_snapshot().unwrap();
    let mut restored = Cpu::new();
    restored.restore_snapshot(&snapshot).unwrap();
    assert!(restored.protected_mode() && restored.paging());
    assert_eq!(restored.control.cr2, 0x401000);
    assert_eq!(restored.control.cr3, 0x50000);
    assert_eq!(restored.read_linear(0x400ffe, 4), Some(0x44332211));
    assert_eq!(restored.instruction_pointer(), cpu.instruction_pointer());
    assert_eq!(restored.stack_pointer(), cpu.stack_pointer());
}

#[test]
fn i386_paging_faults_only_when_fetch_needs_bytes_on_the_unmapped_page() {
    let mut cpu = program(&[]);
    cpu.regs.ip = 0x0fff;
    cpu.control.cr3 = 0x50000;
    cpu.write_cr0(0x8000_0001).unwrap();
    cpu.write_mem(0x50000, 4, 0x51003);
    cpu.write_mem(0x51000 + 0x12 * 4, 4, 0x25003);
    cpu.memory[0x25fff] = 0x90;
    advance(&mut cpu, 1);
    assert_eq!(cpu.control.cr2, 0);
    let fault = cpu.step().unwrap_err();
    assert_eq!(fault.ip, 0x1000);
    assert_eq!(
        fault.kind,
        CpuErrorKind::PageFault {
            linear: 0x13000,
            write: false
        }
    );
    assert_eq!(cpu.control.cr2, 0x13000);
}

#[test]
fn protected_mode_segment_faults_push_error_codes_and_resume_guest_execution() {
    let mut cpu = program_for(
        CpuProfile::I286,
        &[
            0xfa, 0x0f, 1, 0x16, 0, 2, 0x0f, 1, 0x1e, 6, 2, // CLI; LGDT; LIDT.
            0xb8, 1, 0, 0x0f, 1, 0xf0, // Enter protected mode.
            0xea, 0, 0, 8, 0,
        ],
    );
    cpu.load(
        cpu.regs.ds,
        0x200,
        &[0x27, 0, 0, 0x71, 1, 0, 0x6f, 0, 0, 0x60, 1, 0],
    );
    cpu.load(
        0x1700,
        0x108,
        &[
            0xff, 0, 0, 0x80, 1, 0x9a, 0, 0, // Code at 18000h.
            0xff, 2, 0, 0x70, 1, 0x92, 0, 0, // Writable data and stack at 17000h.
            0xff, 0, 0, 0x90, 1, 0x12, 0, 0, // Initially absent, at 19000h.
            0, 3, 0, 0x70, 1, 0x96, 0, 0, // Expand-down stack starts above 0300h.
        ],
    );
    for (vector, offset) in [(11, 0x40), (12, 0x80), (13, 0x60)] {
        cpu.load(0x1600, vector * 8, &[offset, 0, 8, 0, 0, 0x86, 0, 0]);
    }
    cpu.load(
        0x1800,
        0,
        &[
            0xb8, 0x10, 0, 0x8e, 0xc0, 0x8e, 0xd0, 0xbc, 0, 3, // ES=SS=0010h.
            0xb8, 0x18, 0, 0x8e, 0xd8, // MOV DS,AX faults with #NP.
            0xa1, 0xff, 0, 0x89, 0xc2, // Read crosses limit, then succeeds after #GP.
            0xb8, 0x20, 0, 0x8e, 0xd0, 0xbc, 0, 4, // Load the expand-down stack.
            0x36, 0xa0, 0, 3, // SS:0300h lies below the expand-down range.
            0xf4,
        ],
    );
    cpu.load(0x1900, 0xff, &[0xef, 0xbe]);
    cpu.load(
        0x1800,
        0x40,
        &[
            0x5b, 0x26, 0x89, 0x1e, 0, 2, // Save the selector from the #NP frame.
            0x26, 0xc6, 0x06, 0x1d, 1, 0x92, // Mark the data descriptor present.
            0xcf,
        ],
    );
    cpu.load(
        0x1800,
        0x60,
        &[
            0x5b, 0x26, 0x89, 0x1e, 2, 2, // Save the #GP error code.
            0x26, 0xc6, 0x06, 0x19, 1, 1, // Grow the limit.
            0xb8, 0x18, 0, 0x8e, 0xd8, // Reload DS to refresh the cached descriptor.
            0xcf,
        ],
    );
    cpu.load(
        0x1800,
        0x80,
        &[
            0x5b, 0x26, 0x89, 0x1e, 4, 2, // Save the #SS error code.
            0x58, 0x83, 0xc0, 4, 0x50, // Skip the faulting access.
            0xcf,
        ],
    );
    for offset in [0x200, 0x202, 0x204] {
        cpu.write_u16(0x1700, offset, 0xffff);
    }
    for _ in 0..100 {
        if cpu.step().unwrap() == Step::Halt {
            break;
        }
    }
    assert!(cpu.is_halted());
    assert_eq!(cpu.regs.dx, 0xbeef);
    assert_eq!(cpu.regs.ss, 0x20);
    assert_eq!(cpu.regs.sp, 0x400);
    assert_eq!(cpu.read_u16(0x1700, 0x200), 0x18);
    assert_eq!(cpu.read_u16(0x1700, 0x202), 0);
    assert_eq!(cpu.read_u16(0x1700, 0x204), 0);
}

#[test]
fn task_register_load_marks_the_descriptor_busy_and_survives_a_snapshot() {
    for (profile, limit, access) in [
        (CpuProfile::I286, 0x2b, 0x81),
        (CpuProfile::I386, 0x67, 0x89),
    ] {
        let mut cpu = program_for(
            profile,
            &[
                0xfa, 0x0f, 1, 0x16, 0, 2, // CLI; LGDT [0200h].
                0xb8, 9, 0, 0x0f, 1, 0xf0, 0xea, 0, 0, 8, 0, // Set PE and TS, then far jump.
            ],
        );
        cpu.load(cpu.regs.ds, 0x200, &[0x1f, 0, 0, 0x40, 1, 0]);
        cpu.load(
            0x1400,
            8,
            &[
                0xff, 0, 0, 0x80, 1, 0x9a, 0, 0, // Code at 18000h.
                0xff, 0, 0, 0xa0, 1, 0x92, 0, 0, // Stack at 1A000h.
                limit, 0, 0, 0xc0, 1, access, 0, 0, // Available TSS at 1C000h.
            ],
        );
        cpu.load(0, 13 * 8, &[0x60, 0, 8, 0, 0, 0x86, 0, 0]);
        cpu.load(
            0x1800,
            0,
            &[
                0xba, 0x10, 0, 0x8e, 0xd2, 0xbc, 0xf0, 0, // Initialize SS:SP.
                0xb8, 0x18, 0, 0x0f, 0, 0xd8, // LTR AX.
                0x0f, 0x06, // CLTS.
                0x0f, 0, 0xcb, // STR BX.
                0x0f, 0, 0xdb, // LTR BX faults because the TSS is busy.
                0xf4,
            ],
        );
        cpu.load(
            0x1800,
            0x60,
            &[0x5d, 0x58, 0x83, 0xc0, 3, 0x50, 0xcf], // Capture error, skip LTR, IRET.
        );
        advance(&mut cpu, 9);
        let legacy = cpu.save_snapshot().unwrap();
        assert_eq!(&legacy[8..10], &[1, 0]);
        let flags = cpu.regs.flags;
        advance(&mut cpu, 1);
        assert_eq!(cpu.regs.flags, flags);
        assert_eq!(cpu.read_u8(0x1400, 0x1d), access | 2);
        let state = cpu.save_snapshot().unwrap();
        assert_eq!(&state[8..10], &[2, 0]);

        let mut restored = Cpu::with_profile(profile);
        restored.restore_snapshot(&legacy).unwrap();
        assert!(restored.control.task_register.is_none());
        restored.restore_snapshot(&state).unwrap();
        assert!(restored.control.task_register.is_some());
        assert_eq!(restored.control.task_register, cpu.control.task_register);
        assert_ne!(restored.control.cr0 & 8, 0);
        advance(&mut restored, 1); // CLTS.
        assert_eq!(restored.control.cr0 & 8, 0);
        assert_eq!(restored.regs.flags, flags);
        for _ in 0..16 {
            if restored.step().unwrap() == Step::Halt {
                break;
            }
        }
        assert!(restored.is_halted());
        assert_eq!(restored.regs.bx, 0x18);
        assert_eq!(restored.regs.bp, 0x18);
        assert_eq!(restored.regs.sp, 0xf0);
        assert_eq!(restored.read_u8(0x1400, 0x1d), access | 2);
    }
}

#[test]
fn iretd_enters_virtual_8086_and_restores_all_guest_segments() {
    let mut cpu = program_for(
        CpuProfile::I386,
        &[
            0xfa, 0x0f, 1, 0x16, 0, 2, // CLI; LGDT [0200h].
            0xb8, 1, 0, 0x0f, 1, 0xf0, 0xea, 0, 0, 8, 0, // Set PE and far jump.
        ],
    );
    cpu.load(cpu.regs.ds, 0x200, &[0x17, 0, 0, 0x40, 1, 0]);
    cpu.load(
        0x1400,
        8,
        &[
            0xff, 0, 0, 0x80, 1, 0x9a, 0, 0, // Code at 18000h.
            0xff, 0, 0, 0xa0, 1, 0x92, 0, 0, // Stack at 1A000h.
        ],
    );
    cpu.load(
        0x1800,
        0,
        &[0xb8, 0x10, 0, 0x8e, 0xd0, 0xbc, 0xc0, 0, 0x66, 0xcf],
    );
    let frame: [u32; 9] = [
        0x20,
        0x1902,
        0x0002_3002,
        0xbeef_0080,
        0x1b03,
        0x1c04,
        0x1d05,
        0x1e06,
        0x1f07,
    ];
    for (index, word) in frame.into_iter().enumerate() {
        cpu.load(0x1a00, 0xc0 + index as u16 * 4, &word.to_le_bytes());
    }
    cpu.load(
        0x1902,
        0x20,
        &[
            0x8b, 0x1e, 0x10, 0, // MOV BX,[0010h].
            0xb8, 0x34, 0x12, 0x50, 0x5a, // PUSH AX; POP DX.
            0xb8, 0, 0, 0x50, 0x9d, // POPF preserves IOPL.
            0x8e, 0xd8, // MOV DS,AX uses the virtual segment base.
            0xea, 0x20, 0, 0x01, 0x21, // Far JMP to another virtual segment.
        ],
    );
    cpu.load(0x2101, 0x20, &[0x67, 0xa1, 0, 0, 1, 0]); // Read beyond the virtual segment.
    cpu.write_u16(0x1d05, 0x10, 0xabcd);

    advance(&mut cpu, 8);
    let old = cpu.save_snapshot().unwrap();
    assert_eq!(&old[8..10], &[1, 0]);
    cpu.load(0x1a00, 0xc0, &0x10000u32.to_le_bytes());
    assert_eq!(
        cpu.step().unwrap_err().kind,
        CpuErrorKind::SegmentFault {
            vector: 13,
            code: 0
        }
    );
    assert!(!cpu.control.virtual_8086);
    assert_eq!((cpu.regs.cs, cpu.regs.ss, cpu.regs.sp), (8, 0x10, 0xc0));

    cpu.restore_snapshot(&old).unwrap();
    advance(&mut cpu, 1);
    assert!(cpu.protected_mode() && cpu.control.virtual_8086);
    assert_eq!(cpu.instruction_pointer(), 0x20);
    assert_eq!(cpu.get_reg(Register::ESP), Some(0xbeef_0080));
    assert_eq!(
        [
            Register::CS,
            Register::SS,
            Register::ES,
            Register::DS,
            Register::FS,
            Register::GS
        ]
        .map(|register| cpu.get_reg(register).unwrap()),
        [0x1902, 0x1b03, 0x1c04, 0x1d05, 0x1e06, 0x1f07]
    );
    assert_eq!(cpu.regs.flags, 0x3002);
    let state = cpu.save_snapshot().unwrap();
    assert_eq!(&state[8..10], &[3, 0]);

    let mut restored = Cpu::with_profile(CpuProfile::I386);
    restored.restore_snapshot(&state).unwrap();
    assert!(restored.control.virtual_8086);
    advance(&mut restored, 9);
    assert_eq!(restored.regs.bx, 0xabcd);
    assert_eq!(restored.regs.dx, 0x1234);
    assert_eq!(restored.read_u16(0x1b03, 0x7e), 0);
    assert_eq!(restored.get_reg(Register::ESP), Some(0xbeef_0080));
    assert_eq!(restored.regs.flags & 0x3000, 0x3000);
    assert_eq!((restored.regs.cs, restored.regs.ds), (0x2101, 0));
    assert_eq!(
        restored.step().unwrap_err().kind,
        CpuErrorKind::SegmentFault {
            vector: 13,
            code: 0
        }
    );
    assert_eq!(restored.instruction_pointer(), 0x20);
    restored.load(0x2101, 0x20, &[0xe4, 0x61]); // IN AL,61h requires an I/O bitmap.
    assert_eq!(
        restored.step().unwrap_err().kind,
        CpuErrorKind::SegmentFault {
            vector: 13,
            code: 0
        }
    );
    assert_eq!(restored.instruction_pointer(), 0x20);
}

#[test]
fn protected_mode_local_descriptors_are_selectable_and_queryable_on_286_and_386() {
    for profile in [CpuProfile::I286, CpuProfile::I386] {
        let mut cpu = program_for(
            profile,
            &[
                0xfa, 0x0f, 1, 0x16, 0, 2, 0x0f, 1, 0x1e, 6, 2, // LGDT and LIDT.
                0xb8, 1, 0, 0x0f, 1, 0xf0, 0xea, 0, 0, 8, 0, // Enter protected mode.
            ],
        );
        cpu.load(
            cpu.regs.ds,
            0x200,
            &[0x1f, 0, 0, 0x40, 1, 0, 0x6f, 0, 0, 0x60, 1, 0],
        );
        cpu.load(
            0x1400,
            8,
            &[
                0xff, 0, 0, 0x80, 1, 0x9a, 0, 0, // Code at 18000h.
                0xff, 0, 0, 0xa0, 1, 0x92, 0, 0, // GDT data and stack.
                7, 0, 0, 0x70, 1, 0x82, 0, 0, // LDT at 17000h, just one entry.
            ],
        );
        cpu.load(0x1600, 13 * 8, &[0x70, 0, 8, 0, 0, 0x86, 0, 0]);
        cpu.load(0x1700, 0, &[0x3f, 0, 0, 0x90, 1, 0x92, 0, 0]);
        cpu.write_u16(0x1900, 0x20, 0xbabe);
        cpu.load(
            0x1800,
            0,
            &[
                0xb8, 0x10, 0, 0x8e, 0xd0, 0xbc, 0xf0, 0, // SS=0010h, SP=00F0h.
                0xb8, 0x18, 0, 0x0f, 0, 0xd0, 0x0f, 0, 0xc3, // LLDT AX; SLDT BX.
                0xba, 4, 0, 0x8e, 0xda, // DS=0004h is the first LDT entry.
                0x0f, 2, 0xf2, 0x0f, 3, 0xfa, // LAR SI,DX; LSL DI,DX.
                0x0f, 0, 0xe2, // VERR DX.
                0x0f, 0, 0xea, // VERW DX.
                0x31, 0xc0, 0x0f, 0, 0xd0, // LLDT AX with a null selector.
                0x8b, 0x0e, 0x20, 0, // Read through the cached DS after disabling the LDT.
                0x0f, 3, 0xfa, // LSL DI,DX fails and preserves DI.
                0x8e, 0xc2, // MOV ES,DX faults because LDTR is no longer usable.
                0xf4,
            ],
        );
        cpu.load(
            0x1800,
            0x70,
            &[
                0x5d, 0x58, 0x83, 0xc0, 2, 0x50, 0xcf, // Capture error, skip MOV ES, IRET.
            ],
        );

        advance(&mut cpu, 16); // Through LAR and LSL.
        assert!(cpu.flag(ZF));
        advance(&mut cpu, 1); // VERR.
        assert!(cpu.flag(ZF));
        advance(&mut cpu, 1); // VERW.
        assert!(cpu.flag(ZF));
        advance(&mut cpu, 4); // Disable the LDT and probe it again.
        assert!(!cpu.flag(ZF));
        assert_eq!(cpu.regs.di, 0x3f);
        for _ in 0..32 {
            if cpu.step().unwrap() == Step::Halt {
                break;
            }
        }
        assert!(cpu.is_halted());
        assert_eq!(cpu.regs.bx, 0x18);
        assert_eq!(cpu.regs.cx, 0xbabe);
        assert_eq!(cpu.regs.si, 0x9200);
        assert_eq!(cpu.regs.di, 0x3f);
        assert_eq!(cpu.regs.bp, 4);
        assert_eq!(cpu.regs.sp, 0xf0);
        assert_eq!(cpu.regs.es, 0x1400);
    }
}

#[test]
fn cpu_snapshot_restores_a_halted_guest_and_extended_ram() {
    let mut cpu = program(&[0xfb, 0x66, 0xb8, 0x78, 0x56, 0x34, 0x12, 0xf4]);
    cpu.set_a20(true);
    cpu.write_u8(0xffff, 0x10, 0x47);
    advance(&mut cpu, 1);
    assert!(!cpu.can_accept_maskable_interrupt()); // STI shadow.

    let snapshot = cpu.save_snapshot().unwrap();
    let mut restored = Cpu::new();
    restored.restore_snapshot(&snapshot).unwrap();
    assert_eq!(restored.save_registers(), cpu.save_registers());
    assert_eq!(restored.read_u8(0xffff, 0x10), 0x47);
    assert!(!restored.can_accept_maskable_interrupt());
    advance(&mut restored, 1);
    assert!(restored.can_accept_maskable_interrupt());
    assert_eq!(restored.get_reg(Register::EAX), Some(0x12345678));
    assert_eq!(restored.step().unwrap(), Step::Halt);

    let halted = restored.save_snapshot().unwrap();
    cpu.restore_snapshot(&halted).unwrap();
    assert!(cpu.is_halted());
    assert_eq!(cpu.instruction_pointer(), restored.instruction_pointer());
    assert_eq!(cpu.step().unwrap(), Step::Halt);
}

#[test]
fn rejected_cpu_snapshots_leave_registers_and_memory_intact() {
    let mut cpu = program_for(CpuProfile::I8086, &[0x90, 0xf4]);
    let snapshot = cpu.save_snapshot().unwrap();
    advance(&mut cpu, 1);
    cpu.memory[0x12345] = 0x6b;
    let current = cpu.save_snapshot().unwrap();
    let mut corrupt = snapshot.clone();
    corrupt[8..10].copy_from_slice(&999_u16.to_le_bytes());
    assert_eq!(
        cpu.restore_snapshot(&corrupt),
        Err(SnapshotError::Version(999))
    );
    corrupt[8..10].copy_from_slice(&1_u16.to_le_bytes());
    corrupt[10..12].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(cpu.restore_snapshot(&corrupt), Err(SnapshotError::Metadata));
    assert_eq!(
        cpu.restore_snapshot(&snapshot[..3]),
        Err(SnapshotError::Header)
    );
    assert_eq!(
        cpu.restore_snapshot(&snapshot[..snapshot.len() - 1]),
        Err(SnapshotError::MemorySize)
    );
    assert_eq!(cpu.save_snapshot().unwrap(), current);

    let mut incompatible = Cpu::new();
    assert_eq!(
        incompatible.restore_snapshot(&snapshot),
        Err(SnapshotError::Profile)
    );
}

#[test]
fn restoring_cpu_state_requires_the_saved_device_aperture() {
    let mut cpu = Cpu::new();
    let device = TestDevice(Default::default());
    cpu.memory[0xa0000] = 0xa1;
    assert!(cpu.map_memory(0xa0000, 4, Box::new(device.clone())));
    cpu.write_u8(0xa000, 0, 0x35);
    let snapshot = cpu.save_snapshot().unwrap();
    let mut restored = Cpu::new();
    assert_eq!(
        restored.restore_snapshot(&snapshot),
        Err(SnapshotError::Mapping)
    );
    let rebound = TestDevice(Default::default());
    assert!(restored.map_memory(0xa0000, 4, Box::new(rebound.clone())));
    restored.write_u8(0xa000, 0, 0x76);
    restored.restore_snapshot(&snapshot).unwrap();
    assert_eq!(restored.memory[0xa0000], 0xa1);
    assert_eq!(restored.read_u8(0xa000, 0), 0x76);
    assert_eq!(cpu.read_u8(0xa000, 0), 0x35);
}
