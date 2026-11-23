//! The 256-color VGA aperture and indexed register files used by Mode X.

use dosa_cpu::{Cpu, MemoryDevice};
use std::sync::{Arc, Mutex, MutexGuard};

const APERTURE: usize = 0xa0000;
const PLANE_BYTES: usize = 0x10000;
pub(crate) const WIDTH: usize = 320;

const CRTC_MODE_13: [u8; 25] = [
    0x5f, 0x4f, 0x50, 0x82, 0x54, 0x80, 0xbf, 0x1f, 0x00, 0x41, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x9c, 0x8e, 0x8f, 0x28, 0x40, 0x96, 0xb9, 0xa3, 0xff,
];

pub(crate) struct Vga {
    ram: Arc<Mutex<PlaneRam>>,
    sequencer_index: u8,
    graphics_index: u8,
    crtc_index: u8,
    crtc: [u8; CRTC_MODE_13.len()],
    frame: Vec<u8>,
}

impl Vga {
    pub(crate) fn new() -> Self {
        Self {
            ram: Arc::new(Mutex::new(PlaneRam::new())),
            sequencer_index: 0,
            graphics_index: 0,
            crtc_index: 0,
            crtc: CRTC_MODE_13,
            frame: vec![0; WIDTH * 200],
        }
    }

    pub(crate) fn enter(&mut self, cpu: &mut Cpu, already_vga: bool, clear: bool) {
        self.sequencer_index = 0;
        self.graphics_index = 0;
        self.crtc_index = 0;
        self.crtc = CRTC_MODE_13;
        {
            let mut ram = lock(&self.ram);
            ram.reset_registers();
            if clear {
                ram.cells.fill([0; 4]);
                self.frame.fill(0);
            }
        }
        if !already_vga {
            let mapped =
                cpu.map_memory(APERTURE, PLANE_BYTES, Box::new(Aperture(self.ram.clone())));
            debug_assert!(mapped);
        }
    }

    pub(crate) fn leave(&mut self, cpu: &mut Cpu) {
        cpu.unmap_memory();
    }

    pub(crate) fn frame(&self) -> &[u8] {
        &self.frame
    }

    pub(crate) fn refresh(&mut self) {
        self.frame.resize(WIDTH * self.height(), 0);
        let ram = lock(&self.ram);
        let (start, stride) = self.start_and_stride();
        for (y, scanline) in self.frame.as_chunks_mut::<WIDTH>().0.iter_mut().enumerate() {
            let address = start + y * stride;
            for (x, pixels) in scanline.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                *pixels = ram.cells[(address + x) % PLANE_BYTES];
            }
        }
    }

    pub(crate) fn height(&self) -> usize {
        let overflow = usize::from(self.crtc[7]);
        let display_end =
            usize::from(self.crtc[0x12]) | (overflow & 0x02) << 7 | (overflow & 0x40) << 3;
        let max_scan = self.crtc[9];
        let repeat = usize::from(max_scan & 0x1f) + 1;
        let double_scan = if max_scan & 0x80 != 0 { 2 } else { 1 };
        (display_end + 1).div_ceil(repeat * double_scan)
    }

    pub(crate) fn pixel(&self, x: usize, y: usize, write: Option<u8>) -> u8 {
        let (start, stride) = self.start_and_stride();
        let address = (start + y * stride + x / 4) % PLANE_BYTES;
        let mut ram = lock(&self.ram);
        let pixel = &mut ram.cells[address][x % 4];
        if let Some(value) = write {
            *pixel = value;
        }
        *pixel
    }

    fn start_and_stride(&self) -> (usize, usize) {
        let start = u16::from_be_bytes([self.crtc[0x0c], self.crtc[0x0d]]);
        (start.into(), usize::from(self.crtc[0x13]) * 2)
    }

    pub(crate) fn read_port(&self, port: u16) -> Option<u8> {
        match port {
            0x3c4 => Some(self.sequencer_index),
            0x3c5 => lock(&self.ram)
                .sequencer
                .get(usize::from(self.sequencer_index))
                .copied(),
            0x3ce => Some(self.graphics_index),
            0x3cf => lock(&self.ram)
                .graphics
                .get(usize::from(self.graphics_index))
                .copied(),
            0x3d4 => Some(self.crtc_index),
            0x3d5 => self.crtc.get(usize::from(self.crtc_index)).copied(),
            _ => None,
        }
    }

    pub(crate) fn write_port(&mut self, port: u16, value: u8) {
        match port {
            0x3c4 => self.sequencer_index = value,
            0x3c5 => {
                if let Some(reg) = lock(&self.ram)
                    .sequencer
                    .get_mut(usize::from(self.sequencer_index))
                {
                    *reg = value;
                }
            }
            0x3ce => self.graphics_index = value,
            0x3cf => {
                if let Some(reg) = lock(&self.ram)
                    .graphics
                    .get_mut(usize::from(self.graphics_index))
                {
                    *reg = value;
                }
            }
            0x3d4 => self.crtc_index = value,
            0x3d5 => {
                if let Some(reg) = self.crtc.get_mut(usize::from(self.crtc_index)) {
                    *reg = value;
                }
            }
            _ => {}
        }
    }
}

fn lock(ram: &Mutex<PlaneRam>) -> MutexGuard<'_, PlaneRam> {
    ram.lock().unwrap_or_else(|poison| poison.into_inner())
}

struct Aperture(Arc<Mutex<PlaneRam>>);

impl MemoryDevice for Aperture {
    fn read(&self, offset: usize) -> u8 {
        lock(&self.0).read(offset)
    }

    fn write(&self, offset: usize, value: u8) {
        lock(&self.0).write(offset, value);
    }
}

struct PlaneRam {
    cells: Box<[[u8; 4]]>,
    latches: [u8; 4],
    sequencer: [u8; 5],
    graphics: [u8; 9],
}

impl PlaneRam {
    fn new() -> Self {
        let mut ram = Self {
            cells: vec![[0; 4]; PLANE_BYTES].into_boxed_slice(),
            latches: [0; 4],
            sequencer: [0; 5],
            graphics: [0; 9],
        };
        ram.reset_registers();
        ram
    }

    fn reset_registers(&mut self) {
        self.latches = [0; 4];
        self.sequencer = [3, 1, 0x0f, 0, 0x0e];
        self.graphics = [0, 0, 0, 0, 0, 0x40, 0x05, 0x0f, 0xff];
    }

    fn address_and_mask(&self, offset: usize) -> (usize, u8) {
        let mask = self.sequencer[2] & 0x0f;
        if self.sequencer[4] & 8 != 0 {
            (offset / 4, mask & (1 << (offset % 4)))
        } else {
            (offset, mask)
        }
    }

    fn read(&mut self, offset: usize) -> u8 {
        let (address, _) = self.address_and_mask(offset);
        self.latches = self.cells[address];
        if self.graphics[5] & 8 == 0 {
            let plane = if self.sequencer[4] & 8 != 0 {
                offset % 4
            } else {
                usize::from(self.graphics[4] & 3)
            };
            return self.latches[plane];
        }
        let compare = self.graphics[2];
        let care = self.graphics[7];
        self.latches
            .iter()
            .enumerate()
            .fold(0xff, |result, (plane, &latch)| {
                let bit = 1 << plane;
                if care & bit != 0 {
                    result & !(latch ^ fill(compare & bit != 0))
                } else {
                    result
                }
            })
    }

    fn write(&mut self, offset: usize, value: u8) {
        let (address, map_mask) = self.address_and_mask(offset);
        let mode = self.graphics[5] & 3;
        let operation = self.graphics[3] >> 3 & 3;
        let rotate = u32::from(self.graphics[3] & 7);
        let rotated = value.rotate_right(rotate);
        let mut bit_mask = self.graphics[8];
        if mode == 3 {
            bit_mask &= rotated;
        }
        for (plane, pixel) in self.cells[address].iter_mut().enumerate() {
            let bit = 1 << plane;
            if map_mask & bit == 0 {
                continue;
            }
            let latch = self.latches[plane];
            let set_reset = fill(self.graphics[0] & bit != 0);
            let data = match mode {
                0 if self.graphics[1] & bit != 0 => set_reset,
                0 => rotated,
                1 => {
                    *pixel = latch;
                    continue;
                }
                2 => fill(value & bit != 0),
                _ => set_reset,
            };
            let combined = match operation {
                1 => data & latch,
                2 => data | latch,
                3 => data ^ latch,
                _ => data,
            };
            *pixel = (combined & bit_mask) | (latch & !bit_mask);
        }
    }
}

fn fill(bit: bool) -> u8 {
    if bit { 0xff } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planar_writes_latch_copies_and_scanout_share_video_memory() {
        let mut vga = Vga::new();
        let mut cpu = Cpu::new();
        vga.enter(&mut cpu, false, true);
        let ram = 0xa000;
        cpu.write_u16(ram, 0, 0x0201);
        vga.refresh();
        assert_eq!(&vga.frame()[..4], &[1, 2, 0, 0]);

        vga.write_port(0x3c4, 4);
        assert_eq!(vga.read_port(0x3c5), Some(0x0e));
        vga.write_port(0x3c5, 0x06);
        vga.write_port(0x3c4, 2);
        vga.write_port(0x3c5, 0x0c);
        cpu.write_u8(ram, 0, 3);
        vga.write_port(0x3ce, 4);
        vga.write_port(0x3cf, 2);
        assert_eq!(cpu.read_u8(ram, 0), 3);

        vga.write_port(0x3c5, 0x0f);
        vga.write_port(0x3ce, 5);
        vga.write_port(0x3cf, 0x41);
        cpu.write_u8(ram, 1, 0xaa);
        vga.write_port(0x3d4, 0x0d);
        vga.write_port(0x3d5, 1);
        vga.refresh();
        assert_eq!(&vga.frame()[..8], &[1, 2, 3, 3, 0, 0, 0, 0]);
        vga.write_port(0x3d4, 0x12);
        vga.write_port(0x3d5, 0xdf);
        vga.refresh();
        assert_eq!(vga.height(), 240);
        assert_eq!(vga.frame().len(), WIDTH * 240);
        vga.leave(&mut cpu);
        assert_eq!(cpu.read_u8(ram, 1), 0);
    }
}
