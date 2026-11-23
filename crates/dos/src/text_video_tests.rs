use super::*;

fn setup() -> (Cpu, Adapter) {
    let mut cpu = Cpu::new();
    let mut video = Adapter::new();
    initialize(&mut cpu, &mut video);
    (cpu, video)
}

fn text<'a>(video: &'a Adapter, cpu: &'a Cpu) -> TextFrame<'a> {
    let Video::Text(frame) = video.frame(cpu) else {
        panic!("expected text frame")
    };
    frame
}

fn bios(video: &mut Adapter, cpu: &mut Cpu, ax: u16, bx: u16, cx: u16, dx: u16) {
    cpu.regs.ax = ax;
    cpu.regs.bx = bx;
    cpu.regs.cx = cx;
    cpu.regs.dx = dx;
    assert!(video.bios(cpu));
}

fn expect_geometry(video: &Adapter, cpu: &Cpu, rows: u16, height: u16, page_bytes: u16) {
    let frame = text(video, cpu);
    assert_eq!(
        (frame.columns, frame.rows, frame.character_height),
        (80, rows, height)
    );
    assert_eq!(frame.cells.len(), usize::from(rows) * ROW_BYTES);
    assert_eq!(cpu.read_u16(0x40, 0x4a), 80);
    assert_eq!(cpu.read_u8(0x40, 0x84), (rows - 1) as u8);
    assert_eq!(cpu.read_u16(0x40, 0x85), height);
    assert_eq!(cpu.read_u16(0x40, 0x4c), page_bytes);
}

#[test]
fn bios_scan_line_selection_and_font_height_update_geometry() {
    let (mut cpu, mut video) = setup();
    expect_geometry(&video, &cpu, 25, 16, 0x1000);

    bios(&mut video, &mut cpu, 0x1201, 0x0030, 0, 0);
    assert_eq!(cpu.regs.ax, 0x1212);
    assert_eq!(cpu.read_u8(0x40, 0x89) & 0x90, 0);
    expect_geometry(&video, &cpu, 25, 16, 0x1000);
    bios(&mut video, &mut cpu, 0x0083, 0, 0, 0);
    expect_geometry(&video, &cpu, 25, 14, 0x1000);
    bios(&mut video, &mut cpu, 0x1112, 0, 0, 0);
    expect_geometry(&video, &cpu, 43, 8, 0x1be0);

    bios(&mut video, &mut cpu, 0x1202, 0x0030, 0, 0);
    bios(&mut video, &mut cpu, 0x0083, 0, 0, 0);
    bios(&mut video, &mut cpu, 0x1112, 0, 0, 0);
    expect_geometry(&video, &cpu, 50, 8, 0x2040);
    bios(&mut video, &mut cpu, 0x1114, 0, 0, 0);
    expect_geometry(&video, &cpu, 25, 16, 0x10a0);
    assert_eq!(cpu.read_u8(0x40, 0x89) & 0x90, 0x10);
    video.restore_text(&mut cpu);
    expect_geometry(&video, &cpu, 25, 16, 0x1000);
}

#[test]
fn console_scroll_and_backspace_are_bounded_by_current_geometry() {
    let (mut cpu, mut video) = setup();
    assert!(video.set_console_rows(&mut cpu, 43));
    let boundary = TEXT + 43 * ROW_BYTES;
    cpu.memory[boundary] = 0xcd;
    bios(&mut video, &mut cpu, 0x0200, 0, 0, 0xffff);
    assert_eq!(text(&video, &cpu).cursor, Some((79, 42)));
    assert_eq!(cpu.read_u16(0x40, 0x50), 0x2a4f);

    bios(&mut video, &mut cpu, 0x0958, 0x000f, u16::MAX, 0);
    assert_eq!(&cpu.memory[boundary - 2..=boundary], &[b'X', 0x0f, 0xcd]);
    video.write(&mut cpu, b"Z");
    assert_eq!(text(&video, &cpu).cursor, Some((0, 42)));
    let previous = boundary - ROW_BYTES - 2;
    assert_eq!(cpu.memory[previous], b'Z');
    video.erase_previous(&mut cpu);
    assert_eq!(text(&video, &cpu).cursor, Some((79, 41)));
    assert_eq!(cpu.memory[previous], b' ');
    assert_eq!(cpu.memory[boundary], 0xcd);
    let previous_shape = cpu.read_u16(0x40, 0x60);
    bios(&mut video, &mut cpu, 0x0100, 0, 0x2007, 0);
    assert_eq!(text(&video, &cpu).cursor, None);
    assert_ne!(cpu.read_u16(0x40, 0x60), previous_shape);
    bios(&mut video, &mut cpu, 0x0300, 0, 0, 0);
    assert_eq!(cpu.regs.cx, 0x2007);
    assert_eq!(cpu.regs.dx, 0x294f);
}

#[test]
fn console_geometry_and_tail_cells_survive_copy_and_graphics() {
    let (mut from, mut previous) = setup();
    assert!(previous.set_console_rows(&mut from, 50));
    assert!(!previous.set_console_rows(&mut from, 42));
    bios(&mut previous, &mut from, 0x0200, 0, 0, 0x3105);
    previous.write(&mut from, b"Y");

    let (mut cpu, mut video) = setup();
    video.copy_console(&previous, &from, &mut cpu);
    expect_geometry(&video, &cpu, 50, 8, 0x2040);
    let index = (49 * COLS + 5) * 2;
    assert_eq!(text(&video, &cpu).cells[index], b'Y');
    assert_eq!(text(&video, &cpu).cursor, Some((6, 49)));

    bios(&mut video, &mut cpu, 0x0006, 0, 0, 0);
    assert!(matches!(video.frame(&cpu), Video::Graphics { .. }));
    video.restore_text(&mut cpu);
    expect_geometry(&video, &cpu, 50, 8, 0x2040);
    assert_eq!(text(&video, &cpu).cells[index], b'Y');
    assert_eq!(text(&video, &cpu).cursor, Some((6, 49)));

    bios(&mut video, &mut cpu, 0x0083, 0, 0, 0);
    expect_geometry(&video, &cpu, 25, 16, 0x1000);
    assert_eq!(text(&video, &cpu).cursor, Some((6, 24)));
    video.restore_text(&mut cpu);
    expect_geometry(&video, &cpu, 50, 8, 0x2040);
    assert_eq!(text(&video, &cpu).cells[index], b'Y');
}
#[test]
fn mode_twelve_reports_graphics_and_bios_geometry() {
    let mut cpu = dosa_cpu::Cpu::new();
    let mut video = super::Adapter::new();
    cpu.regs.ax = 0x12;
    assert!(video.bios(&mut cpu));
    assert_eq!(cpu.read_u16(0x40, 0x4a), 80);
    assert_eq!(cpu.read_u16(0x40, 0x4c), 0xa000);
    assert_eq!(cpu.read_u8(0x40, 0x84), 29);
    assert_eq!(cpu.read_u16(0x40, 0x85), 16);
    video.refresh(&cpu);
    let super::Video::Graphics {
        width,
        height,
        indices,
        ..
    } = video.frame(&cpu)
    else {
        panic!("expected graphics");
    };
    assert_eq!((width, height, indices.len()), (640, 480, 640 * 480));
    cpu.regs.ax = 3;
    assert!(video.bios(&mut cpu));
    assert_eq!(cpu.read_u8(0x40, 0x84), 24);
}
