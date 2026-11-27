use crate::{Machine, RunState, Video};
use dosa_mount::Mount;
use std::{fs, path::Path};

pub(super) struct Directory(tempfile::TempDir);

impl Directory {
    pub(super) fn new(name: &str) -> Self {
        Self(
            tempfile::Builder::new()
                .prefix(&format!("dosa-dos-{name}-"))
                .tempdir()
                .unwrap(),
        )
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    pub(super) fn write(&self, name: &str, bytes: &[u8]) {
        fs::write(self.path().join(name), bytes).unwrap();
    }

    pub(super) fn machine(&self) -> Machine {
        let mut machine = Machine::new();
        machine
            .mount('C', Mount::open(self.path()).unwrap())
            .unwrap();
        machine
    }
}

#[test]
fn guest_joystick_reads_port_and_bios_with_one_or_no_controllers() {
    let directory = Directory::new("joystick");
    directory.write(
        "JOYCHECK.COM",
        &[
            0xba, 1, 2, 0xec, 0xa2, 0, 2, // IN AL,DX; store settled port
            0xb8, 0, 0x84, 0x31, 0xd2, 0xcd, 0x15, // BIOS buttons
            0xa3, 2, 2, 0x9c, 0x8f, 6, 0x0c, 2, // Save AX and flags
            0xb8, 0, 0x84, 0xba, 1, 0, 0xcd, 0x15, // BIOS axes
            0xa3, 4, 2, 0x89, 0x1e, 6, 2, 0x89, 0x0e, 8, 2, 0x89, 0x16, 0x0a, 2, 0x9c, 0x8f, 6,
            0x0e, 2, // Save flags
            0xb8, 0, 0x4c, 0xcd, 0x21,
        ],
    );
    let mut machine = directory.machine();
    let read = |machine: &mut Machine| {
        machine.launch('C', "JOYCHECK.COM", "").unwrap();
        assert_eq!(machine.run(100).unwrap(), RunState::Exited(0));
        std::array::from_fn::<_, 8, _>(|offset| {
            machine
                .cpu
                .read_u16(machine.cpu.regs.ds, 0x200 + 2 * offset as u16)
        })
    };
    let disconnected = read(&mut machine);
    assert_eq!(disconnected[0] as u8, 0xff);
    assert_eq!(disconnected[1], 0xf0);
    assert_eq!(&disconnected[2..6], &[0, 0, 0, 0]);
    assert_eq!(disconnected[6] & crate::CARRY, 1);
    assert_eq!(disconnected[7] & crate::CARRY, 1);

    machine.set_joystick(
        1,
        Some(crate::JoystickState {
            x: 41,
            y: 203,
            buttons: [true, false],
        }),
    );
    let connected = read(&mut machine);
    assert_eq!(connected[0] as u8, 0xb3);
    assert_eq!(connected[1] as u8, 0xb0);
    assert_eq!(&connected[2..6], &[0, 0, 41, 203]);
    assert_eq!(connected[6] & crate::CARRY, 0);
    assert_eq!(connected[7] & crate::CARRY, 0);
}

#[test]
fn guest_display_detection_agrees_with_bios_equipment_word() {
    let directory = Directory::new("equipment-word");
    directory.write(
        "DISPLAY.COM",
        &[
            0x31, 0xc0, 0x8e, 0xc0, // ES = 0
            0x26, 0x8b, 0x1e, 0x10, 0x04, // MOV BX, ES:[0410h]
            0xcd, 0x11, // Read the BIOS equipment word
            0x39, 0xd8, 0x75, 0x0b, // Fail if the two readings disagree
            0x24, 0x30, 0x3c, 0x20, 0x75, 0x05, // Require 80-column color
            0xb8, 0x00, 0x4c, 0xcd, 0x21, // Exit successfully
            0xb8, 0x01, 0x4c, 0xcd, 0x21, // Exit with an error
        ],
    );
    let mut machine = directory.machine();
    machine.launch('C', "DISPLAY.COM", "").unwrap();
    assert_eq!(machine.run(100).unwrap(), RunState::Exited(0));
}

#[test]
fn redirected_break_and_null_input_do_not_wait_for_the_keyboard() {
    let directory = Directory::new("redirect-input");
    directory.write("INPUT.COM", &[0xeb, 0xfe]);
    directory.write("CTRL.DAT", &[3]);
    let mut machine = directory.machine();
    machine.launch('C', "INPUT.COM", "").unwrap();

    let null = machine.files.open(&machine.mounts, "NUL", 0).unwrap();
    machine.files.duplicate_to(null, 0).unwrap();
    machine.key(b'Q', 0x10);
    machine.cpu.regs.ax = 0x0800;
    machine.interrupt(0x21).unwrap();
    assert_eq!(machine.cpu.regs.ax as u8, 0);
    assert_eq!(machine.state, RunState::Running);
    machine.cpu.regs.ax = 0x0c07;
    machine.interrupt(0x21).unwrap();
    assert_eq!(machine.keys.front().copied(), Some((b'Q', 0x10)));

    let input = machine.files.open(&machine.mounts, "CTRL.DAT", 0).unwrap();
    machine.files.duplicate_to(input, 0).unwrap();
    machine.set_extended_break(true);
    machine.cpu.regs.ax = 0x0800;
    machine.interrupt(0x21).unwrap();
    assert_eq!(machine.state, RunState::Exited(3));
}

#[test]
fn dos_keyboard_echo_obeys_redirected_stdout() {
    let directory = Directory::new("redirect-echo");
    directory.write("ECHO.COM", &[0xeb, 0xfe]);
    let mut machine = Machine::new();
    machine
        .mount(
            'C',
            Mount::open_writable_directory(directory.path()).unwrap(),
        )
        .unwrap();
    machine.launch('C', "ECHO.COM", "").unwrap();
    let handle = machine
        .files
        .create(&machine.mounts, "ECHO.DAT", None)
        .unwrap();
    machine.files.duplicate_to(handle, 1).unwrap();
    machine.key(b'A', 0);
    machine.cpu.regs.ax = 0x0100;
    machine.interrupt(0x21).unwrap();
    assert_eq!(machine.cpu.regs.ax as u8, b'A');

    let segment = machine.cpu.regs.ds;
    machine.cpu.write_u8(segment, 0x200, 8);
    for key in [b'B', b'C', 8, b'D', b'\r'] {
        machine.key(key, 0);
    }
    machine.cpu.regs.ax = 0x0a00;
    machine.cpu.regs.dx = 0x200;
    machine.interrupt(0x21).unwrap();
    assert_eq!(machine.cpu.read_u8(segment, 0x201), 2);
    assert_eq!(
        machine.cpu.read_u16(segment, 0x202),
        u16::from_le_bytes(*b"BD")
    );
    assert!(machine.take_output().is_empty());
    assert_eq!(
        fs::read(directory.path().join("ECHO.DAT")).unwrap(),
        b"ABC\x08 \x08D\r\n"
    );
}

#[test]
fn dos_handle_table_closes_and_reuses_numbers_without_closing_aliases() {
    let directory = Directory::new("handle-table");
    directory.write("PROBE.COM", &[0xeb, 0xfe]);
    let mut machine = directory.machine();
    machine.launch('C', "PROBE.COM", "").unwrap();
    let mut int21 = |ax, bx, cx| {
        machine.cpu.regs.ax = ax;
        machine.cpu.regs.bx = bx;
        machine.cpu.regs.cx = cx;
        machine.interrupt(0x21).unwrap();
        machine.cpu.regs
    };
    let state = int21(0x4500, 0, 0);
    assert_eq!((state.flags & 1, state.ax), (0, 5));
    assert_eq!(int21(0x3e00, 0, 0).flags & 1, 0);
    let unknown = int21(0x4600, 0, 5);
    assert_eq!((unknown.flags & 1, unknown.ax), (1, 6));
    let restored = int21(0x4500, 5, 0);
    assert_eq!((restored.flags & 1, restored.ax), (0, 0));
    assert_eq!(int21(0x4600, 0, 5).flags & 1, 0);
    let invalid = int21(0x4600, 0, u16::MAX);
    assert_eq!((invalid.flags & 1, invalid.ax), (1, 6));
    assert_eq!(int21(0x3e00, 5, 0).flags & 1, 0);
    assert_eq!(int21(0x4400, 0, 0).dx, 0x81);
    assert_eq!(machine.cpu.read_u8(crate::PSP, 0x18), 0);
    assert_eq!(machine.cpu.read_u8(crate::PSP, 0x1d), 0xff);
}

#[test]
fn named_null_and_console_handles_support_file_services() {
    let directory = Directory::new("devices");
    directory.write("PROBE.COM", &[0xeb, 0xfe]); // wait in a guest loop
    let mut machine = directory.machine();
    machine.launch('C', "PROBE.COM", "").unwrap();
    let ds = machine.cpu.regs.ds;
    machine.cpu.load(ds, 0x200, b"nUl.TxT\0CoN\0Q");
    let call = |machine: &mut Machine, ax, bx, cx, dx| {
        machine.cpu.regs.ax = ax;
        machine.cpu.regs.bx = bx;
        machine.cpu.regs.cx = cx;
        machine.cpu.regs.dx = dx;
        machine.interrupt(0x21).unwrap();
        machine.cpu.regs
    };

    let null = call(&mut machine, 0x3c00, 0, 0, 0x200);
    assert_eq!(null.flags & 1, 0);
    assert_eq!(null.ax, 5);
    let handle = null.ax;
    assert_eq!(call(&mut machine, 0x4400, handle, 0, 0).dx, 0x84);
    assert_eq!(call(&mut machine, 0x3f00, handle, 1, 0x20c).ax, 0);
    assert_eq!(machine.cpu.read_u8(ds, 0x20c), b'Q');
    assert_eq!(call(&mut machine, 0x4000, handle, 1, 0x20c).ax, 1);
    assert_eq!(call(&mut machine, 0x4200, handle, 0, 10).ax, 0);
    assert!(machine.take_output().is_empty());
    assert_eq!(call(&mut machine, 0x3e00, handle, 0, 0).flags & 1, 0);
    assert!(!directory.path().join("nUl.TxT").exists());

    let console = call(&mut machine, 0x3d02, 0, 0, 0x208);
    assert_eq!(console.flags & 1, 0);
    assert_eq!(console.ax, handle);
    assert_eq!(call(&mut machine, 0x4400, handle, 0, 0).dx & 0x83, 0x83);
    assert_eq!(call(&mut machine, 0x4406, handle, 0, 0).ax as u8, 0);
    call(&mut machine, 0x3f00, handle, 1, 0x20d);
    assert_eq!(machine.run(20).unwrap(), RunState::WaitingForKey);
    machine.key(b'Y', 0x15);
    assert_eq!(machine.run(20).unwrap(), RunState::Running);
    assert_eq!(machine.cpu.read_u8(ds, 0x20d), b'Y');
    assert_eq!(call(&mut machine, 0x4000, handle, 2, 0x20c).ax, 2);
    assert_eq!(machine.take_output(), b"QY");
    assert_eq!(call(&mut machine, 0x3e00, handle, 0, 0).flags & 1, 0);
    let closed = call(&mut machine, 0x4400, handle, 0, 0);
    assert_eq!((closed.flags & 1, closed.ax), (1, 6));
}

#[test]
fn named_device_paths_respect_directories_and_access_modes() {
    let directory = Directory::new("device-names");
    fs::create_dir(directory.path().join("SUB")).unwrap();
    let mut machine = directory.machine();
    assert!(machine.files.open(&machine.mounts, "NUL", 0).is_ok());
    assert_eq!(
        machine.files.open(&machine.mounts, "C:\\MISSING\\NUL", 0),
        Err(3)
    );
    assert_eq!(machine.files.open(&machine.mounts, "Z:\\NUL", 0), Err(15));
    let handle = machine
        .files
        .open(&machine.mounts, "C:\\SUB\\NUL", 0)
        .unwrap();
    assert!(machine.files.device(handle).unwrap().readable);
    assert!(!machine.files.device(handle).unwrap().writable);
}

#[test]
fn dos_break_checks_disk_calls_only_when_enabled() {
    let directory = Directory::new("disk-break");
    directory.write(
        "OPEN.COM",
        &[
            0xba, 0x14, 1, 0xb8, 0, 0x3d, 0xcd, 0x21, // open DATA.TXT
            0x73, 5, 0xb8, 1, 0x4c, 0xcd, 0x21, // fail if open failed
            0xb8, 0, 0x4c, 0xcd, 0x21, // exit 0
            b'D', b'A', b'T', b'A', b'.', b'T', b'X', b'T', 0,
        ],
    );
    directory.write("DATA.TXT", b"x");
    for (enabled, status) in [(false, 0), (true, 3)] {
        let mut machine = directory.machine();
        machine.set_extended_break(enabled);
        machine.launch('C', "OPEN.COM", "").unwrap();
        machine.key(3, 0x2e);
        assert_eq!(machine.run(100).unwrap(), RunState::Exited(status));
        assert_eq!(machine.take_output() == b"^C\r\n", enabled);
    }
}

#[test]
fn dos_break_leaves_raw_input_and_resumes_cooked_input_after_int23() {
    let directory = Directory::new("console-break");
    directory.write(
        "INPUT.COM",
        &[
            0xba, 0x20, 1, 0xb8, 0x23, 0x25, 0xcd, 0x21, // install INT 23h
            0xb4, 7, 0xcd, 0x21, 0x3c, 3, 0x74, 5, // raw read must return Ctrl+C
            0xb8, 1, 0x4c, 0xcd, 0x21, 0xb4, 8, 0xcd, 0x21, // cooked read checks Ctrl+C
            0xb4, 0x4c, 0xcd, 0x21, // exit with the input byte
            0, 0, 0, 0x2e, 0xfe, 0x06, 0x26, 0x01, 0xcf, 0, // INC CS:[0126h], IRET, counter
        ],
    );
    let mut machine = directory.machine();
    machine.launch('C', "INPUT.COM", "").unwrap();
    machine.key(3, 0x2e);
    assert_eq!(machine.run(100).unwrap(), RunState::WaitingForKey);
    let segment = machine.cpu.regs.cs;
    assert_eq!(machine.cpu.read_u8(segment, 0x126), 0);

    machine.key(3, 0x2e);
    assert_eq!(machine.run(100).unwrap(), RunState::WaitingForKey);
    assert_eq!(machine.cpu.read_u8(segment, 0x126), 1);
    assert_eq!(machine.take_output(), b"^C\r\n");

    machine.key(b'Q', 0x10);
    assert_eq!(machine.run(100).unwrap(), RunState::Exited(b'Q'));
}

#[test]
fn paused_debugger_steps_through_wait_and_consumes_no_wall_clock_time() {
    let directory = Directory::new("debugger");
    directory.write(
        "STEP.COM",
        &[
            0xb8, 0x34, 0x12, // MOV AX,1234h
            0x40, // INC AX
            0x31, 0xc0, // XOR AX,AX
            0xcd, 0x16, // Wait for a key
            0xb4, 0x4c, 0xcd, 0x21, // Exit with the key as status
        ],
    );
    let mut machine = directory.machine();
    machine.launch('C', "step.com", "").unwrap();
    assert!(matches!(
        machine.step_paused(),
        Err(crate::DosError::GuestNotPaused)
    ));
    machine.set_paused(true);
    let frozen_time = machine.elapsed();
    let guest = machine.paused_guest().unwrap();
    let regs = guest.registers();
    assert_eq!(guest.peek(regs.cs, regs.ip), Some(0xb8));
    assert_eq!(machine.run(100).unwrap(), RunState::Running);
    assert_eq!(machine.paused_guest().unwrap().registers(), regs);
    assert_eq!(machine.step_paused().unwrap(), RunState::Running);
    let after = machine.paused_guest().unwrap().registers();
    assert_eq!(after.ax, 0x1234);
    assert_eq!(after.ip, regs.ip + 3);
    machine.step_paused().unwrap();
    assert_eq!(machine.paused_guest().unwrap().registers().ax, 0x1235);
    machine.step_paused().unwrap();
    assert_eq!(machine.step_paused().unwrap(), RunState::WaitingForKey);
    let waiting = machine.paused_guest().unwrap().registers();
    assert_eq!(machine.step_paused().unwrap(), RunState::WaitingForKey);
    assert_eq!(machine.paused_guest().unwrap().registers(), waiting);
    machine.paste_key(b'Q', 0);
    assert_eq!(machine.step_paused().unwrap(), RunState::Running);
    assert_eq!(
        machine.paused_guest().unwrap().registers().ip,
        waiting.ip + 2
    );
    assert_eq!(machine.elapsed(), frozen_time);
    assert_eq!(machine.step_paused().unwrap(), RunState::Exited(b'Q'));
    assert!(machine.paused_guest().is_none());
}

#[test]
fn com_reads_key_then_prints_and_exits() {
    let directory = Directory::new("keyboard");
    fs::write(
        directory.path().join("KEY.COM"),
        [
            0xb4, 0x00, 0xcd, 0x16, // BIOS read
            0x88, 0xc2, 0xb4, 0x02, 0xcd, 0x21, // echo AL using DOS output
            0xb8, 0x07, 0x4c, 0xcd, 0x21, // exit 7
        ],
    )
    .unwrap();
    let mut machine = directory.machine();
    machine.write_console(b"> ");
    machine.launch('c', "key.com", "").unwrap();
    assert_eq!(machine.run(100).unwrap(), RunState::WaitingForKey);
    machine.key(0, 0x2a);
    assert_eq!(machine.run(100).unwrap(), RunState::WaitingForKey);
    assert_eq!(machine.cpu.read_u8(0x40, 0x17), 2);
    machine.key_up(0x2a);
    assert_eq!(machine.cpu.read_u8(0x40, 0x17), 0);
    machine.key(b'Y', 0x15);
    assert_eq!(machine.run(100).unwrap(), RunState::Exited(7));
    assert_eq!(machine.take_output(), b"Y");
    let Video::Text(frame) = machine.video() else {
        panic!("expected text video")
    };
    assert_eq!(&frame.cells[..6], &[b'>', 7, b' ', 7, b'Y', 7]);
}

#[test]
fn guest_keyboard_irq_waits_for_if_and_pic_and_delivers_make_and_break() {
    let directory = Directory::new("keyboard-irq");
    let mut program = vec![
        0xba, 0x40, 0x01, // mov dx, 0140h
        0xb8, 0x09, 0x25, 0xcd, 0x21, // DOS set INT 09h vector
        0xfa, // cli, host queues input here
        0xb0, 0x02, 0xe6, 0x21, // mask PIC IRQ1
        0xfb, // sti
        0x31, 0xc0, 0xe6, 0x21, // unmask PIC
        0xeb, 0xfe, // wait on IRQ1
    ];
    program.resize(0x40, 0x90);
    program.extend_from_slice(&[
        0xfb, // sti, must not recursively receive the current IRQ
        0x50, 0x53, // push ax, push bx
        0xb0, 0xad, 0xe6, 0x64, // disable keyboard interface while reading
        0x31, 0xdb, 0x8a, 0x1e, 0x80, 0x01, // bx = processed count
        0xe4, 0x60, 0x88, 0x87, 0x81, 0x01, // store scancode at [bx+0181h]
        0xfe, 0x06, 0x80, 0x01, // increment processed count
        0xb0, 0xae, 0xe6, 0x64, // enable keyboard interface
        0xb0, 0x20, 0xe6, 0x20, // non-specific EOI
        0x5b, 0x58, 0xcf, // pop bx, pop ax, iret
    ]);
    fs::write(directory.path().join("IRQ.COM"), program).unwrap();
    let mut machine = directory.machine();
    machine.launch('C', "IRQ.COM", "").unwrap();
    machine.run(4).unwrap();
    let segment = machine.cpu.regs.cs;
    let initial_stack = machine.cpu.regs.sp;
    machine.key(b'A', 0x1e);
    machine.key_up(0x1e);

    machine.run(5).unwrap(); // executes through unmasking
    assert_eq!(machine.cpu.read_u8(segment, 0x180), 0);
    assert_eq!(machine.cpu.regs.sp, initial_stack);
    assert!(machine.keys.is_empty(), "guest handler owns the scancodes");

    machine.run(3).unwrap(); // handler entry, STI and both pushes
    assert_eq!(machine.cpu.regs.sp, initial_stack - 10);
    machine.run(100).unwrap();
    assert_eq!(machine.cpu.read_u8(segment, 0x180), 2);
    assert_eq!(machine.cpu.read_u8(segment, 0x181), 0x1e);
    assert_eq!(machine.cpu.read_u8(segment, 0x182), 0x9e);
    assert_eq!(machine.cpu.regs.sp, initial_stack);
}

#[test]
fn guest_timer_runs_repeatedly_with_programmed_pit_period() {
    let directory = Directory::new("timer-irq");
    let mut program = vec![
        0xba, 0x40, 0x01, // mov dx, 0140h
        0xb8, 0x08, 0x25, 0xcd, 0x21, // DOS set INT 08h vector
        0xb0, 0x36, 0xe6, 0x43, // PIT channel 0, low/high, mode 3
        0xb0, 0x10, 0xe6, 0x40, // divisor low byte
        0x31, 0xc0, 0xe6, 0x40, // divisor high byte
        0xeb, 0xfe, // wait for timer
    ];
    program.resize(0x40, 0x90);
    program.extend_from_slice(&[
        0x50, // push ax
        0xfe, 0x06, 0x80, 0x01, // increment timer count
        0xcd, 0x1c, // default BIOS timer callback
        0xb0, 0x20, 0xe6, 0x20, // non-specific EOI
        0x58, 0xcf, // pop ax, iret
    ]);
    fs::write(directory.path().join("TIMER.COM"), program).unwrap();
    let mut machine = directory.machine();
    machine.launch('C', "TIMER.COM", "").unwrap();
    machine.pit = crate::pit::Pit::new(0);
    let segment = machine.cpu.regs.cs;
    let initial_stack = machine.cpu.regs.sp;
    crate::bios::install(&mut machine.cpu, crate::pit::BIOS_DIVISOR * 3);
    assert_eq!(machine.cpu.read_u16(0x40, 0x6c), 3);
    for _ in 0..9 {
        machine.step().unwrap();
    }

    machine.pit.advance(15);
    machine.step().unwrap();
    assert_eq!(machine.cpu.read_u8(segment, 0x180), 0);

    machine.pit.advance(17);
    machine.step().unwrap();
    machine.pit.advance(33);
    for _ in 0..20 {
        machine.step().unwrap();
    }
    assert_eq!(machine.cpu.read_u8(segment, 0x180), 2);
    assert_eq!(machine.cpu.regs.sp, initial_stack);
    // The guest calls INT 1Ch itself but never chains BIOS IRQ0.
    assert_eq!(machine.cpu.read_u16(0x40, 0x6c), 3);
}

#[test]
fn mz_relocates_and_displays_vga_memory() {
    let directory = Directory::new("mz");
    let program = [
        0xb8, 0x13, 0x00, 0xcd, 0x10, // select mode 13h
        0xb8, 0x00, 0x00, 0x8e, 0xd8, // set DS from relocation
        0xbe, 0x1a, 0x00, 0x8a, 0x04, // read color through DS
        0xba, 0x00, 0xa0, 0x8e, 0xc2, // ES = video memory
        0x26, 0xa2, 0x00, 0x00, // store AL at ES:0
        0xeb, 0xfe, // stay in graphics mode
        42,
    ];
    let mut mz = vec![0_u8; 32];
    mz[0..2].copy_from_slice(b"MZ");
    for (offset, value) in [
        (2, 32 + program.len()),
        (4, 1),
        (6, 1),
        (8, 2),
        (0x10, 0x100),
        (0x14, 0x100),
        (0x16, 0xfff0),
        (0x18, 0x1c),
        (0x1c, 6),
    ] {
        mz[offset..offset + 2].copy_from_slice(&(value as u16).to_le_bytes());
    }
    mz.extend(program);
    fs::write(directory.path().join("VGA.EXE"), mz).unwrap();
    let mut machine = directory.machine();
    machine.launch('C', "VGA.EXE", "").unwrap();
    assert_eq!(machine.run(30).unwrap(), RunState::Running);
    let Video::Graphics {
        width,
        height,
        indices,
        ..
    } = machine.video()
    else {
        panic!("expected VGA mode")
    };
    assert_eq!((width, height, indices.len()), (320, 200, 64_000));
    assert_eq!(indices[0], 42);
    machine.stop();
    assert_eq!(machine.run(1).unwrap(), RunState::Exited(3));
    assert!(matches!(machine.video(), Video::Text(_)));
}

#[test]
fn guest_opens_file_relative_to_nested_executable() {
    let directory = Directory::new("files");
    let nested = directory.path().join("PROGRAMS");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("VALUE.TXT"), b"abc").unwrap();
    let mut program = vec![
        0xb8, 0x00, 0x3d, 0xba, 0x28, 0x01, 0xcd, 0x21, // open VALUE.TXT
        0x89, 0xc3, 0xb4, 0x3f, 0xb9, 0x03, 0x00, 0xba, 0x40, 0x01, 0xcd, 0x21, 0x89, 0xc1, 0xb4,
        0x40, 0xbb, 0x01, 0x00, 0xcd, 0x21, // write bytes to stdout
        0xb8, 0x00, 0x4c, 0xcd, 0x21,
    ];
    program.resize(0x28, 0);
    program.extend_from_slice(b"VALUE.TXT\0");
    fs::write(nested.join("READ.COM"), program).unwrap();
    let mut machine = directory.machine();
    machine.launch('C', "programs/read.com", "").unwrap();
    assert_eq!(machine.run(100).unwrap(), RunState::Exited(0));
    assert_eq!(machine.take_output(), b"abc");
}

#[test]
fn cga_graphics_reads_both_interleaved_scanline_banks() {
    let directory = Directory::new("cga");
    fs::write(
        directory.path().join("CGA.COM"),
        [
            0xb8, 0x06, 0x00, 0xcd, 0x10, // set CGA mode 6
            0xba, 0x00, 0xb8, 0x8e, 0xc2, // ES = video memory
            0xb0, 0x80, 0x26, 0xa2, 0x00, 0x00, // first even pixel
            0xb0, 0x01, 0x26, 0xa2, 0x00, 0x20, // eighth odd pixel
            0xeb, 0xfe, // stay in graphics mode
        ],
    )
    .unwrap();
    let mut machine = directory.machine();
    machine.write_console(b">");
    machine.launch('C', "CGA.COM", "").unwrap();
    assert_eq!(machine.run(30).unwrap(), RunState::Running);
    let Video::Graphics {
        width,
        height,
        indices,
        palette,
    } = machine.video()
    else {
        panic!("expected CGA graphics");
    };
    assert_eq!((width, height), (640, 200));
    assert_eq!(&indices[..8], &[15, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&indices[640..648], &[0, 0, 0, 0, 0, 0, 0, 15]);
    assert_eq!(palette[15], [255; 3]);
    machine.stop();
    let Video::Text(frame) = machine.video() else {
        panic!("expected restored console")
    };
    assert_eq!(&frame.cells[..2], &[b'>', 7]);
}

#[test]
fn chained_default_timer_updates_bda_calls_int1c_and_acknowledges_irq() {
    let directory = Directory::new("chained-timer");
    fs::write(directory.path().join("TIMER.COM"), [0xeb, 0xfe]).unwrap();
    let mut machine = directory.machine();
    machine.launch('C', "timer.com", "").unwrap();
    let segment = machine.cpu.regs.cs;

    // Obtain the callable default handler via DOS and chain from a guest ISR.
    machine.cpu.regs.ax = 0x3508;
    machine.interrupt(0x21).unwrap();
    let old = (machine.cpu.regs.es, machine.cpu.regs.bx);
    assert_ne!(old, (0, 0));
    machine.cpu.write_u16(segment, 0x180, old.1);
    machine.cpu.write_u16(segment, 0x182, old.0);
    machine.cpu.load(
        segment,
        0x120,
        &[0x9c, 0xff, 0x1e, 0x80, 0x01, 0xcf], // PUSHF / CALL FAR [0180h] / IRET
    );
    machine.cpu.write_u16(0, 0x08 * 4, 0x120);
    machine.cpu.write_u16(0, 0x08 * 4 + 2, segment);
    machine.cpu.load(
        segment,
        0x140,
        &[0x2e, 0xfe, 0x06, 0x60, 0x01, 0xcf], // INC BYTE PTR CS:[0160h] / IRET
    );
    machine.cpu.write_u16(0, 0x1c * 4, 0x140);
    machine.cpu.write_u16(0, 0x1c * 4 + 2, segment);
    machine.cpu.write_u16(0x40, 0x6c, 0x00af);
    machine.cpu.write_u16(0x40, 0x6e, 0x0018);

    let before = machine.cpu.regs;
    let divisor = crate::pit::BIOS_DIVISOR;
    machine.pit = crate::pit::Pit::new(0);
    machine.pit.advance(divisor);
    for _ in 0..16 {
        machine.step().unwrap();
    }
    assert_eq!(machine.cpu.regs, before);
    assert_eq!(machine.cpu.read_u8(segment, 0x160), 1);
    assert_eq!(machine.cpu.read_u16(0x40, 0x6c), 0);
    assert_eq!(machine.cpu.read_u16(0x40, 0x6e), 0);
    assert_eq!(machine.cpu.read_u8(0x40, 0x70), 1);
    machine.pic.port_write(0x20, 0x0b);
    assert_eq!(machine.pic.port_read(0x20, 0), Some(0));

    machine.cpu.regs.ax = 0;
    machine.interrupt(0x1a).unwrap();
    assert_eq!(machine.cpu.regs.ax, 1);
    assert_eq!(machine.cpu.regs.cx, 0);
    assert_eq!(machine.cpu.regs.dx, 0);
    assert_eq!(machine.cpu.read_u8(0x40, 0x70), 0);
    machine.pit.advance(divisor * 2);
    for _ in 0..16 {
        machine.step().unwrap();
    }
    assert_eq!(machine.cpu.read_u8(segment, 0x160), 2);
    assert_eq!(machine.cpu.read_u16(0x40, 0x6c), 1);
}

#[test]
fn chained_default_keyboard_handler_completes_irq_and_delivers_a_key_once() {
    let directory = Directory::new("chained-keyboard");
    fs::write(directory.path().join("KEYBOARD.COM"), [0xeb, 0xfe]).unwrap();
    let mut machine = directory.machine();
    machine.launch('C', "keyboard.com", "").unwrap();
    let segment = machine.cpu.regs.cs;

    machine.cpu.regs.ax = 0x3509;
    machine.interrupt(0x21).unwrap();
    let old = (machine.cpu.regs.es, machine.cpu.regs.bx);
    assert_ne!(old, (0, 0));
    machine.cpu.write_u16(segment, 0x180, old.1);
    machine.cpu.write_u16(segment, 0x182, old.0);
    machine.cpu.load(
        segment,
        0x120,
        &[0x9c, 0xff, 0x1e, 0x80, 0x01, 0xcf], // PUSHF / CALL FAR [0180h] / IRET
    );
    machine.cpu.write_u16(0, 0x09 * 4, 0x120);
    machine.cpu.write_u16(0, 0x09 * 4 + 2, segment);

    let before = machine.cpu.regs;
    machine.key(b'K', 0x25);
    machine.key_up(0x25);
    assert!(machine.keys.is_empty());
    for _ in 0..20 {
        machine.step().unwrap();
    }
    assert_eq!(machine.cpu.regs, before);
    assert_eq!(machine.keys.len(), 1);
    machine.pic.port_write(0x20, 0x0b);
    assert_eq!(machine.pic.port_read(0x20, 0), Some(0));
    assert_eq!(machine.keyboard.port_read(0x64), Some(0));

    machine.cpu.regs.ax = 0;
    machine.interrupt(0x16).unwrap();
    assert_eq!(machine.cpu.regs.ax, 0x254b);
    assert!(machine.keys.is_empty());
}

#[test]
fn halted_guest_stays_active_and_resumes_only_after_an_enabled_irq() {
    let directory = Directory::new("halt");
    fs::write(
        directory.path().join("HALT.COM"),
        [0xfa, 0xf4, 0xb8, 7, 0x4c, 0xcd, 0x21], // CLI / HLT / MOV AX,4C07h / INT 21h
    )
    .unwrap();
    let mut machine = directory.machine();
    machine.launch('C', "halt.com", "").unwrap();
    assert_eq!(machine.run(256).unwrap(), RunState::WaitingForInterrupt);
    let halted_ip = machine.cpu.regs.ip;
    machine.key(b'A', 0x1e);
    assert_eq!(machine.run(0).unwrap(), RunState::WaitingForInterrupt);
    assert_eq!(machine.run(256).unwrap(), RunState::WaitingForInterrupt);
    assert_eq!(machine.cpu.regs.ip, halted_ip);
    assert!(matches!(
        machine.launch('C', "halt.com", ""),
        Err(crate::DosError::Busy)
    ));

    // Externally enabling IF lets the already pending keyboard IRQ wake HLT.
    machine.cpu.regs.flags |= 0x200;
    assert_eq!(machine.run(256).unwrap(), RunState::Exited(7));
}

#[test]
fn blocking_int16_wait_runs_a_chained_keyboard_irq_before_resuming() {
    let directory = Directory::new("blocked-int16");
    fs::write(
        directory.path().join("WAIT.COM"),
        [0x31, 0xc0, 0xcd, 0x16, 0xb4, 0x4c, 0xcd, 0x21],
    )
    .unwrap();
    let mut machine = directory.machine();
    machine.launch('C', "wait.com", "").unwrap();
    let segment = machine.cpu.regs.cs;
    let old = (
        machine.cpu.read_u16(0, 0x09 * 4),
        machine.cpu.read_u16(0, 0x09 * 4 + 2),
    );
    machine.cpu.write_u16(segment, 0x180, old.0);
    machine.cpu.write_u16(segment, 0x182, old.1);
    machine.cpu.load(
        segment,
        0x120,
        &[0x9c, 0xff, 0x1e, 0x80, 0x01, 0xcf], // PUSHF / CALL FAR [0180h] / IRET
    );
    machine.cpu.write_u16(0, 0x09 * 4, 0x120);
    machine.cpu.write_u16(0, 0x09 * 4 + 2, segment);

    let sp = machine.cpu.regs.sp;
    assert_eq!(machine.run(32).unwrap(), RunState::WaitingForKey);
    assert_eq!(machine.cpu.regs.ip, 0x0104);
    machine.key(b'K', 0x25);
    assert!(
        machine.keys.is_empty(),
        "input must be delivered by the chain"
    );
    assert_eq!(machine.run(1).unwrap(), RunState::Running);
    assert_ne!(machine.cpu.regs.sp, sp, "ISR is still running");
    assert_eq!(machine.run(32).unwrap(), RunState::Exited(b'K'));
    assert_eq!(machine.cpu.regs.sp, sp);
    assert!(machine.pending.is_empty());
}

#[test]
fn masked_irq1_leaves_kbc_output_available_for_guest_port_polling() {
    let directory = Directory::new("masked-kbc");
    fs::write(
        directory.path().join("POLL.COM"),
        [0xe4, 0x60, 0xb4, 0x4c, 0xcd, 0x21], // IN AL,60h / MOV AH,4Ch / INT 21h
    )
    .unwrap();
    let mut machine = directory.machine();
    machine.launch('C', "poll.com", "").unwrap();
    machine.pic.port_write(0x21, 0b10);
    machine.key(b'A', 0x1e);
    assert_eq!(machine.run(32).unwrap(), RunState::Exited(0x1e));
    assert_eq!(machine.keyboard.port_read(0x64), Some(0));
}

#[test]
fn dos_guest_enters_protected_mode_dispatches_an_interrupt_and_returns() {
    let folder = Directory::new("protected-mode");
    let mut com = vec![0; 0x708];
    fn place(com: &mut [u8], offset: usize, bytes: &[u8]) {
        let start = offset - 0x100;
        com[start..start + bytes.len()].copy_from_slice(bytes);
    }
    place(
        &mut com,
        0x100,
        &[
            0xfa, // CLI
            0x0f, 1, 0x16, 0x80, 1, // LGDT [0180h]
            0x0f, 1, 0x1e, 0x86, 1, // LIDT [0186h]
            0x0f, 0x20, 0xc3, // MOV EBX,CR0
            0x66, 0x83, 0xcb, 1, // OR EBX,1
            0x0f, 0x22, 0xc3, // MOV CR0,EBX
            0xea, 0, 0, 8, 0, // JMP 0008:0000
        ],
    );
    place(
        &mut com,
        0x170,
        &[
            0x8c, 0xcb, 0x8e, 0xdb, 0x8e, 0xd3, 0xbc, 0xfe,
            0xff, // Restore real-mode DS/SS/SP.
            0xb4, 0x4c, 0x88, 0xd0, 0xcd, 0x21, // Exit with the protected-mode IRQ result.
        ],
    );
    place(
        &mut com,
        0x180,
        &[
            0x17, 0, 0x90, 1, 1, 0, // GDTR at physical 10190h.
            0x07, 3, 0, 5, 1, 0, // IDTR at physical 10500h, limit through INT 60h.
        ],
    );
    place(
        &mut com,
        0x190,
        &[
            0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0, 0, 4, 1, 0x9a, 0, 0, // Code at DOS offset 0400h.
            0xff, 1, 0, 2, 1, 0x92, 0, 0, // Data and stack at offset 0200h.
        ],
    );
    place(
        &mut com,
        0x400,
        &[
            0xb8, 0x10, 0, 0x8e, 0xd8, 0x8e, 0xd0, 0xbc, 0, 1, // DS=SS=0010h, SP=0100h.
            0xcd, 0x60, 0x0f, 0x20, 0xc3, // MOV EBX,CR0
            0x66, 0x83, 0xe3, 0xfe, // AND EBX,-2
            0x0f, 0x22, 0xc3, // MOV CR0,EBX
            0xea, 0x70, 1, 0, 0x10, // JMP 1000:0170h
        ],
    );
    place(
        &mut com,
        0x440,
        &[
            0xc6, 0x06, 2, 0, 0x6b, // Write through the protected data selector.
            0xb2, 0x2a, 0xcf, // MOV DL,2Ah; IRET
        ],
    );
    place(&mut com, 0x800, &[0x40, 0, 8, 0, 0, 0x86, 0, 0]); // 16-bit interrupt gate.
    folder.write("PROTECT.COM", &com);
    let mut machine = folder.machine();
    machine.launch('C', "PROTECT.COM", "").unwrap();
    assert_eq!(machine.run(100).unwrap(), RunState::Exited(0x2a));
    assert!(!machine.cpu.protected_mode());
    assert_eq!(machine.cpu.read_u8(crate::PSP, 0x202), 0x6b);
}

#[test]
fn dos_guest_uses_32_bit_code_stack_and_interrupt_gate_above_64_kib() {
    let folder = Directory::new("protected-mode-32");
    let mut com = vec![0; 0x808 - 0x100];
    fn place(com: &mut [u8], offset: usize, bytes: &[u8]) {
        let start = offset - 0x100;
        com[start..start + bytes.len()].copy_from_slice(bytes);
    }

    let mut code = vec![
        0x66, 0xb8, 0x10, 0, 0x8e, 0xd8, 0x8e, 0xc0, 0x8e, 0xd0, // DS=ES=SS=0010h.
        0xbc, 0x0c, 0, 2, 0, // ESP=0002000Ch.
        0xc7, 0x05, 0, 0x70, 1, 0, 0x78, 0x56, 0x34, 0x12, // Write DS:00017000h.
        0xbe, 0, 0x70, 1, 0, 0xbf, 0, 0x80, 1, 0, // ESI and EDI reach above 64 KiB.
        0xb9, 4, 0, 0, 0, 0xf3, 0xa4, // REP MOVSB.
    ];
    let call = code.len();
    code.extend([0xe8, 0, 0, 0, 0]); // Near CALL to offset 102A0h.
    code.extend([
        0xcd, 0x60, // 32-bit IDT gate to offset 102C0h.
        0xa3, 4, 0x80, 1, 0, // Store the handler's EAX.
        0x89, 0x25, 8, 0x80, 1, 0, // Store ESP after IRETD.
        0x0f, 0x20, 0xc3, 0x83, 0xe3, 0xfe, 0x0f, 0x22, 0xc3, // Turn off PE.
        0x66, 0xea, 0x70, 1, 0, 0x10, // Real-mode JMP to 1000:0170.
    ]);
    let subroutine = 0xa0;
    let displacement = (subroutine - (call + 5)) as u32;
    code[call + 1..call + 5].copy_from_slice(&displacement.to_le_bytes());
    code.resize(subroutine, 0x90);
    code.extend([
        0xc8, 0x10, 0, 0, // ENTER 16,0 uses the wide stack.
        0xc7, 0x45, 0xf8, 0x04, 0x03, 0x02, 0x01, // Store through SS:[EBP-8].
        0xc9, 0xc3, // LEAVE; RET
    ]);
    code.resize(0xc0, 0x90);
    code.extend([0xb8, 0x2a, 0xb4, 0x37, 0x92, 0xcf]); // MOV EAX,9237B42Ah; IRETD.

    let length = (code.len() as u16).to_le_bytes();
    place(
        &mut com,
        0x100,
        &[
            0xfa, 0xfc, // CLI; CLD
            0xb8, 0, 0x30, 0x8e, 0xc0, // ES=3000h.
            0xbe, 0, 5, 0xbf, 0, 2, 0xb9, length[0], length[1], 0xf3, 0xa4, // Copy payload.
            0x0f, 1, 0x16, 0x80, 1, 0x0f, 1, 0x1e, 0x86, 1, // LGDT and LIDT.
            0x0f, 0x20, 0xc3, 0x66, 0x83, 0xcb, 1, 0x0f, 0x22, 0xc3, // Turn on PE.
            0x66, 0xea, 0, 2, 1, 0, 8, 0, // JMP 0008:00010200h.
        ],
    );
    place(
        &mut com,
        0x170,
        &[
            0x8c, 0xcb, 0x8e, 0xdb, 0x8e, 0xd3, 0xbc, 0xfe, 0xff, // Restore DOS segments.
            0xb4, 0x4c, 0xcd, 0x21,
        ],
    );
    place(
        &mut com,
        0x180,
        &[
            0x17, 0, 0x90, 1, 1, 0, // GDTR at 10190h.
            0x07, 3, 0, 5, 1, 0, // IDTR at 10500h.
        ],
    );
    place(
        &mut com,
        0x190,
        &[
            0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0, 0, 2, 0x9a, 0x42,
            0, // Wide code at physical 20000h.
            0xff, 0xff, 0, 0, 1, 0x92, 0x42, 0, // Wide stack and data at physical 10000h.
        ],
    );
    place(&mut com, 0x500, &code);
    place(&mut com, 0x800, &[0xc0, 2, 8, 0, 0, 0x8e, 1, 0]);
    folder.write("PM32.COM", &com);

    let mut machine = folder.machine();
    machine.launch('C', "PM32.COM", "").unwrap();
    assert_eq!(machine.run(code.len() + 13).unwrap(), RunState::Running);
    machine.set_paused(true);
    let guest = machine.paused_guest().unwrap();
    assert_eq!(guest.instruction_pointer(), 0x10200);
    assert_eq!(guest.peek_code(0), Some(0x66));
    assert_eq!(guest.peek_code(1), Some(0xb8));
    machine.set_paused(false);
    assert_eq!(machine.run(512).unwrap(), RunState::Exited(0x2a));
    assert!(!machine.cpu.protected_mode());
    assert_eq!(
        &machine.cpu.memory[0x28000..0x2800c],
        &[
            0x78, 0x56, 0x34, 0x12, 0x2a, 0xb4, 0x37, 0x92, 0x0c, 0, 2, 0
        ]
    );
    assert_eq!(&machine.cpu.memory[0x2fffc..0x30000], &[4, 3, 2, 1]);
}

#[test]
fn guest_a20_controls_share_the_same_bus_and_preserve_independent_latches() {
    let folder = Directory::new("a20");
    let code = [
        0xb8, 0xff, 0xff, 0x8e, 0xc0, // ES=FFFFh.
        0x26, 0xc6, 0x06, 0x10, 5, 0x22, // Write 22h to aliased physical 0500h.
        0xb0, 2, 0xe6, 0x92, // Enable fast A20.
        0x26, 0xc6, 0x06, 0x10, 5, 0x33, // Write 33h to 100500h.
        0xb0, 0xd1, 0xe6, 0x64, 0xb0, 3, 0xe6, 0x60, // Enable A20 through the 8042.
        0x30, 0xc0, 0xe6, 0x92, // Disable fast A20 only.
        0xe4, 0x92, 0xa2, 0, 2, // Read back fast A20 latch.
        0x26, 0xa0, 0x10, 5, 0xa2, 1, 2, // Extended RAM remains visible.
        0xb0, 0xd0, 0xe6, 0x64, 0xe4, 0x64, 0xa2, 2, 2, // Controller data ready.
        0xe4, 0x60, 0xa2, 3, 2, // Read back 8042 output port.
        0xb8, 0, 0x24, 0xcd, 0x15, // BIOS disables both A20 paths.
        0x26, 0xa0, 0x10, 5, 0xa2, 4, 2, // Low alias visible again.
        0xb8, 2, 0x24, 0xcd, 0x15, 0xa2, 5, 2, // BIOS returns A20 off.
        0xb8, 1, 0x24, 0xcd, 0x15, // BIOS enables A20.
        0xb8, 2, 0x24, 0xcd, 0x15, 0xa2, 6, 2, // BIOS returns A20 on.
        0x26, 0xa0, 0x10, 5, 0xa2, 7, 2, // High data survives toggles.
        0xb8, 3, 0x24, 0xcd, 0x15, 0x89, 0x1e, 8, 2, // Save supported A20 methods.
        0xb4, 0x88, 0xcd, 0x15, 0xa3, 0x0a, 2, // Query KiB above 1 MiB.
        0xb8, 0, 0x4c, 0xcd, 0x21,
    ];
    folder.write("A20.COM", &code);
    folder.write("IDLE.COM", &[0xeb, 0xfe]);
    for profile in [crate::CpuProfile::I286, crate::CpuProfile::I386] {
        let mut machine = folder.machine();
        machine.set_cpu_profile(profile).unwrap();
        machine.launch('C', "A20.COM", "").unwrap();
        assert_eq!(machine.run(256).unwrap(), RunState::Exited(0));
        assert_eq!(
            &machine.cpu.memory[0x10200..0x1020c],
            &[0, 0x33, 1, 3, 0x22, 0, 1, 0x33, 3, 0, 0, 60]
        );
        assert_eq!(machine.cpu.memory[0x500], 0x22);
        assert_eq!(machine.cpu.memory[0x100500], 0x33);
        machine.launch('C', "IDLE.COM", "").unwrap();
        assert!(!machine.cpu.a20_enabled());
        assert_eq!(machine.cpu.memory[0x100500], 0);
    }
}

#[test]
fn dos_guest_discovers_xms_moves_data_and_releases_the_handle() {
    fn call(code: &mut Vec<u8>, function: u8) {
        code.extend([0xb4, function, 0xff, 0x1e, 0, 4]); // CALL FAR [0400h].
    }

    let folder = Directory::new("xms");
    let mut code = vec![
        0xb8, 0, 0x43, 0xcd, 0x2f, 0xa2, 0x40, 4, // Save installed-state response.
        0xb8, 0x10, 0x43, 0xcd, 0x2f, // Driver pointer in ES:BX.
        0x89, 0x1e, 0, 4, 0x8c, 0x06, 2, 4,
    ];
    call(&mut code, 0);
    code.extend([0xa3, 0x42, 4, 0x89, 0x16, 0x44, 4]); // Version and HMA support.
    call(&mut code, 8);
    code.extend([0xa3, 0x46, 4]); // Largest available block.
    code.extend([0xba, 1, 0]);
    call(&mut code, 9);
    code.extend([
        0xa3, 0x48, 4, 0x89, 0x16, 4, 4, // Record success and save handle.
        0x89, 0x16, 0x2a, 4, 0xbe, 0x20, 4, // Control block with XMS destination.
    ]);
    call(&mut code, 0x0b);
    code.extend([0xa3, 0x4a, 4]);
    call(&mut code, 0x0c);
    code.extend([
        0x89, 0x1e, 0x4c, 4, 0x89, 0x16, 0x4e, 4, // Save locked address.
        0x8b, 0x16, 4, 4, // Restore handle.
    ]);
    call(&mut code, 0x0a);
    code.extend([0x88, 0x1e, 0x50, 4]); // Locked-block error.
    call(&mut code, 0x0d);
    code.extend([0xbb, 2, 0]); // Resize the block to 2 KiB.
    call(&mut code, 0x0f);
    code.extend([0xa3, 0x52, 4]);
    call(&mut code, 0x0e);
    code.extend([
        0x89, 0x1e, 0x54, 4, 0x89, 0x16, 0x56, 4, // Free handles, locks and size.
        0x8b, 0x16, 4, 4, 0x89, 0x16, 0x34, 4, // Control block with XMS source.
        0xbe, 0x30, 4,
    ]);
    call(&mut code, 0x0b);
    code.extend([0xa3, 0x58, 4]);
    code.extend([0xc6, 0x06, 0x30, 4, 3]); // Odd lengths are invalid.
    call(&mut code, 0x0b);
    code.extend([0x88, 0x1e, 0x5a, 4]);
    call(&mut code, 0x0a);
    code.extend([0xa3, 0x5c, 4]);
    call(&mut code, 8);
    code.extend([0xa3, 0x5e, 4, 0xb8, 0, 0x4c, 0xcd, 0x21]);

    code.resize(0x390, 0);
    code[0x320..0x330].copy_from_slice(&[4, 0, 0, 0, 0, 0, 0x80, 4, 0, 0x10, 0, 0, 0, 0, 0, 0]);
    code[0x330..0x340].copy_from_slice(&[4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x88, 4, 0, 0x10]);
    code[0x380..0x384].copy_from_slice(b"XMS!");
    folder.write("XMS.COM", &code);
    let mut machine = folder.machine();
    machine.launch('C', "XMS.COM", "").unwrap();
    assert_eq!(machine.run(512).unwrap(), RunState::Exited(0));
    let result = |offset| machine.cpu.read_u16(crate::PSP, offset);
    assert_eq!(result(0x440) as u8, 0x80);
    assert_eq!(result(0x442), 0x0200);
    assert_eq!(result(0x444), 1);
    assert_eq!(result(0x446), 15 * 1024 - 64);
    assert_eq!(result(0x448), 1);
    assert_eq!(result(0x44a), 1);
    assert_eq!(result(0x44c), 0);
    assert_eq!(result(0x44e), 0x11);
    assert_eq!(result(0x450) as u8, 0xab);
    assert_eq!(result(0x452), 1);
    assert_eq!(result(0x454), 63);
    assert_eq!(result(0x456), 2);
    assert_eq!(result(0x458), 1);
    assert_eq!(result(0x45a) as u8, 0xa7);
    assert_eq!(result(0x45c), 1);
    assert_eq!(result(0x45e), result(0x446));
    assert_eq!(&machine.cpu.memory[0x10488..0x1048c], b"XMS!");
    assert!(!machine.cpu.a20_enabled());

    machine.set_cpu_profile(crate::CpuProfile::I8086).unwrap();
    folder.write(
        "DETECT.COM",
        &[0xb8, 0, 0x43, 0xcd, 0x2f, 0xb4, 0x4c, 0xcd, 0x21],
    );
    machine.launch('C', "DETECT.COM", "").unwrap();
    assert_eq!(machine.run(32).unwrap(), RunState::Exited(0));
}

#[test]
fn xms_a20_requests_are_balanced_and_hma_can_be_reclaimed() {
    let mut machine = Machine::new();
    let call = |machine: &mut Machine, function: u16| {
        machine.cpu.regs.ax = function << 8;
        machine.xms_call();
        (machine.cpu.regs.ax, machine.cpu.regs.bx as u8)
    };
    machine.cpu.regs.dx = 0xffff;
    assert_eq!(call(&mut machine, 1).0, 1);
    assert_eq!(call(&mut machine, 1), (0, 0x91));
    assert_eq!(call(&mut machine, 2).0, 1);
    assert_eq!(call(&mut machine, 2), (0, 0x93));
    assert_eq!(call(&mut machine, 1).0, 1);

    assert_eq!(call(&mut machine, 5).0, 1);
    assert_eq!(call(&mut machine, 5).0, 1);
    assert_eq!(call(&mut machine, 6).0, 1);
    assert_eq!(call(&mut machine, 7), (1, 0));
    assert_eq!(call(&mut machine, 6).0, 1);
    assert_eq!(call(&mut machine, 7), (0, 0));

    machine.fast_a20 = true;
    machine.refresh_a20();
    assert_eq!(call(&mut machine, 3).0, 1);
    assert_eq!(call(&mut machine, 4), (0, 0x94));
    assert_eq!(call(&mut machine, 7), (1, 0));
}

#[test]
fn xms_upper_memory_survives_save_and_restore_and_can_be_reused() {
    fn call(code: &mut Vec<u8>, function: u8, dx: u16, result: u8) {
        let [low, high] = dx.to_le_bytes();
        code.extend([
            0xba,
            low,
            high,
            0xb4,
            function,
            0xff,
            0x1e,
            0,
            4, // CALL FAR [0400h]
            0xa3,
            result,
            4,
            0x89,
            0x1e,
            result + 2,
            4,
            0x89,
            0x16,
            result + 4,
            4,
        ]);
    }

    let directory = Directory::new("xms-upper-memory");
    let mut code = vec![
        0xb8, 0x10, 0x43, 0xcd, 0x2f, // Read the installed XMS entry point.
        0x89, 0x1e, 0, 4, 0x8c, 0x06, 2, 4,
    ];
    call(&mut code, 0x10, 0xffff, 0x10);
    call(&mut code, 0x10, 0x1000, 0x16);
    code.extend([0x8e, 0xc3, 0x26, 0xc6, 0x06, 0, 0, 0xa5]); // Mark the first allocated block.
    call(&mut code, 0x10, 0x1000, 0x1c);
    call(&mut code, 0x10, 1, 0x22);
    call(&mut code, 0x11, 0xd001, 0x28);
    code.extend([0x31, 0xc0, 0xcd, 0x16]); // Save while both upper blocks are live.
    code.extend([0x26, 0xa0, 0, 0, 0xa2, 0x3a, 4]); // Read the original byte through ES.
    call(&mut code, 0x11, 0xd000, 0x2e);
    call(&mut code, 0x10, 1, 0x34);
    call(&mut code, 0x11, 0xe000, 0x40);
    call(&mut code, 0x11, 0xd000, 0x46);
    call(&mut code, 0x10, 0x2000, 0x4c);
    code.extend([0xb8, 0, 0x4c, 0xcd, 0x21]);
    directory.write("UMB.COM", &code);

    for profile in [crate::CpuProfile::I286, crate::CpuProfile::I386] {
        let mut machine = directory.machine();
        machine.set_cpu_profile(profile).unwrap();
        machine.launch('C', "UMB.COM", "").unwrap();
        assert_eq!(machine.run(256).unwrap(), RunState::WaitingForKey);
        assert!(!machine.cpu.a20_enabled());
        let checkpoint = machine.save_guest().unwrap();

        let mut restored = directory.machine();
        restored.load_guest(&checkpoint).unwrap();
        let result = |offset| {
            std::array::from_fn::<_, 3, _>(|index| {
                restored.cpu.read_u16(crate::PSP, offset + index as u16 * 2)
            })
        };
        let [ax, bx, dx] = result(0x410);
        assert_eq!((ax, bx as u8, dx), (0, 0xb0, 0x2000));
        assert_eq!(result(0x416), [1, 0xd000, 0x1000]);
        assert_eq!(result(0x41c), [1, 0xe000, 0x1000]);
        let [ax, bx, dx] = result(0x422);
        assert_eq!((ax, bx as u8, dx), (0, 0xb1, 0));
        let [ax, bx, _] = result(0x428);
        assert_eq!((ax, bx as u8), (0, 0xb2));
        restored.key(b'\r', 0x1c);
        assert_eq!(restored.run(256).unwrap(), RunState::Exited(0));
        assert_eq!(restored.cpu.read_u8(crate::PSP, 0x43a), 0xa5);
        let status = |offset| restored.cpu.read_u16(crate::PSP, offset);
        for offset in [0x42e, 0x434, 0x440, 0x446, 0x44c] {
            assert_eq!(status(offset), 1, "AX stored at {offset:04x}");
        }
        assert_eq!((status(0x436), status(0x438)), (0xd000, 1));
        assert_eq!((status(0x44e), status(0x450)), (0xd000, 0x2000));
    }
}

#[test]
fn high_program_restore_preserves_dos_and_xms_memory_boundaries() {
    let directory = Directory::new("high-program-save");
    directory.write(
        "HIGH.COM",
        &[
            0xbb, 0x40, 0, 0xb4, 0x4a, 0xcd, 0x21, // Resize the DOS program block.
            0xbb, 2, 0, 0xb4, 0x48, 0xcd, 0x21, 0xa3, 0, 2, // Allocate extra DOS space.
            0xb8, 0x10, 0x43, 0xcd, 0x2f, // Read the XMS far entry point.
            0x89, 0x1e, 2, 2, 0x8c, 0x06, 4, 2, 0xba, 1, 0, 0xb4, 0x10, 0xff, 0x1e, 2,
            2, // Request an XMS upper-memory block.
            0x89, 0x1e, 6, 2, 0x31, 0xc0, 0xcd,
            0x16, // Wait for a keystroke before restoring.
            0xb4, 0x62, 0xcd, 0x21, 0x89, 0x1e, 8, 2, // Get the restored program segment.
            0xbb, 3, 0, 0xb4, 0x48, 0xcd, 0x21, 0xa3, 0x0a, 2, // Allocate more DOS space.
            0xb8, 0, 0x4c, 0xcd, 0x21,
        ],
    );
    let mut machine = directory.machine();
    let io = machine
        .prepare_shell_io('C', &Default::default(), None, None, None)
        .unwrap();
    machine
        .launch_high_with_shell_io(
            'C',
            "HIGH.COM",
            "",
            io,
            crate::UpperMemoryRequest {
                minimum_bytes: 4096,
                shrink: true,
                ..crate::UpperMemoryRequest::default()
            },
        )
        .unwrap();
    assert_eq!(machine.run(256).unwrap(), RunState::WaitingForKey);
    let bytes = machine.save_guest().unwrap();

    let mut restored = directory.machine();
    restored.load_guest(&bytes).unwrap();
    assert_eq!(restored.blocks.get(&0xd000), Some(&0x40));
    assert_eq!(restored.blocks.get(&0xd041), Some(&2));
    assert_eq!(restored.xms.upper_blocks().get(&0xd100), Some(&1));
    restored.key(b'\r', 0x1c);
    assert_eq!(restored.run(256).unwrap(), RunState::Exited(0));
    let read = |offset| restored.cpu.read_u16(0xd000, offset);
    assert_eq!(read(2), 0xd040);
    assert_eq!(read(0x200), 0xd041);
    assert_eq!(read(0x206), 0xd100);
    assert_eq!(read(0x208), 0xd000);
    assert_eq!(read(0x20a), 0xd044);
}
