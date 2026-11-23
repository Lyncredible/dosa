use crate::vga::{Vga, WIDTH as VGA_WIDTH};
use dosa_cpu::Cpu;

const TEXT: usize = 0xb8000;
const COLS: usize = 80;
const ROWS: usize = 25;
const ROW_BYTES: usize = COLS * 2;
const TEXT_BYTES: usize = ROW_BYTES * ROWS;
const CGA_WIDTH: usize = 640;
const CGA_HEIGHT: usize = 200;
const CGA_STRIDE: usize = CGA_WIDTH / 8;
const CGA_BANK: usize = 0x2000;

#[derive(Clone, Copy)]
pub struct TextFrame<'a> {
    pub columns: u16,
    pub rows: u16,
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

pub(crate) struct Adapter {
    mode: u8,
    col: usize,
    row: usize,
    cursor: bool,
    palette: [[u8; 3]; 256],
    dac_read: usize,
    dac_write: usize,
    retrace: bool,
    cga_pixels: Box<[u8]>,
    cga_color: u8,
    saved_text: Option<Box<[u8]>>,
    vga: Vga,
}

impl Adapter {
    pub(crate) fn new() -> Self {
        Self {
            mode: 3,
            col: 0,
            row: 0,
            cursor: true,
            palette: default_palette(),
            dac_read: 0,
            dac_write: 0,
            retrace: false,
            cga_pixels: vec![0; CGA_WIDTH * CGA_HEIGHT].into_boxed_slice(),
            cga_color: 15,
            saved_text: None,
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
            0x13 => Video::Graphics {
                width: VGA_WIDTH as u32,
                height: (self.vga.frame().len() / VGA_WIDTH) as u32,
                indices: self.vga.frame(),
                palette: &self.palette,
            },
            _ => Video::Text(TextFrame {
                columns: COLS as u16,
                rows: ROWS as u16,
                cells: &cpu.memory[TEXT..TEXT + TEXT_BYTES],
                cursor: self.cursor.then_some((self.col as u16, self.row as u16)),
            }),
        }
    }

    pub(crate) fn refresh(&mut self, cpu: &Cpu) {
        if self.mode == 0x13 {
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
        if self.mode != 3 {
            self.set_mode(cpu, 0x83);
        }
        self.cursor = true;
    }

    pub(crate) fn copy_console(&mut self, previous: &Self, from: &Cpu, to: &mut Cpu) {
        to.memory[TEXT..TEXT + TEXT_BYTES].copy_from_slice(&from.memory[TEXT..TEXT + TEXT_BYTES]);
        self.col = previous.col;
        self.row = previous.row;
        self.update_cursor(to);
    }

    fn set_mode(&mut self, cpu: &mut Cpu, value: u8) -> bool {
        let mode = value & 0x7f;
        if !matches!(mode, 0x03 | 0x06 | 0x13) {
            return false;
        }
        if mode == 6 && self.mode != 6 {
            self.saved_text = Some(
                cpu.memory[TEXT..TEXT + TEXT_BYTES]
                    .to_vec()
                    .into_boxed_slice(),
            );
        } else if mode != 6
            && let Some(text) = self.saved_text.take()
        {
            cpu.memory[TEXT..TEXT + TEXT_BYTES].copy_from_slice(&text);
        }
        let clear = value & 0x80 == 0;
        if mode == 0x13 {
            self.vga.enter(cpu, self.mode == 0x13, clear);
        } else if self.mode == 0x13 {
            self.vga.leave(cpu);
        }
        self.mode = mode;
        self.palette = default_palette();
        self.cga_color = 15;
        if clear {
            match mode {
                3 => self.clear(cpu),
                6 => cpu.memory[TEXT..TEXT + 2 * CGA_BANK].fill(0),
                _ => {}
            }
        }
        cpu.write_u8(0x40, 0x49, mode);
        cpu.write_u16(0x40, 0x4a, self.columns().into());
        cpu.write_u16(
            0x40,
            0x4c,
            match mode {
                3 => 0x1000,
                6 => 0x4000,
                _ => 0xfa00,
            },
        );
        cpu.write_u8(0x40, 0x84, 24);
        true
    }

    pub(crate) fn clear(&mut self, cpu: &mut Cpu) {
        for cell in cpu.memory[TEXT..TEXT + TEXT_BYTES].as_chunks_mut::<2>().0 {
            *cell = [b' ', 0x07];
        }
        self.col = 0;
        self.row = 0;
        self.update_cursor(cpu);
    }

    fn update_cursor(&self, cpu: &mut Cpu) {
        cpu.write_u16(0x40, 0x50, (self.row as u16) << 8 | self.col as u16);
    }

    pub(crate) fn erase_previous(&mut self, cpu: &mut Cpu) {
        let cursor = self.row * COLS + self.col;
        let Some(previous) = cursor.checked_sub(1) else {
            return;
        };
        self.row = previous / COLS;
        self.col = previous % COLS;
        cpu.memory[TEXT + previous * 2] = b' ';
        self.update_cursor(cpu);
    }

    pub(crate) fn write(&mut self, cpu: &mut Cpu, bytes: &[u8]) {
        if self.mode == 6 {
            return;
        }
        for &byte in bytes {
            match byte {
                b'\r' => self.col = 0,
                b'\n' => self.row += 1,
                8 => self.col = self.col.saturating_sub(1),
                7 => {}
                b'\t' => {
                    let spaces = 8 - self.col % 8;
                    self.write(cpu, &b"        "[..spaces]);
                }
                _ => {
                    let cell = TEXT + self.row * ROW_BYTES + self.col * 2;
                    cpu.memory[cell] = byte;
                    self.col += 1;
                    if self.col == COLS {
                        self.col = 0;
                        self.row += 1;
                    }
                }
            }
            if self.row == ROWS {
                self.scroll(cpu, [0, 0, 24, 79], 1, 0x07, false);
                self.row = ROWS - 1;
            }
        }
        self.update_cursor(cpu);
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
            usize::from(bottom).min(ROWS - 1),
            usize::from(right).min(COLS - 1),
        );
        if top > bottom || left > right {
            return;
        }
        let count = if count == 0 {
            bottom - top + 1
        } else {
            usize::from(count)
        };
        let row_start = |row| TEXT + ROW_BYTES * row + left * 2;
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
            0x01 => self.cursor = ch & 0x20 == 0,
            0x02 => {
                self.col = usize::from(dl).min(COLS - 1);
                self.row = usize::from(dh).min(ROWS - 1);
                self.update_cursor(cpu);
            }
            0x03 => {
                cpu.regs.dx = (self.row as u16) << 8 | self.col as u16;
                cpu.regs.cx = if self.cursor { 0x0607 } else { 0x2007 };
            }
            0x05 => {}
            0x06 | 0x07 if self.mode == 3 => {
                self.scroll(cpu, [ch, cl, dh, dl], al, bh, ah == 7);
            }
            0x08 if self.mode == 3 => {
                let start = TEXT + (self.row * COLS + self.col) * 2;
                cpu.regs.ax = u16::from_le_bytes([cpu.memory[start], cpu.memory[start + 1]]);
            }
            0x09 | 0x0a if self.mode == 3 => {
                let start = self.row * COLS + self.col;
                let end = (start + usize::from(cpu.regs.cx)).min(COLS * ROWS);
                for cell in start..end {
                    cpu.memory[TEXT + cell * 2] = al;
                    if ah == 0x09 {
                        cpu.memory[TEXT + cell * 2 + 1] = bl;
                    }
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
            0x0c | 0x0d if self.mode == 0x13 => {
                let x = usize::from(cpu.regs.cx);
                let y = usize::from(cpu.regs.dx);
                if x < VGA_WIDTH && y < self.vga.height() {
                    let pixel = self.vga.pixel(x, y, (ah == 0x0c).then_some(al));
                    if ah == 0x0d {
                        cpu.regs.ax = u16::from_le_bytes([pixel, ah]);
                    }
                }
            }
            0x0e => self.write(cpu, &[al]),
            0x0f => {
                cpu.regs.ax = u16::from_le_bytes([self.mode, self.columns()]);
                cpu.regs.bx &= 0x00ff;
            }
            0x10 if al == 0x10 && cpu.regs.bx < 256 => {
                self.palette[usize::from(cpu.regs.bx)] = [dh, ch, cl].map(expand_dac);
            }
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
            0x12 if bl == 0x10 => {
                cpu.regs.bx = 3;
                cpu.regs.cx = 0;
            }
            0x1a if al == 0 => {
                cpu.regs.ax = 0x1a;
                cpu.regs.bx = 8;
            }
            _ => return false,
        }
        true
    }

    fn columns(&self) -> u8 {
        if self.mode == 0x13 { 40 } else { 80 }
    }

    pub(crate) fn port_read(&mut self, port: u16) -> Option<u8> {
        match port {
            0x3c9 => {
                let byte = self.palette[self.dac_read / 3][self.dac_read % 3] >> 2;
                self.dac_read = (self.dac_read + 1) % 768;
                Some(byte)
            }
            0x3da => {
                self.retrace = !self.retrace;
                Some(if self.retrace { 0x09 } else { 0 })
            }
            _ => self.vga.read_port(port),
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

pub(crate) fn initialize(cpu: &mut Cpu, video: &mut Adapter) {
    video.set_mode(cpu, 3);
    cpu.write_u16(0x40, 0x13, 640);
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
