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
    cpu.enter_interrupt(0x60);
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
    let mut cpu = program_for(CpuProfile::I286, &[0x0f, 0x01, 0xe0]); // SMSW AX
    let error = cpu.step().unwrap_err();
    assert_eq!(error.mnemonic, Mnemonic::Smsw);
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

    cpu.enter_interrupt(8);
    assert!(!cpu.is_halted());
    assert!(!cpu.can_accept_maskable_interrupt());
    assert_eq!(cpu.read_u16(cpu.regs.ss, cpu.regs.sp), 0x0102);
    advance(&mut cpu, 2);
    assert_eq!(cpu.regs.ax, 0x1234);
    assert_eq!(cpu.regs.sp, 0xfffe);
    assert!(cpu.can_accept_maskable_interrupt());
}
