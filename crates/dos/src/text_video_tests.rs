use super::*;

#[test]
fn graphics_bios_text_draws_wraps_and_keeps_repeat_cursor() {
    for (mode, columns, height) in [(0x12, 80, 16), (0x13, 40, 8)] {
        let (mut cpu, mut video) = setup();
        bios(&mut video, &mut cpu, mode, 0, 0, 0);
        bios(&mut video, &mut cpu, 0x0200, 0, 0, columns - 1);
        bios(&mut video, &mut cpu, 0x0941, 15, 2, 0);
        assert_eq!((video.col, video.row), (usize::from(columns - 1), 0));
        assert!((0..8).any(|x| (0..height).any(|y| video.vga.pixel(
            (usize::from(columns) - 1) * 8 + x,
            y,
            None
        ) != 0)));
        bios(&mut video, &mut cpu, 0x0e42, 15, 0, 0);
        assert_eq!((video.col, video.row), (0, 1));
        bios(&mut video, &mut cpu, 0x0200, 0, 0, 0xffff);
        assert_eq!(video.col, usize::from(columns) - 1);
        let bottom = video.row;
        bios(&mut video, &mut cpu, 0x0e0a, 15, 0, 0);
        assert_eq!(video.row, bottom);
    }
}

#[test]
fn graphics_text_handles_tiny_crtc_height_and_restores_text_cursor() {
    let (mut cpu, mut video) = setup();
    bios(&mut video, &mut cpu, 0x0200, 0, 0, 0x0304);
    bios(&mut video, &mut cpu, 0x12, 0, 0, 0);
    assert_eq!((video.col, video.row), (0, 0));
    assert_eq!(cpu.read_u16(0x40, 0x50), 0);
    video.port_write(0x3d4, 7);
    video.port_write(0x3d5, 0);
    video.port_write(0x3d4, 0x12);
    video.port_write(0x3d5, 0);
    bios(&mut video, &mut cpu, 0x0200, 0, 0, 0xffff);
    bios(&mut video, &mut cpu, 0x0e0a, 15, 0, 0);
    cpu.regs.es = 0x2000;
    cpu.regs.bp = 0;
    cpu.write_u8(0x2000, 0, b'A');
    bios(&mut video, &mut cpu, 0x1300, 15, 1, 0xffff);
    assert_eq!(video.row, 0);
    video.restore_text(&mut cpu);
    assert_eq!((video.col, video.row), (4, 3));
}

#[test]
fn graphics_string_colors_and_cp437_blocks_are_visible() {
    let (mut cpu, mut video) = setup();
    bios(&mut video, &mut cpu, 0x13, 0, 0, 0);
    cpu.regs.es = 0x2000;
    cpu.regs.bp = 0;
    cpu.load(0x2000, 0, &[0xdb, 5, 0xdb, 6]);
    bios(&mut video, &mut cpu, 0x1303, 0, 2, 0);
    assert_eq!((video.col, video.row), (2, 0));
    assert_eq!(video.vga.pixel(0, 0, None), 5);
    assert_eq!(video.vga.pixel(8, 0, None), 6);
}

#[test]
fn wide_console_wraps_scrolls_and_reports_bios_stride() {
    for rows in [25, 43, 50] {
        let (mut cpu, mut video) = setup();
        assert!(video.set_console_geometry(&mut cpu, 132, rows));
        assert_eq!(cpu.read_u16(0x40, 0x4a), 132);
        let frame = text(&video, &cpu);
        assert_eq!((frame.columns, frame.rows), (132, u16::from(rows)));
        assert_eq!(frame.cells.len(), 132 * usize::from(rows) * 2);
        video.write(&mut cpu, &vec![b'X'; 132 * usize::from(rows)]);
        assert_eq!((video.col, video.row), (0, usize::from(rows) - 1));
        video.erase_previous(&mut cpu);
        assert_eq!((video.col, video.row), (131, usize::from(rows) - 2));
        assert!(video.set_console_rows(&mut cpu, 25));
        assert_eq!(text(&video, &cpu).columns, 132);
        assert!(!video.set_console_geometry(&mut cpu, 256, 25));
        assert_eq!(text(&video, &cpu).columns, 132);
    }
}

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
    for (index, expected) in [(0x10, 1), (0x12, 15)] {
        video.port_read(0x3da);
        video.port_write(0x3c0, index);
        assert_eq!(video.port_read(0x3c1), Some(expected));
    }
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
