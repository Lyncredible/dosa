use crate::{Machine, RunState, Video};
use dosa_mount::Mount;
use std::{fs, path::PathBuf};

struct Directory(PathBuf);

impl Directory {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("dosa-dos-{}-{name}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn machine(&self) -> Machine {
        let mut machine = Machine::new();
        machine.mount('C', Mount::open(&self.0).unwrap()).unwrap();
        machine
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn com_reads_key_then_prints_and_exits() {
    let directory = Directory::new("keyboard");
    fs::write(
        directory.0.join("KEY.COM"),
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
    fs::write(directory.0.join("IRQ.COM"), program).unwrap();
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
    fs::write(directory.0.join("VGA.EXE"), mz).unwrap();
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
    let nested = directory.0.join("PROGRAMS");
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
        directory.0.join("CGA.COM"),
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
