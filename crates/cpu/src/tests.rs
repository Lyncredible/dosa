use super::*;

fn program(code: &[u8]) -> Cpu {
    let mut cpu = Cpu::new();
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
