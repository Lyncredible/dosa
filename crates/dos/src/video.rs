use crate::{
    CodePage,
    ansi::{Ansi, Byte},
    vga::{Vga, VgaSnapshot, VgaTextFonts},
};
use dosa_cpu::Cpu;

#[cfg(test)]
#[path = "text_video_tests.rs"]
mod text_tests;

const TEXT: usize = 0xb8000;
// The firmware flag is part of CPU RAM, so older guest snapshots still decode.
const VIDEO_FIRMWARE_SEGMENT: u16 = 0xf000;
const FONT_FIRMWARE_FLAG: u16 = 0x0780;
const BLINK_FIRMWARE_FLAG: u16 = 0x0781;
const ROM_FONTS_INSTALLED: u16 = 0x0782;
const ROM_FONT_8: u16 = 0x5000;
const ROM_FONT_14: u16 = 0x5800;
const ROM_FONT_16: u16 = 0x6600;
const ROM_ALTERNATE_FONT: u16 = 0x7600;
const HIGH_FONT_VECTOR: u16 = 0x1f * 4;
const GRAPHICS_FONT_VECTOR: u16 = 0x43 * 4;
const COLS: usize = 80;
#[cfg(test)]
const ROW_BYTES: usize = COLS * 2;
const MAX_TEXT_BYTES: usize = 132 * 2 * 50;
const CGA_WIDTH: usize = 640;
const CGA_HEIGHT: usize = 200;
const CGA_STRIDE: usize = CGA_WIDTH / 8;
const CGA_BANK: usize = 0x2000;

#[derive(Clone, Copy)]
pub struct TextFrame<'a> {
    pub columns: u16,
    pub rows: u16,
    pub character_height: u16,
    pub code_page: CodePage,
    pub fonts: Option<VgaTextFonts<'a>>,
    pub blink_enabled: bool,
    pub cells: &'a [u8],
    pub cursor: Option<(u16, u16)>,
}

pub enum Video<'a> {
    Text(TextFrame<'a>),
    Graphics {
        width: u32,
        height: u32,
        indices: &'a [u8],
        palette: &'a [[u8; 3]; 256],
    },
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
struct TextGeometry {
    columns: usize,
    scan_lines: u16,
    character_height: u16,
}

impl TextGeometry {
    fn valid_snapshot(self) -> bool {
        matches!(self.columns, 80 | 132)
            && matches!(self.scan_lines, 200 | 350 | 400)
            && matches!(self.character_height, 8 | 14 | 16)
            && self.text_bytes() <= MAX_TEXT_BYTES
    }

    const VGA_DEFAULT: Self = Self::standard(400);

    const fn standard(scan_lines: u16) -> Self {
        let character_height = match scan_lines {
            200 => 8,
            350 => 14,
            _ => 16,
        };
        Self {
            columns: COLS,
            scan_lines,
            character_height,
        }
    }

    const fn rows(self) -> usize {
        (self.scan_lines / self.character_height) as usize
    }

    const fn text_bytes(self) -> usize {
        self.columns * 2 * self.rows()
    }

    const fn font_page_bytes(self) -> u16 {
        // DOSBox-compatible BIOS page padding after a font-height change.
        self.text_bytes() as u16 + 0x100
    }

    const fn console_page_bytes(self) -> u16 {
        if self.columns == 80 && self.rows() == 25 {
            0x1000
        } else {
            self.font_page_bytes()
        }
    }

    const fn cursor_shape(self) -> u16 {
        (self.character_height - 2) << 8 | (self.character_height - 1)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct VideoSnapshot {
    code_page: CodePage,
    ansi: Option<Ansi>,
    mode: u8,
    col: u16,
    row: u16,
    cursor_shape: u16,
    text: TextGeometry,
    console: TextGeometry,
    next_scan_lines: u16,
    palette: Box<[[u8; 3]]>,
    dac_read: u16,
    dac_write: u16,
    retrace: bool,
    cga_color: u8,
    saved_text: Option<Box<[u8]>>,
    saved_text_cursor: Option<(u16, u16)>,
    vga: VgaSnapshot,
}

pub(crate) struct Adapter {
    pub(crate) code_page: CodePage,
    pub(crate) ansi: Option<Ansi>,
    mode: u8,
    col: usize,
    row: usize,
    cursor_shape: u16,
    text: TextGeometry,
    console: TextGeometry,
    next_scan_lines: u16,
    palette: [[u8; 3]; 256],
    dac_read: usize,
    dac_write: usize,
    retrace: bool,
    cga_pixels: Box<[u8]>,
    cga_color: u8,
    saved_text: Option<Box<[u8]>>,
    saved_text_cursor: Option<(usize, usize)>,
    vga: Vga,
}

impl Adapter {
    pub(crate) fn snapshot(&self) -> VideoSnapshot {
        VideoSnapshot {
            code_page: self.code_page,
            ansi: self.ansi,
            mode: self.mode,
            col: self.col as u16,
            row: self.row as u16,
            cursor_shape: self.cursor_shape,
            text: self.text,
            console: self.console,
            next_scan_lines: self.next_scan_lines,
            palette: Box::new(self.palette),
            dac_read: self.dac_read as u16,
            dac_write: self.dac_write as u16,
            retrace: self.retrace,
            cga_color: self.cga_color,
            saved_text: self.saved_text.clone(),
            saved_text_cursor: self
                .saved_text_cursor
                .map(|(col, row)| (col as u16, row as u16)),
            vga: self.vga.snapshot(),
        }
    }

    pub(crate) fn from_snapshot(state: VideoSnapshot) -> Option<Self> {
        let col = usize::from(state.col);
        let row = usize::from(state.row);
        if !matches!(state.mode, 3 | 6 | 0x12 | 0x13)
            || !state.text.valid_snapshot()
            || !state.console.valid_snapshot()
            || !matches!(state.next_scan_lines, 200 | 350 | 400)
            || state.dac_read >= 768
            || state.dac_write >= 768
            || state.cga_color > 15
            || state.cursor_shape & !0x3f1f != 0
            || state
                .ansi
                .as_ref()
                .is_some_and(|ansi| !ansi.valid_snapshot())
            || state.mode == 3 && (col >= state.text.columns || row >= state.text.rows())
            || col > 255
            || row > 255
            || (state.mode == 6) != state.saved_text.is_some()
            || state
                .saved_text
                .as_ref()
                .is_some_and(|text| text.len() != MAX_TEXT_BYTES)
        {
            return None;
        }
        let palette = *Box::<[[u8; 3]; 256]>::try_from(state.palette).ok()?;
        Some(Self {
            code_page: state.code_page,
            ansi: state.ansi,
            mode: state.mode,
            col,
            row,
            cursor_shape: state.cursor_shape,
            text: state.text,
            console: state.console,
            next_scan_lines: state.next_scan_lines,
            palette,
            dac_read: state.dac_read.into(),
            dac_write: state.dac_write.into(),
            retrace: state.retrace,
            cga_pixels: vec![0; CGA_WIDTH * CGA_HEIGHT].into_boxed_slice(),
            cga_color: state.cga_color,
            saved_text: state.saved_text,
            saved_text_cursor: state
                .saved_text_cursor
                .map(|(col, row)| (col.into(), row.into())),
            vga: Vga::from_snapshot(state.vga, state.mode)?,
        })
    }

    pub(crate) fn bind_snapshot_aperture(&self, cpu: &mut Cpu) -> bool {
        if matches!(self.mode, 0x12 | 0x13) {
            self.vga.bind_aperture(cpu)
        } else {
            cpu.unmap_memory();
            true
        }
    }

    pub(crate) fn new() -> Self {
        Self {
            code_page: CodePage::default(),
            ansi: None,
            mode: 3,
            col: 0,
            row: 0,
            cursor_shape: TextGeometry::VGA_DEFAULT.cursor_shape(),
            text: TextGeometry::VGA_DEFAULT,
            console: TextGeometry::VGA_DEFAULT,
            next_scan_lines: TextGeometry::VGA_DEFAULT.scan_lines,
            palette: default_palette(),
            dac_read: 0,
            dac_write: 0,
            retrace: false,
            cga_pixels: vec![0; CGA_WIDTH * CGA_HEIGHT].into_boxed_slice(),
            cga_color: 15,
            saved_text: None,
            saved_text_cursor: None,
            vga: Vga::new(),
        }
    }

    pub(crate) fn frame<'a>(&'a self, cpu: &'a Cpu) -> Video<'a> {
        match self.mode {
            6 => Video::Graphics {
                width: CGA_WIDTH as u32,
                height: CGA_HEIGHT as u32,
                indices: &self.cga_pixels,
                palette: &self.palette,
            },
            0x12 | 0x13 => Video::Graphics {
                width: self.vga.width() as u32,
                height: (self.vga.frame().len() / self.vga.width()) as u32,
                indices: self.vga.frame(),
                palette: &self.palette,
            },
            _ => Video::Text(TextFrame {
                columns: self.text.columns as u16,
                rows: self.text.rows() as u16,
                character_height: self.text.character_height,
                code_page: self.code_page,
                fonts: Self::bios_fonts_active(cpu).then(|| self.vga.text_fonts()),
                blink_enabled: self.vga.blink_enabled(),
                cells: &cpu.memory[TEXT..TEXT + self.text.text_bytes()],
                cursor: (self.cursor_shape & 0x2000 == 0)
                    .then_some((self.col as u16, self.row as u16)),
            }),
        }
    }

    pub(crate) fn refresh(&mut self, cpu: &Cpu) {
        if matches!(self.mode, 0x12 | 0x13) {
            self.vga.refresh();
            return;
        }
        if self.mode != 6 {
            return;
        }
        for (y, pixels) in self
            .cga_pixels
            .as_chunks_mut::<CGA_WIDTH>()
            .0
            .iter_mut()
            .enumerate()
        {
            let start = cga_scanline(y);
            for (byte, group) in cpu.memory[start..start + CGA_STRIDE]
                .iter()
                .zip(pixels.as_chunks_mut::<8>().0)
            {
                for (x, pixel) in group.iter_mut().enumerate() {
                    *pixel = ((byte >> (7 - x)) & 1) * self.cga_color;
                }
            }
        }
    }

    pub(crate) fn restore_text(&mut self, cpu: &mut Cpu) {
        let row = self.saved_text_cursor.map_or(self.row, |(_, row)| row);
        if self.mode != 3 {
            self.set_mode(cpu, 0x83);
        }
        self.next_scan_lines = self.console.scan_lines;
        self.row = row;
        self.set_text_geometry(cpu, self.console, self.console.console_page_bytes());
        self.set_cursor_shape(cpu, self.cursor_shape & !0x2000);
        self.update_scan_line_selection(cpu);
    }

    pub(crate) fn copy_console(&mut self, previous: &Self, from: &Cpu, to: &mut Cpu) {
        to.memory[TEXT..TEXT + MAX_TEXT_BYTES]
            .copy_from_slice(&from.memory[TEXT..TEXT + MAX_TEXT_BYTES]);
        self.code_page = previous.code_page;
        self.ansi = previous.ansi;
        if previous.mode == 3 {
            self.set_blink_enabled(to, previous.vga.blink_enabled());
            if Self::bios_fonts_active(from) {
                self.vga.copy_text_fonts(&previous.vga);
                to.write_u8(VIDEO_FIRMWARE_SEGMENT, FONT_FIRMWARE_FLAG, 1);
            }
        }
        self.console = previous.console;
        self.next_scan_lines = self.console.scan_lines;
        self.col = previous.col;
        self.row = previous.row;
        self.set_text_geometry(to, self.console, self.console.console_page_bytes());
        self.update_scan_line_selection(to);
    }

    pub(crate) fn set_console_rows(&mut self, cpu: &mut Cpu, rows: u8) -> bool {
        self.set_console_geometry(cpu, self.console.columns as u16, rows)
    }

    pub(crate) fn set_console_geometry(&mut self, cpu: &mut Cpu, columns: u16, rows: u8) -> bool {
        if !matches!(columns, 80 | 132) {
            return false;
        }
        self.console = match rows {
            25 => TextGeometry::VGA_DEFAULT,
            43 => TextGeometry {
                columns: COLS,
                scan_lines: 350,
                character_height: 8,
            },
            50 => TextGeometry {
                columns: COLS,
                scan_lines: 400,
                character_height: 8,
            },
            _ => return false,
        };
        self.console.columns = usize::from(columns);
        self.restore_text(cpu);
        true
    }

    fn set_mode(&mut self, cpu: &mut Cpu, value: u8) -> bool {
        let mode = value & 0x7f;
        if !matches!(mode, 0x03 | 0x06 | 0x12 | 0x13) {
            return false;
        }
        if mode == 6 && self.mode != 6 {
            self.saved_text = Some(
                cpu.memory[TEXT..TEXT + MAX_TEXT_BYTES]
                    .to_vec()
                    .into_boxed_slice(),
            );
        } else if mode != 6
            && let Some(text) = self.saved_text.take()
        {
            cpu.memory[TEXT..TEXT + MAX_TEXT_BYTES].copy_from_slice(&text);
        }
        let clear = value & 0x80 == 0;
        if mode != 3 && self.mode == 3 {
            self.saved_text_cursor = Some((self.col, self.row));
        }
        if mode == 3 && self.mode != 3 {
            if let Some((col, row)) = self.saved_text_cursor.take() {
                self.col = col;
                self.row = row;
            }
        } else if mode != 3 {
            self.col = 0;
            self.row = 0;
        }
        if matches!(mode, 0x12 | 0x13) {
            self.vga
                .enter(cpu, matches!(self.mode, 0x12 | 0x13), clear, mode == 0x12);
        } else if matches!(self.mode, 0x12 | 0x13) {
            self.vga.leave(cpu);
        }
        self.mode = mode;
        cpu.write_u8(VIDEO_FIRMWARE_SEGMENT, FONT_FIRMWARE_FLAG, 0);
        self.palette = default_palette();
        if mode == 0x12 {
            for (index, color) in self.palette[..64].iter_mut().enumerate() {
                *color = std::array::from_fn(|channel| {
                    let low_bit = 2 - channel;
                    let high_bit = 5 - channel;
                    u8::from(index & (1 << low_bit) != 0) * 170
                        + u8::from(index & (1 << high_bit) != 0) * 85
                });
            }
        }
        self.cga_color = 15;
        if mode == 3 {
            self.set_blink_enabled(cpu, true);
            self.set_text_geometry(cpu, TextGeometry::standard(self.next_scan_lines), 0x1000);
        }
        if clear {
            match mode {
                3 => self.clear(cpu),
                6 => cpu.memory[TEXT..TEXT + 2 * CGA_BANK].fill(0),
                _ => {}
            }
        }
        cpu.write_u8(0x40, 0x49, mode);
        cpu.write_u16(0x40, 0x4a, self.columns().into());
        if mode != 3 {
            let page_bytes = match mode {
                6 => 0x4000,
                0x12 => 0xa000,
                _ => 0xfa00,
            };
            cpu.write_u16(0x40, 0x4c, page_bytes);
            cpu.write_u8(0x40, 0x84, if mode == 0x12 { 29 } else { 24 });
            cpu.write_u16(0x40, 0x85, if mode == 0x12 { 16 } else { 8 });
        }
        set_font_vector(cpu, GRAPHICS_FONT_VECTOR, rom_font_for_screen(cpu));
        self.update_cursor(cpu);
        true
    }

    pub(crate) fn clear(&mut self, cpu: &mut Cpu) {
        blank_text(&mut cpu.memory[TEXT..TEXT + self.text.text_bytes()]);
        self.col = 0;
        self.row = 0;
        self.update_cursor(cpu);
    }

    fn update_cursor(&self, cpu: &mut Cpu) {
        cpu.write_u16(0x40, 0x50, (self.row as u16) << 8 | self.col as u16);
    }

    fn set_cursor_shape(&mut self, cpu: &mut Cpu, shape: u16) {
        self.cursor_shape = shape & 0x3f1f;
        cpu.write_u16(0x40, 0x60, self.cursor_shape);
    }

    fn set_text_geometry(&mut self, cpu: &mut Cpu, geometry: TextGeometry, page_bytes: u16) {
        self.text = geometry;
        self.col = self.col.min(geometry.columns - 1);
        cpu.write_u16(0x40, 0x4a, geometry.columns as u16);
        self.row = self.row.min(geometry.rows() - 1);
        cpu.write_u16(0x40, 0x4c, page_bytes);
        cpu.write_u8(0x40, 0x84, (geometry.rows() - 1) as u8);
        cpu.write_u16(0x40, 0x85, geometry.character_height);
        self.set_cursor_shape(cpu, geometry.cursor_shape() | (self.cursor_shape & 0x2000));
        self.update_cursor(cpu);
    }

    fn update_scan_line_selection(&self, cpu: &mut Cpu) {
        let previous = cpu.read_u8(0x40, 0x89) & !0x90;
        let selection = match self.next_scan_lines {
            200 => 0x80,
            400 => 0x10,
            _ => 0,
        };
        cpu.write_u8(0x40, 0x89, previous | selection);
    }

    pub(crate) fn erase_previous(&mut self, cpu: &mut Cpu) {
        let cursor = self.row * self.text.columns + self.col;
        let Some(previous) = cursor.checked_sub(1) else {
            return;
        };
        self.row = previous / self.text.columns;
        self.col = previous % self.text.columns;
        cpu.memory[TEXT + previous * 2] = b' ';
        self.update_cursor(cpu);
    }

    pub(crate) fn write_dos(&mut self, cpu: &mut Cpu, bytes: &[u8]) {
        let Some(mut ansi) = self.ansi.take() else {
            return self.write(cpu, bytes);
        };
        if self.mode == 3 {
            for &byte in bytes {
                match ansi.consume(byte) {
                    Byte::Text(byte) => {
                        self.write_byte(cpu, byte, Some(ansi.attribute()), ansi.wrap)
                    }
                    Byte::LiteralEscape(byte) => {
                        self.write_byte(cpu, 27, Some(ansi.attribute()), ansi.wrap);
                        self.write_byte(cpu, byte, Some(ansi.attribute()), ansi.wrap);
                    }
                    Byte::Pending => {}
                    Byte::Command(command) => self.apply_ansi(cpu, &mut ansi, command),
                }
            }
            self.update_cursor(cpu);
        } else {
            self.write(cpu, bytes);
        }
        self.ansi = Some(ansi);
    }

    fn apply_ansi(&mut self, cpu: &mut Cpu, ansi: &mut Ansi, command: u8) {
        let value = ansi.parameter(0);
        if ansi.mode_command() {
            if value == 7 && matches!(command, b'h' | b'l') {
                ansi.wrap = command == b'h';
            }
            return;
        }
        let bottom = self.text.rows() - 1;
        let right = self.text.columns - 1;
        let distance = usize::from(value.max(1));
        match command {
            b'A' => self.row = self.row.saturating_sub(distance),
            b'B' => self.row = (self.row + distance).min(bottom),
            b'C' => self.col = (self.col + distance).min(right),
            b'D' => self.col = self.col.saturating_sub(distance),
            b'H' | b'f' => {
                self.row = (distance - 1).min(bottom);
                self.col = usize::from(ansi.parameter(1).saturating_sub(1)).min(right);
            }
            b'J' if value == 2 => {
                self.blank_ansi(
                    cpu,
                    0,
                    self.text.rows() * self.text.columns,
                    ansi.attribute(),
                );
                self.row = 0;
                self.col = 0;
            }
            b'K' if value == 0 => {
                let row_start = self.row * self.text.columns;
                self.blank_ansi(
                    cpu,
                    row_start + self.col,
                    row_start + self.text.columns,
                    ansi.attribute(),
                );
            }
            b'm' => ansi.set_graphics_rendition(),
            b's' => ansi.saved_cursor = Some((self.col, self.row)),
            b'u' => {
                if let Some((col, row)) = ansi.saved_cursor {
                    self.col = col.min(right);
                    self.row = row.min(bottom);
                }
            }
            _ => {}
        }
    }

    fn blank_ansi(&self, cpu: &mut Cpu, start: usize, end: usize, attr: u8) {
        for cell in cpu.memory[TEXT + start * 2..TEXT + end * 2]
            .as_chunks_mut::<2>()
            .0
        {
            *cell = [b' ', attr];
        }
    }

    pub(crate) fn write(&mut self, cpu: &mut Cpu, bytes: &[u8]) {
        if self.mode == 6 {
            return;
        }
        for &byte in bytes {
            self.write_byte(cpu, byte, None, true);
        }
        self.update_cursor(cpu);
    }

    fn write_byte(&mut self, cpu: &mut Cpu, byte: u8, attr: Option<u8>, wrap: bool) {
        match byte {
            b'\r' => self.col = 0,
            b'\n' => self.row += 1,
            8 => self.col = self.col.saturating_sub(1),
            7 => {}
            b'\t' => {
                let spaces = 8 - self.col % 8;
                for _ in 0..spaces {
                    self.write_byte(cpu, b' ', attr, wrap);
                }
            }
            _ => {
                let cell = TEXT + self.row * self.text.columns * 2 + self.col * 2;
                cpu.memory[cell] = byte;
                if let Some(attr) = attr {
                    cpu.memory[cell + 1] = attr;
                }
                if self.col + 1 < self.text.columns {
                    self.col += 1;
                } else if wrap {
                    self.col = 0;
                    self.row += 1;
                }
            }
        }
        if self.row == self.text.rows() {
            let bottom = self.text.rows() - 1;
            self.scroll(
                cpu,
                [0, 0, bottom as u8, (self.text.columns - 1) as u8],
                1,
                attr.unwrap_or(0x07),
                false,
            );
            self.row = bottom;
        }
    }

    fn graphics_character(&self, x: usize, y: usize, character: u8, color: u8) {
        let height = if self.mode == 0x12 { 16 } else { 8 };
        let glyph = self.code_page.glyphs()[usize::from(character)];
        for row in 0..height {
            for column in 0..8 {
                if x + column >= self.vga.width() || y + row >= self.vga.height() {
                    continue;
                }
                let foreground = glyph[row * 8 / height] & (1 << column) != 0;
                if foreground || self.mode != 0x12 || color & 0x80 == 0 {
                    self.vga.pixel(
                        x + column,
                        y + row,
                        Some(if foreground { color } else { 0 }),
                    );
                }
            }
        }
    }

    fn graphics_teletype(&mut self, character: u8, color: u8) {
        let height = if self.mode == 0x12 { 16 } else { 8 };
        let columns = self.vga.width() / 8;
        let rows = (self.vga.height() / height).max(1);
        match character {
            7 => return,
            8 => self.col = self.col.saturating_sub(1),
            13 => self.col = 0,
            10 => self.row += 1,
            _ => {
                self.graphics_character(self.col * 8, self.row * height, character, color);
                self.col += 1;
                if self.col >= columns {
                    self.col = 0;
                    self.row += 1;
                }
            }
        }
        if self.row >= rows {
            self.vga.scroll_up(height);
            self.row = rows - 1;
        }
    }

    fn scroll(
        &self,
        cpu: &mut Cpu,
        [top, left, bottom, right]: [u8; 4],
        count: u8,
        attr: u8,
        down: bool,
    ) {
        let (top, left, bottom, right) = (
            usize::from(top),
            usize::from(left),
            usize::from(bottom).min(self.text.rows() - 1),
            usize::from(right).min(self.text.columns - 1),
        );
        if top > bottom || left > right {
            return;
        }
        let count = if count == 0 {
            bottom - top + 1
        } else {
            usize::from(count)
        };
        let row_start = |row| TEXT + self.text.columns * 2 * row + left * 2;
        let width = (right + 1 - left) * 2;
        for amount in 0..=bottom - top {
            let row = if down { bottom - amount } else { top + amount };
            let start = row_start(row);
            if amount + count > bottom - top {
                for cell in cpu.memory[start..start + width].as_chunks_mut::<2>().0 {
                    *cell = [b' ', attr];
                }
            } else {
                let source = row_start(if down { row - count } else { row + count });
                cpu.memory.copy_within(source..source + width, start);
            }
        }
    }

    pub(crate) fn bios(&mut self, cpu: &mut Cpu) -> bool {
        let [al, ah] = cpu.regs.ax.to_le_bytes();
        let [bl, bh] = cpu.regs.bx.to_le_bytes();
        let [cl, ch] = cpu.regs.cx.to_le_bytes();
        let [dl, dh] = cpu.regs.dx.to_le_bytes();
        match ah {
            0x00 => return self.set_mode(cpu, al),
            0x01 => self.set_cursor_shape(cpu, cpu.regs.cx),
            0x02 => {
                self.col = usize::from(dl).min(usize::from(self.columns()) - 1);
                let rows = match self.mode {
                    0x12 => 30,
                    0x13 => self.vga.height() / 8,
                    _ => self.text.rows(),
                };
                self.row = usize::from(dh).min(rows.saturating_sub(1));
                self.update_cursor(cpu);
            }
            0x03 => {
                cpu.regs.dx = (self.row as u16) << 8 | self.col as u16;
                cpu.regs.cx = self.cursor_shape;
            }
            0x05 => {}
            0x06 | 0x07 if self.mode == 3 => {
                self.scroll(cpu, [ch, cl, dh, dl], al, bh, ah == 7);
            }
            0x08 if self.mode == 3 => {
                let start = TEXT + (self.row * self.text.columns + self.col) * 2;
                cpu.regs.ax = u16::from_le_bytes([cpu.memory[start], cpu.memory[start + 1]]);
            }
            0x09 | 0x0a if self.mode == 3 => {
                let start = self.row * self.text.columns + self.col;
                let end =
                    (start + usize::from(cpu.regs.cx)).min(self.text.columns * self.text.rows());
                for cell in start..end {
                    cpu.memory[TEXT + cell * 2] = al;
                    if ah == 0x09 {
                        cpu.memory[TEXT + cell * 2 + 1] = bl;
                    }
                }
            }
            0x09 | 0x0a if matches!(self.mode, 0x12 | 0x13) => {
                let columns = self.vga.width() / 8;
                let height = if self.mode == 0x12 { 16 } else { 8 };
                let start = self.row * columns + self.col;
                let end =
                    (start + usize::from(cpu.regs.cx)).min(columns * (self.vga.height() / height));
                for cell in start..end {
                    self.graphics_character(cell % columns * 8, cell / columns * height, al, bl);
                }
            }
            0x0b if self.mode == 6 && bh == 0 => self.cga_color = bl & 15,
            0x0c | 0x0d if self.mode == 6 => {
                let x = usize::from(cpu.regs.cx);
                let y = usize::from(cpu.regs.dx);
                if x < CGA_WIDTH && y < CGA_HEIGHT {
                    let byte = &mut cpu.memory[cga_scanline(y) + x / 8];
                    let mask = 1 << (7 - x % 8);
                    let color = (al & 1) * mask;
                    if ah == 0x0d {
                        cpu.regs.ax = u16::from_le_bytes([u8::from(*byte & mask != 0), ah]);
                    } else if al & 0x80 != 0 {
                        *byte ^= color;
                    } else {
                        *byte = (*byte & !mask) | color;
                    }
                }
            }
            0x0c | 0x0d if matches!(self.mode, 0x12 | 0x13) => {
                let x = usize::from(cpu.regs.cx);
                let y = usize::from(cpu.regs.dx);
                if x < self.vga.width() && y < self.vga.height() {
                    let pixel = self.vga.pixel(x, y, (ah == 0x0c).then_some(al));
                    if ah == 0x0d {
                        cpu.regs.ax = u16::from_le_bytes([pixel, ah]);
                    }
                }
            }
            0x0e if matches!(self.mode, 0x12 | 0x13) => {
                self.graphics_teletype(al, bl);
                self.update_cursor(cpu);
            }
            0x0e => self.write(cpu, &[al]),
            0x13 if matches!(self.mode, 0x12 | 0x13) => {
                let previous = (self.col, self.row);
                self.col = usize::from(dl).min(usize::from(self.columns()) - 1);
                let height = if self.mode == 0x12 { 16 } else { 8 };
                self.row = usize::from(dh).min((self.vga.height() / height).saturating_sub(1));
                let mut offset = cpu.regs.bp;
                for _ in 0..cpu.regs.cx {
                    let character = cpu.read_u8(cpu.regs.es, offset);
                    offset = offset.wrapping_add(1);
                    let color = if al & 2 != 0 {
                        let color = cpu.read_u8(cpu.regs.es, offset);
                        offset = offset.wrapping_add(1);
                        color
                    } else {
                        bl
                    };
                    self.graphics_teletype(character, color);
                }
                if al & 1 == 0 {
                    (self.col, self.row) = previous;
                }
                self.update_cursor(cpu);
            }
            0x0f => {
                cpu.regs.ax = u16::from_le_bytes([self.mode, self.columns()]);
                cpu.regs.bx &= 0x00ff;
            }
            0x10 if al == 0x10 && cpu.regs.bx < 256 => {
                self.palette[usize::from(cpu.regs.bx)] = [dh, ch, cl].map(expand_dac);
            }
            0x10 if al == 0 && bl < 16 => self.vga.set_attribute(bl, bh),
            0x10 if al == 1 => self.vga.set_attribute(0x11, bh),
            0x10 if al == 2 => {
                for index in 0..16 {
                    self.vga.set_attribute(
                        index,
                        cpu.read_u8(cpu.regs.es, cpu.regs.dx.wrapping_add(u16::from(index))),
                    );
                }
                self.vga
                    .set_attribute(0x11, cpu.read_u8(cpu.regs.es, cpu.regs.dx.wrapping_add(16)));
            }
            0x10 if al == 3 && bl <= 1 => self.set_blink_enabled(cpu, bl == 1),
            0x10 if al == 0x12 => {
                let start = usize::from(cpu.regs.bx).min(256);
                let count = usize::from(cpu.regs.cx).min(256 - start);
                for i in 0..count {
                    self.palette[start + i] = [0, 1, 2]
                        .map(|channel| {
                            cpu.read_u8(
                                cpu.regs.es,
                                cpu.regs.dx.wrapping_add((i * 3 + channel) as u16),
                            )
                        })
                        .map(expand_dac);
                }
            }
            0x11 if al == 0x30 => return bios_font_info(cpu, bh),
            0x11 if self.mode == 3 => return self.bios_text_font(cpu, al, bh, bl),
            0x12 if bl == 0x10 => {
                cpu.regs.bx = 3;
                cpu.regs.cx = 0;
            }
            0x12 if bl == 0x30 && al <= 2 => {
                self.next_scan_lines = [200, 350, 400][usize::from(al)];
                self.update_scan_line_selection(cpu);
                cpu.regs.ax = u16::from_le_bytes([0x12, ah]);
            }
            0x1a if al == 0 => {
                cpu.regs.ax = 0x1a;
                cpu.regs.bx = 8;
            }
            // A VGA-only BIOS returns without the VBE signature in AL.
            0x4f if al == 0 => {}
            _ => return false,
        }
        true
    }

    fn bios_fonts_active(cpu: &Cpu) -> bool {
        cpu.read_u8(VIDEO_FIRMWARE_SEGMENT, FONT_FIRMWARE_FLAG) == 1
    }

    pub(crate) fn set_code_page(&mut self, cpu: &mut Cpu, page: CodePage) {
        self.code_page = page;
        cpu.write_u8(VIDEO_FIRMWARE_SEGMENT, FONT_FIRMWARE_FLAG, 0);
    }

    fn activate_bios_fonts(&mut self, cpu: &mut Cpu) {
        if !Self::bios_fonts_active(cpu) {
            self.vga.select_text_fonts(0);
            self.load_rom_text_font(0, self.text.character_height);
            cpu.write_u8(VIDEO_FIRMWARE_SEGMENT, FONT_FIRMWARE_FLAG, 1);
        }
    }

    fn load_rom_text_font(&mut self, bank: u8, height: u16) {
        let height = usize::from(height);
        let glyphs = CodePage::Cp437.glyphs().iter().map(|bitmap| {
            std::array::from_fn(|row| {
                if row < height {
                    bitmap[row * 8 / height].reverse_bits()
                } else {
                    0
                }
            })
        });
        self.vga.load_text_font(bank, 0, glyphs);
    }

    fn bios_text_font(&mut self, cpu: &mut Cpu, function: u8, height: u8, bank: u8) -> bool {
        if function == 3 {
            self.activate_bios_fonts(cpu);
            self.vga.select_text_fonts(bank);
            return true;
        }
        if bank > 7 {
            return false;
        }
        let update_geometry = function & 0x10 != 0;
        let height = match function {
            0x00 | 0x10 if (1..=32).contains(&height) => u16::from(height),
            0x01 | 0x11 => 14,
            0x02 | 0x12 => 8,
            0x04 | 0x14 => 16,
            _ => return false,
        };
        if update_geometry && !matches!(height, 8 | 14 | 16) {
            return false;
        }
        if matches!(function, 0x00 | 0x10) {
            let first = usize::from(cpu.regs.dx);
            let count = usize::from(cpu.regs.cx);
            if first + count > 256 {
                return false;
            }
            self.activate_bios_fonts(cpu);
            let base = cpu.regs.bp;
            let segment = cpu.regs.es;
            let glyphs = (0..count)
                .map(|glyph| {
                    let offset = base.wrapping_add(glyph as u16 * height);
                    std::array::from_fn(|row| {
                        if (row as u16) < height {
                            cpu.read_u8(segment, offset.wrapping_add(row as u16))
                        } else {
                            0
                        }
                    })
                })
                .collect::<Vec<_>>();
            self.vga.load_text_font(bank, first, glyphs.into_iter());
        } else {
            self.activate_bios_fonts(cpu);
            self.load_rom_text_font(bank, height);
        }
        if update_geometry {
            let geometry = TextGeometry {
                character_height: height,
                ..self.text
            };
            self.set_text_geometry(cpu, geometry, geometry.font_page_bytes());
        }
        true
    }

    fn columns(&self) -> u8 {
        match self.mode {
            0x13 => 40,
            3 => self.text.columns as u8,
            _ => 80,
        }
    }

    pub(crate) fn port_read(&mut self, port: u16) -> Option<u8> {
        match port {
            0x3c9 => {
                let byte = self.palette[self.dac_read / 3][self.dac_read % 3] >> 2;
                self.dac_read = (self.dac_read + 1) % 768;
                Some(byte)
            }
            0x3da => {
                self.vga.reset_attribute_latch();
                self.retrace = !self.retrace;
                Some(if self.retrace { 0x09 } else { 0 })
            }
            _ => self.vga.read_port(port),
        }
    }

    fn set_blink_enabled(&mut self, cpu: &mut Cpu, enabled: bool) {
        self.vga.set_blink_enabled(enabled);
        let control = cpu.read_u8(0x40, 0x65);
        cpu.write_u8(0x40, 0x65, (control & !0x20) | (u8::from(enabled) << 5));
        cpu.write_u8(VIDEO_FIRMWARE_SEGMENT, BLINK_FIRMWARE_FLAG, 1);
    }

    pub(crate) fn restore_legacy_blink_default(&mut self, cpu: &mut Cpu) {
        if self.mode == 3 && cpu.read_u8(VIDEO_FIRMWARE_SEGMENT, BLINK_FIRMWARE_FLAG) == 0 {
            self.set_blink_enabled(cpu, true);
        }
    }

    pub(crate) fn port_write(&mut self, port: u16, value: u8) {
        match port {
            0x3c7 => self.dac_read = usize::from(value) * 3,
            0x3c8 => self.dac_write = usize::from(value) * 3,
            0x3c9 => {
                self.palette[self.dac_write / 3][self.dac_write % 3] = expand_dac(value);
                self.dac_write = (self.dac_write + 1) % 768;
            }
            0x3d9 if self.mode == 6 => self.cga_color = value & 15,
            _ => self.vga.write_port(port, value),
        }
    }
}

fn cga_scanline(y: usize) -> usize {
    TEXT + (y % 2) * CGA_BANK + (y / 2) * CGA_STRIDE
}

fn set_font_vector(cpu: &mut Cpu, vector: u16, offset: u16) {
    cpu.write_u16(0, vector, offset);
    cpu.write_u16(0, vector + 2, VIDEO_FIRMWARE_SEGMENT);
}

fn rom_font_for_screen(cpu: &Cpu) -> u16 {
    match cpu.read_u16(0x40, 0x85) {
        8 => ROM_FONT_8,
        14 => ROM_FONT_14,
        _ => ROM_FONT_16,
    }
}

fn install_rom_font_tables(cpu: &mut Cpu) {
    if cpu.read_u8(VIDEO_FIRMWARE_SEGMENT, ROM_FONTS_INSTALLED) == 1 {
        return;
    }
    let glyphs = CodePage::Cp437.glyphs();
    for (offset, height) in [(ROM_FONT_8, 8), (ROM_FONT_14, 14), (ROM_FONT_16, 16)] {
        let bytes: Vec<u8> = glyphs
            .iter()
            .flat_map(|bitmap| (0..height).map(move |row| bitmap[row * 8 / height].reverse_bits()))
            .collect();
        cpu.load(VIDEO_FIRMWARE_SEGMENT, offset, &bytes);
    }
    // The built-in font uses the same glyphs in 8- and 9-dot layouts.
    cpu.write_u8(VIDEO_FIRMWARE_SEGMENT, ROM_ALTERNATE_FONT, 0);
    for (vector, offset) in [
        (HIGH_FONT_VECTOR, ROM_FONT_8 + 128 * 8),
        (GRAPHICS_FONT_VECTOR, rom_font_for_screen(cpu)),
    ] {
        if cpu.read_u16(0, vector) == 0 && cpu.read_u16(0, vector + 2) == 0 {
            set_font_vector(cpu, vector, offset);
        }
    }
    cpu.write_u8(VIDEO_FIRMWARE_SEGMENT, ROM_FONTS_INSTALLED, 1);
}

fn bios_font_info(cpu: &mut Cpu, selector: u8) -> bool {
    let offset = match selector {
        0 => HIGH_FONT_VECTOR,
        1 => GRAPHICS_FONT_VECTOR,
        2 => ROM_FONT_14,
        3 => ROM_FONT_8,
        4 => ROM_FONT_8 + 128 * 8,
        5 | 7 => ROM_ALTERNATE_FONT,
        6 => ROM_FONT_16,
        _ => return false,
    };
    install_rom_font_tables(cpu);
    let (segment, pointer) = if selector <= 1 {
        (cpu.read_u16(0, offset + 2), cpu.read_u16(0, offset))
    } else {
        (VIDEO_FIRMWARE_SEGMENT, offset)
    };
    cpu.regs.es = segment;
    cpu.regs.bp = pointer;
    cpu.regs.cx = cpu.read_u16(0x40, 0x85);
    cpu.regs.dx = (cpu.regs.dx & 0xff00) | u16::from(cpu.read_u8(0x40, 0x84));
    true
}

pub(crate) fn initialize(cpu: &mut Cpu, video: &mut Adapter) {
    install_rom_font_tables(cpu);
    blank_text(&mut cpu.memory[TEXT..TEXT + MAX_TEXT_BYTES]);
    video.set_mode(cpu, 3);
    video.update_scan_line_selection(cpu);
    cpu.write_u16(0x40, 0x13, 640);
}

fn blank_text(memory: &mut [u8]) {
    for cell in memory.as_chunks_mut::<2>().0 {
        *cell = [b' ', 0x07];
    }
}

fn expand_dac(value: u8) -> u8 {
    let value = value & 63;
    value << 2 | value >> 4
}

fn default_palette() -> [[u8; 3]; 256] {
    let mut palette = [[0; 3]; 256];
    for (index, color) in palette[..16].iter_mut().enumerate() {
        let intensity = if index & 8 == 0 { 0 } else { 85 };
        for (channel, value) in color.iter_mut().enumerate() {
            *value = intensity + if index & (4 >> channel) == 0 { 0 } else { 170 };
        }
    }
    palette[6] = [170, 85, 0];
    for (index, color) in palette[16..32].iter_mut().enumerate() {
        *color = [index as u8 * 17; 3];
    }
    let levels = [0, 51, 102, 153, 204, 255];
    for (index, color) in palette[32..248].iter_mut().enumerate() {
        *color = [levels[index / 36], levels[index / 6 % 6], levels[index % 6]];
    }
    palette
}
