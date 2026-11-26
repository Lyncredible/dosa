use super::Cpu;

pub(super) const PG: u32 = 1 << 31;
const PAGE_MASK: u32 = !0xfff;
const PRESENT: u32 = 1;
const ACCESSED: u32 = 1 << 5;
const DIRTY: u32 = 1 << 6;

#[derive(Clone, Copy)]
pub(super) struct PageFault {
    pub(super) linear: u32,
    pub(super) write: bool,
}

#[derive(Clone, Copy)]
pub(super) struct Translation {
    directory: usize,
    table: usize,
    directory_entry: u32,
    table_entry: u32,
}

impl Translation {
    pub(super) fn address(self, linear: u32) -> usize {
        ((self.table_entry & PAGE_MASK) | (linear & !PAGE_MASK)) as usize
    }
}

impl Cpu {
    pub(super) fn paging(&self) -> bool {
        self.control.cr0 & PG != 0
    }

    pub(super) fn walk_page(&self, linear: u32) -> Option<Translation> {
        let directory = (self.control.cr3 & PAGE_MASK) | ((linear >> 20) & 0xffc);
        let directory_entry = self.read_mem(directory as usize, 4);
        if directory_entry & PRESENT == 0 {
            return None;
        }
        let table = (directory_entry & PAGE_MASK) | ((linear >> 10) & 0xffc);
        let table_entry = self.read_mem(table as usize, 4);
        if table_entry & PRESENT == 0 {
            return None;
        }
        // Ring 0 on the 386 may write read-only pages. Supervisor write
        // protection and user privilege checks belong to later CPU work.
        Some(Translation {
            directory: directory as usize,
            table: table as usize,
            directory_entry,
            table_entry,
        })
    }

    pub(super) fn touch_page(&mut self, page: Translation, write: bool) {
        let directory = page.directory_entry | ACCESSED;
        let table = page.table_entry | ACCESSED | if write { DIRTY } else { 0 };
        if directory != page.directory_entry {
            self.write_mem(page.directory, 4, directory);
        }
        if table != page.table_entry {
            self.write_mem(page.table, 4, table);
        }
    }

    pub(super) fn paged_span(
        &mut self,
        linear: usize,
        width: u8,
        write: bool,
        touch: bool,
    ) -> Option<[usize; 8]> {
        let linear = linear as u32;
        let last = linear.wrapping_add(u32::from(width) - 1);
        let Some(first_page) = self.walk_page(linear) else {
            self.page_fault = Some(PageFault { linear, write });
            return None;
        };
        let crosses = (linear ^ last) & PAGE_MASK != 0;
        let last_page = if crosses {
            let Some(page) = self.walk_page(last) else {
                self.page_fault = Some(PageFault {
                    linear: last & PAGE_MASK,
                    write,
                });
                return None;
            };
            page
        } else {
            first_page
        };
        if touch {
            self.touch_page(first_page, write);
            if crosses {
                self.touch_page(last_page, write);
            }
        }
        let mut addresses = [0; 8];
        for (index, physical) in addresses[..usize::from(width)].iter_mut().enumerate() {
            let current = linear.wrapping_add(index as u32);
            let page = if (linear ^ current) & PAGE_MASK == 0 {
                first_page
            } else {
                last_page
            };
            *physical = page.address(current);
        }
        Some(addresses)
    }
}
