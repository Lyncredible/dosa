use std::ops::Range;

use dosa_cpu::Cpu;

pub(super) const SEGMENT: u16 = 0xf000;
pub(super) const ENTRY: u16 = 0x4300;
const RETURN: u16 = ENTRY + 5;
const HMA_BYTES: usize = 0xfff0;
const HMA_END: usize = 0x100000 + HMA_BYTES;
const POOL_START: usize = 0x110000;
const KIB: usize = 1024;
const HANDLES: u16 = 64;

const OUT_OF_MEMORY: u8 = 0xa0;
const INVALID_HANDLE: u8 = 0xa2;
const BLOCK_LOCKED: u8 = 0xab;

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
struct Block {
    base: usize,
    bytes: usize,
    locks: u8,
}

pub(super) struct Xms {
    blocks: [Option<Block>; HANDLES as usize],
    hma: bool,
    global_a20: bool,
    local_a20: u16,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct XmsSnapshot {
    blocks: Vec<Option<Block>>,
    hma: bool,
    global_a20: bool,
    local_a20: u16,
}

impl Default for Xms {
    fn default() -> Self {
        Self {
            blocks: [None; HANDLES as usize],
            hma: false,
            global_a20: false,
            local_a20: 0,
        }
    }
}

impl Xms {
    pub(super) fn snapshot(&self) -> XmsSnapshot {
        XmsSnapshot {
            blocks: self.blocks.to_vec(),
            hma: self.hma,
            global_a20: self.global_a20,
            local_a20: self.local_a20,
        }
    }

    pub(super) fn from_snapshot(state: XmsSnapshot, pool_end: usize) -> Option<Self> {
        let blocks: [Option<Block>; HANDLES as usize] = state.blocks.try_into().ok()?;
        let mut allocated: Vec<_> = blocks.iter().flatten().copied().collect();
        allocated.sort_unstable_by_key(|block| block.base);
        let mut next = POOL_START;
        for block in allocated {
            if block.base < POOL_START
                || !block.base.is_multiple_of(KIB)
                || !block.bytes.is_multiple_of(KIB)
                || block.base.checked_add(block.bytes)? > pool_end
            {
                return None;
            }
            if block.bytes != 0 {
                if block.base < next {
                    return None;
                }
                next = block.base + block.bytes;
            }
        }
        Some(Self {
            blocks,
            hma: state.hma,
            global_a20: state.global_a20,
            local_a20: state.local_a20,
        })
    }

    pub(super) fn install(cpu: &mut Cpu) {
        // The driver ABI reserves five patchable bytes at the far entry point.
        cpu.load(SEGMENT, ENTRY, &[0xeb, 3, 0x90, 0x90, 0x90, 0xcb]);
    }

    pub(super) fn at_entry(cpu: &Cpu) -> bool {
        cpu.regs.cs == SEGMENT && cpu.regs.ip == RETURN
    }

    pub(super) fn a20_enabled(&self) -> bool {
        self.global_a20 || self.local_a20 != 0
    }

    pub(super) fn update_a20(&mut self, function: u8) -> Result<(), u8> {
        match function {
            3 => self.global_a20 = true,
            4 => self.global_a20 = false,
            5 => self.local_a20 = self.local_a20.checked_add(1).ok_or(0x82u8)?,
            6 => self.local_a20 = self.local_a20.checked_sub(1).ok_or(0x82u8)?,
            _ => unreachable!("caller supplies an XMS A20 control function"),
        }
        Ok(())
    }

    pub(super) fn complete(cpu: &mut Cpu, result: Result<(), u8>) {
        cpu.regs.ax = u16::from(result.is_ok());
        if let Err(error) = result {
            cpu.regs.bx = cpu.regs.bx & 0xff00 | u16::from(error);
        }
    }

    pub(super) fn call(&mut self, cpu: &mut Cpu) {
        let function = (cpu.regs.ax >> 8) as u8;
        let handle = cpu.regs.dx;
        let pool_end = cpu.memory.len();
        let result = match function {
            0 => {
                cpu.regs.ax = 0x0200;
                cpu.regs.bx = 0x0200;
                cpu.regs.dx = 1;
                return;
            }
            1 if self.hma => Err(0x91),
            1 => {
                self.hma = true;
                Ok(())
            }
            2 if !self.hma => Err(0x93),
            2 => {
                self.hma = false;
                Ok(())
            }
            7 => {
                cpu.regs.ax = u16::from(cpu.a20_enabled());
                cpu.regs.bx &= 0xff00;
                return;
            }
            8 => {
                let (largest, total) = self.free_space(pool_end);
                cpu.regs.ax = (largest / KIB) as u16;
                cpu.regs.dx = (total / KIB) as u16;
                cpu.regs.bx &= 0xff00;
                if total == 0 {
                    cpu.regs.bx |= u16::from(OUT_OF_MEMORY);
                }
                return;
            }
            9 => match self.allocate(usize::from(handle) * KIB, pool_end) {
                Ok(handle) => {
                    cpu.regs.dx = handle;
                    Ok(())
                }
                Err(error) => {
                    cpu.regs.dx = 0;
                    Err(error)
                }
            },
            0x0a => self.free(handle),
            0x0b => self.move_block(cpu),
            0x0c => self.lock(handle).map(|address| {
                cpu.regs.dx = (address >> 16) as u16;
                cpu.regs.bx = address as u16;
            }),
            0x0d => self.unlock(handle),
            0x0e => self.block(handle).map(|block| {
                cpu.regs.bx = u16::from_le_bytes([self.free_handles(), block.locks]);
                cpu.regs.dx = (block.bytes / KIB) as u16;
            }),
            0x0f => self.resize(handle, usize::from(cpu.regs.bx) * KIB, cpu),
            0x10 => {
                cpu.regs.dx = 0;
                Err(0xb1)
            }
            0x11 => Err(0xb2),
            _ => Err(0x80),
        };
        Self::complete(cpu, result);
    }

    fn free_handles(&self) -> u8 {
        self.blocks.iter().filter(|block| block.is_none()).count() as u8
    }

    fn slot(&mut self, handle: u16) -> Result<&mut Option<Block>, u8> {
        let index = usize::from(handle.checked_sub(1).ok_or(INVALID_HANDLE)?);
        self.blocks.get_mut(index).ok_or(INVALID_HANDLE)
    }

    fn block(&self, handle: u16) -> Result<Block, u8> {
        let index = usize::from(handle.checked_sub(1).ok_or(INVALID_HANDLE)?);
        self.blocks
            .get(index)
            .copied()
            .flatten()
            .ok_or(INVALID_HANDLE)
    }

    fn gaps(&self, end: usize, ignored: Option<u16>) -> impl Iterator<Item = Range<usize>> {
        let mut occupied: Vec<_> = self
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(index, block)| {
                let block = (*block)?;
                (block.bytes != 0 && Some(index as u16 + 1) != ignored).then_some(block)
            })
            .collect();
        occupied.sort_unstable_by_key(|block| block.base);
        let mut next = POOL_START;
        occupied
            .into_iter()
            .map(|block| (block.base, block.bytes))
            .chain(std::iter::once((end, 0)))
            .map(move |(base, bytes)| {
                let gap = next..base;
                next = base + bytes;
                gap
            })
    }

    fn free_space(&self, end: usize) -> (usize, usize) {
        self.gaps(end, None).fold((0, 0), |(largest, total), gap| {
            (largest.max(gap.len()), total + gap.len())
        })
    }

    fn first_fit(&self, bytes: usize, end: usize, ignored: Option<u16>) -> Result<usize, u8> {
        self.gaps(end, ignored)
            .find(|gap| gap.len() >= bytes)
            .map(|gap| gap.start)
            .ok_or(OUT_OF_MEMORY)
    }

    fn allocate(&mut self, bytes: usize, end: usize) -> Result<u16, u8> {
        let index = self.blocks.iter().position(Option::is_none).ok_or(0xa1u8)?;
        let base = self.first_fit(bytes, end, None)?;
        self.blocks[index] = Some(Block {
            base,
            bytes,
            locks: 0,
        });
        Ok(index as u16 + 1)
    }

    fn free(&mut self, handle: u16) -> Result<(), u8> {
        let block = self.block(handle)?;
        if block.locks != 0 {
            return Err(BLOCK_LOCKED);
        }
        *self.slot(handle)? = None;
        Ok(())
    }

    fn lock(&mut self, handle: u16) -> Result<usize, u8> {
        let block = self.slot(handle)?.as_mut().ok_or(INVALID_HANDLE)?;
        block.locks = block.locks.checked_add(1).ok_or(0xacu8)?;
        Ok(block.base)
    }

    fn unlock(&mut self, handle: u16) -> Result<(), u8> {
        let block = self.slot(handle)?.as_mut().ok_or(INVALID_HANDLE)?;
        block.locks = block.locks.checked_sub(1).ok_or(0xaau8)?;
        Ok(())
    }

    fn resize(&mut self, handle: u16, bytes: usize, cpu: &mut Cpu) -> Result<(), u8> {
        let old = self.block(handle)?;
        if old.locks != 0 {
            return Err(BLOCK_LOCKED);
        }
        let mut base = old.base;
        if bytes > old.bytes {
            base = self.first_fit(bytes, cpu.memory.len(), Some(handle))?;
            cpu.memory.copy_within(old.base..old.base + old.bytes, base);
        }
        *self.slot(handle)? = Some(Block {
            base,
            bytes,
            locks: 0,
        });
        Ok(())
    }

    fn move_block(&self, cpu: &mut Cpu) -> Result<(), u8> {
        let word = |offset: u16| cpu.read_u16(cpu.regs.ds, cpu.regs.si.wrapping_add(offset));
        let dword = |offset| u32::from(word(offset)) | u32::from(word(offset + 2)) << 16;
        let length = dword(0) as usize;
        if length & 1 != 0 {
            return Err(0xa7);
        }
        let source = self.endpoint(word(4), dword(6), length, 0xa3, 0xa4)?;
        let dest = self.endpoint(word(10), dword(12), length, 0xa5, 0xa6)?;
        if cpu.copy_physical_range(source..source + length, dest) {
            Ok(())
        } else {
            Err(0xa7)
        }
    }

    fn endpoint(
        &self,
        handle: u16,
        offset: u32,
        length: usize,
        handle_error: u8,
        offset_error: u8,
    ) -> Result<usize, u8> {
        let (base, size, offset) = if handle == 0 {
            let address = ((offset >> 16) << 4) + (offset & 0xffff);
            (0, HMA_END, address as usize)
        } else {
            let block = self.block(handle).map_err(|_| handle_error)?;
            (block.base, block.bytes, offset as usize)
        };
        if offset > size || offset == size && length != 0 {
            return Err(offset_error);
        }
        if length > size - offset {
            return Err(0xa7);
        }
        Ok(base + offset)
    }
}

#[cfg(test)]
#[test]
fn resize_relocates_an_unlocked_block_and_keeps_live_handles_unchanged() {
    let mut xms = Xms::default();
    let mut cpu = Cpu::new();
    let last = cpu.memory.len();
    let first = xms.allocate(2 * KIB, last).unwrap();
    let pinned = xms.allocate(2 * KIB, last).unwrap();
    let old = xms.block(first).unwrap().base;
    cpu.memory[old..old + 4].copy_from_slice(b"data");
    let pinned_address = xms.lock(pinned).unwrap();
    assert_eq!(xms.resize(pinned, 3 * KIB, &mut cpu), Err(BLOCK_LOCKED));
    assert_eq!(xms.resize(first, last, &mut cpu), Err(OUT_OF_MEMORY));
    assert_eq!(xms.block(first).unwrap().base, old);
    xms.resize(first, 3 * KIB, &mut cpu).unwrap();
    let new = xms.block(first).unwrap().base;
    assert!(new >= pinned_address + 2 * KIB);
    assert_eq!(&cpu.memory[new..new + 4], b"data");
    assert_eq!(xms.block(pinned).unwrap().base, pinned_address);
}

#[cfg(test)]
#[test]
fn xms_snapshot_retains_handle_numbers_and_rejects_overlapping_blocks() {
    let end = Cpu::new().memory.len();
    let mut xms = Xms::default();
    let first = xms.allocate(KIB, end).unwrap();
    let second = xms.allocate(3 * KIB, end).unwrap();
    let address = xms.lock(second).unwrap();
    xms.update_a20(5).unwrap();

    let bytes = postcard::to_allocvec(&xms.snapshot()).unwrap();
    let mut restored = Xms::from_snapshot(postcard::from_bytes(&bytes).unwrap(), end).unwrap();
    assert_eq!(restored.block(second).unwrap().base, address);
    assert_eq!(restored.free(second), Err(BLOCK_LOCKED));
    assert!(restored.a20_enabled());
    assert_eq!(restored.free(first), Ok(()));
    assert_eq!(restored.allocate(KIB, end), Ok(first));

    let mut invalid = xms.snapshot();
    invalid.blocks[1].as_mut().unwrap().base = POOL_START;
    assert!(Xms::from_snapshot(invalid, end).is_none());
}
