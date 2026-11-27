use super::*;

#[test]
fn bios_font_information_returns_guest_readable_rom_and_screen_geometry() {
    let (mut cpu, mut video) = setup();
    let glyphs = CodePage::Cp437.glyphs();
    for (selector, height, first, character) in [
        (2, 14, 0, b'A'),
        (3, 8, 0, b'A'),
        (4, 8, 128, 0x80),
        (6, 16, 0, b'A'),
    ] {
        bios(&mut video, &mut cpu, 0x1130, selector << 8, 0, 0x1200);
        assert_eq!(cpu.regs.cx, 16);
        assert_eq!(cpu.regs.dx, 0x1218);
        for row in 0..height {
            let offset = cpu.regs.bp + (u16::from(character - first) * height + row);
            assert_eq!(
                cpu.read_u8(cpu.regs.es, offset),
                glyphs[usize::from(character)][usize::from(row * 8 / height)].reverse_bits()
            );
        }
    }
    for selector in [5, 7] {
        bios(&mut video, &mut cpu, 0x1130, selector << 8, 0, 0);
        assert_eq!(cpu.read_u8(cpu.regs.es, cpu.regs.bp), 0);
    }

    bios(&mut video, &mut cpu, 0x13, 0, 0, 0);
    bios(&mut video, &mut cpu, 0x1130, 0x0100, 0, 0);
    assert_eq!(cpu.regs.cx, 8);
    assert_eq!(cpu.regs.dx, 24);
    assert_eq!(
        (cpu.regs.es, cpu.regs.bp),
        (VIDEO_FIRMWARE_SEGMENT, ROM_FONT_8)
    );
    for (selector, vector) in [(0, HIGH_FONT_VECTOR), (1, GRAPHICS_FONT_VECTOR)] {
        cpu.write_u16(0, vector, 0x4567);
        cpu.write_u16(0, vector + 2, 0x1234);
        bios(&mut video, &mut cpu, 0x1130, selector << 8, 0, 0);
        assert_eq!((cpu.regs.es, cpu.regs.bp), (0x1234, 0x4567));
    }

    // A legacy save has no firmware font tables and may already have custom pointers.
    cpu.write_u8(VIDEO_FIRMWARE_SEGMENT, ROM_FONTS_INSTALLED, 0);
    cpu.load(
        VIDEO_FIRMWARE_SEGMENT,
        ROM_FONT_8 + u16::from(b'A') * 8,
        &[0; 8],
    );
    bios(&mut video, &mut cpu, 0x1130, 0x0300, 0, 0);
    assert!(
        (0..8).any(|row| cpu.read_u8(cpu.regs.es, cpu.regs.bp + u16::from(b'A') * 8 + row) != 0)
    );
    assert_eq!(cpu.read_u16(0, GRAPHICS_FONT_VECTOR), 0x4567);
}

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
fn bios_graphics_text_uses_the_active_code_page() {
    let print = |page| {
        let (mut cpu, mut video) = setup();
        video.code_page = page;
        bios(&mut video, &mut cpu, 0x13, 0, 0, 0);
        bios(&mut video, &mut cpu, 0x099b, 15, 1, 0);
        std::array::from_fn::<_, 64, _>(|i| video.vga.pixel(i % 8, i / 8, None))
    };

    assert_ne!(print(CodePage::Cp437), print(CodePage::Cp850));
}

#[test]
fn wide_console_wraps_scrolls_and_reports_bios_stride() {
    for rows in [25, 43, 50] {
        let (mut cpu, mut video) = setup();
        assert!(video.set_console_geometry(&mut cpu, 132, rows));
        assert_eq!(cpu.read_u16(0x40, 0x4a), 132);
        assert!(usize::from(cpu.read_u16(0x40, 0x4c)) >= 132 * usize::from(rows) * 2);
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
fn bios_text_fonts_keep_independent_banks_and_survive_guest_restore() {
    let (mut cpu, mut video) = setup();
    bios(&mut video, &mut cpu, 0x1112, 0, 0, 0);
    expect_geometry(&video, &cpu, 50, 8, 0x2040);
    bios(&mut video, &mut cpu, 0x1104, 1, 0, 0);
    expect_geometry(&video, &cpu, 50, 8, 0x2040);
    // Attribute bit 3 chooses bank 1 after this BIOS block selector call.
    bios(&mut video, &mut cpu, 0x1103, 4, 0, 0);
    let glyph = CodePage::Cp437.glyphs()[usize::from(b'A')];
    let fonts = text(&video, &cpu).fonts.unwrap().read();
    assert!(fonts.uses_dual_maps());
    for (row, bitmap) in glyph.iter().enumerate() {
        assert_eq!(fonts.glyph(b'A', 7)[row], bitmap.reverse_bits());
        assert_eq!(fonts.glyph(b'A', 15)[row * 2], bitmap.reverse_bits());
        assert_eq!(fonts.glyph(b'A', 15)[row * 2 + 1], bitmap.reverse_bits());
    }

    cpu.regs.es = 0x2000;
    cpu.regs.bp = 0x200;
    cpu.load(0x2000, 0x200, &[0x80, 0x01, 0xff]);
    bios(&mut video, &mut cpu, 0x1100, 0x0301, 1, u16::from(b'A'));
    let registers_and_ram = cpu.save_snapshot().unwrap();
    let mut restored_cpu = Cpu::new();
    let mut restored = Adapter::from_snapshot(video.snapshot()).unwrap();
    assert!(restored.bind_snapshot_aperture(&mut restored_cpu));
    restored_cpu.restore_snapshot(&registers_and_ram).unwrap();
    let fonts = text(&restored, &restored_cpu).fonts.unwrap().read();
    assert_eq!(&fonts.glyph(b'A', 15)[..4], &[0x80, 0x01, 0xff, 0]);
    assert_eq!(fonts.glyph(b'A', 7)[0], glyph[0].reverse_bits());
    let (mut next_cpu, mut next) = setup();
    next.copy_console(&restored, &restored_cpu, &mut next_cpu);
    assert_eq!(
        text(&next, &next_cpu).fonts.unwrap().read().glyph(b'A', 15)[0],
        0x80
    );
    next.set_code_page(&mut next_cpu, CodePage::Cp850);
    let frame = text(&next, &next_cpu);
    assert_eq!(frame.code_page, CodePage::Cp850);
    assert!(frame.fonts.is_none());
    bios(&mut restored, &mut restored_cpu, 3, 0, 0, 0);
    assert!(text(&restored, &restored_cpu).fonts.is_none());
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

#[test]
fn ansi_text_erase_and_wrap_use_current_geometry_and_attributes() {
    let (mut cpu, mut video) = setup();
    video.ansi = Some(crate::ansi::Ansi::default());
    assert!(video.set_console_geometry(&mut cpu, 132, 43));

    video.write_dos(&mut cpu, b"\x1b[37;44m\x1b[2J\x1b[43;132H\x1b[=7lAB");
    let Video::Text(frame) = video.frame(&cpu) else {
        panic!("expected text mode");
    };
    assert_eq!(frame.cursor, Some((131, 42)));
    assert_eq!(&frame.cells[..2], b" \x17");
    assert_eq!(&frame.cells[frame.cells.len() - 2..], b"B\x17");

    video.write_dos(&mut cpu, b"\x1b[2D\x1b[K\x1b[=7h\x1b[43;132HZ");
    let Video::Text(frame) = video.frame(&cpu) else {
        panic!("expected text mode");
    };
    let cells = frame.cells.as_chunks::<2>().0;
    assert_eq!(frame.cursor, Some((0, 42)));
    assert_eq!(cells[132 * 42 - 2], [b' ', 0x17]);
    assert_eq!(cells[132 * 42 - 1], [b'Z', 0x17]);
    assert_eq!(cells[132 * 43 - 1], [b' ', 0x17]);
}

#[test]
fn video_snapshot_rebinds_the_aperture_and_replays_guest_drawing() {
    for mode in [0x12, 0x13] {
        let (mut cpu, mut video) = setup();
        assert!(video.set_mode(&mut cpu, mode));
        video.port_write(0x3c8, 5);
        video.port_write(0x3c9, 0x20); // Save a partially updated DAC entry.
        cpu.write_u8(0xa000, 0x123, 0x31);
        cpu.read_u8(0xa000, 0x123); // Populate the plane latches.
        cpu.regs.cs = 0x1000;
        cpu.regs.ds = 0xa000;
        cpu.regs.ip = 0x100;
        cpu.load(0x1000, 0x100, &[0xc6, 0x06, 0x24, 1, 0x73, 0xf4]);
        video.refresh(&cpu);
        let cpu_state = cpu.save_snapshot().unwrap();
        let video_state = postcard::to_allocvec(&video.snapshot()).unwrap();

        let mut restored_cpu = Cpu::new();
        let state: super::VideoSnapshot = postcard::from_bytes(&video_state).unwrap();
        let mut restored = Adapter::from_snapshot(state).unwrap();
        assert!(restored.bind_snapshot_aperture(&mut restored_cpu));
        restored_cpu.restore_snapshot(&cpu_state).unwrap();
        restored.refresh(&restored_cpu);
        assert_eq!(restored.vga.frame(), video.vga.frame());
        assert_eq!(restored.palette, video.palette);
        assert_eq!(
            postcard::to_allocvec(&restored.snapshot()).unwrap(),
            video_state
        );

        for (adapter, cpu) in [(&mut video, &mut cpu), (&mut restored, &mut restored_cpu)] {
            adapter.port_write(0x3c9, 3);
            adapter.port_write(0x3c9, 0x3f);
            assert_eq!(cpu.step().unwrap(), dosa_cpu::Step::Continue);
            assert_eq!(cpu.step().unwrap(), dosa_cpu::Step::Halt);
            adapter.refresh(cpu);
        }
        assert_eq!(video.vga.frame(), restored.vga.frame());
        assert_eq!(video.palette, restored.palette);
        assert_eq!(restored_cpu.peek_u8(0xa000, 0x124), Some(0x73));
    }
}

#[test]
fn video_snapshot_restores_text_geometry_and_rejects_invalid_modes() {
    let (mut cpu, mut video) = setup();
    assert!(video.set_console_geometry(&mut cpu, 132, 50));
    video.ansi = Some(crate::ansi::Ansi::default());
    video.write_dos(&mut cpu, b"\x1b[2;4H\x1b[3");
    let mut invalid = video.snapshot();
    invalid.mode = 0xff;
    assert!(Adapter::from_snapshot(invalid).is_none());

    let state = postcard::to_allocvec(&video.snapshot()).unwrap();
    let state = postcard::from_bytes(&state).unwrap();
    let mut restored = Adapter::from_snapshot(state).unwrap();
    let mut restored_cpu = Cpu::new();
    assert!(restored.bind_snapshot_aperture(&mut restored_cpu));
    restored_cpu
        .restore_snapshot(&cpu.save_snapshot().unwrap())
        .unwrap();
    restored.write_dos(&mut restored_cpu, b"1mX");
    let Video::Text(frame) = restored.frame(&restored_cpu) else {
        panic!("expected text mode");
    };
    assert_eq!((frame.columns, frame.rows), (132, 50));
    assert_eq!(frame.cursor, Some((4, 1)));
    assert_eq!(&frame.cells[(132 + 3) * 2..(132 + 4) * 2], b"X\x04");
}
