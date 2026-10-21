// SPDX-License-Identifier: MPL-2.0

mod frame;
mod frame_allocator;
mod paging;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use kernel::{
    hardware::mmu::{
        Mmu, MmuError, PAGE_SIZE, PageAttributes, PageMapping, PagePermissions, PageTableEntry,
        PageTableLayout, TableWindow,
    },
    memory::{
        self, Frame, FrameOwner, FramePool, FramePurpose, MAX_MEMORY_REGIONS, MemoryError,
        MemoryManager, MemoryRegion, MemoryRegionState, PhysRange, ReferenceKind,
    },
};

const PAGE: u64 = PAGE_SIZE as u64;
const KERNEL_FRAMES: u64 = 32 * 1024 * 1024 / PAGE;
const BASE: u64 = 0x100_0000;
const NO_FAILURE: u64 = u64::MAX;

type FrameBytes = Box<[u64; PAGE_SIZE / size_of::<u64>()]>;

struct PhysicalMmu {
    memory: Mutex<BTreeMap<u64, FrameBytes>>,
    zeros_until_failure: AtomicU64,
    bits: u32,
    writes_until_failure: AtomicU64,
    invalidations: AtomicU64,
    levels: AtomicUsize,
}

static MMU: PhysicalMmu = PhysicalMmu {
    memory: Mutex::new(BTreeMap::new()),
    zeros_until_failure: AtomicU64::new(NO_FAILURE),
    bits: 56,
    writes_until_failure: AtomicU64::new(NO_FAILURE),
    invalidations: AtomicU64::new(0),
    levels: AtomicUsize::new(3),
};

fn isolated(name: &str) -> bool {
    if std::env::var("ARONA_MEMORY_TEST").as_deref() == Ok(name) {
        let levels = std::env::var("ARONA_TABLE_LEVELS").unwrap_or_else(|_| "3".into());
        MMU.levels.store(levels.parse().unwrap(), Ordering::Relaxed);
        kernel::hardware::mmu::bind(&MMU).unwrap();
        return false;
    }
    let levels = if name.starts_with("paging::") {
        &[3, 4][..]
    } else {
        &[3][..]
    };
    for levels in levels {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env("ARONA_MEMORY_TEST", name)
            .env("ARONA_TABLE_LEVELS", levels.to_string())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{name} with {levels} levels failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    true
}

fn initialize(regions: &[MemoryRegion]) -> &'static MemoryManager {
    memory::initialize(regions).unwrap();
    memory::manager()
}

impl PhysicalMmu {
    fn validate(&self, address: u64, offset: usize, length: usize) -> Result<(), MmuError> {
        if address >= 1 << self.bits {
            return Err(MmuError::InvalidAddress);
        }
        if !address.is_multiple_of(PAGE) {
            return Err(MmuError::InvalidAlignment);
        }
        if offset > PAGE_SIZE / 8 || length > PAGE_SIZE / 8 - offset {
            return Err(MmuError::InvalidRange);
        }
        Ok(())
    }
}

impl Mmu for PhysicalMmu {
    fn physical_address_bits(&self) -> u32 {
        self.bits
    }

    fn read_frame(&self, address: u64, offset: usize, words: &mut [u64]) -> Result<(), MmuError> {
        self.validate(address, offset, words.len())?;
        let memory = self.memory.lock().unwrap();
        if let Some(frame) = memory.get(&address) {
            words.copy_from_slice(&frame[offset..offset + words.len()]);
        } else {
            words.fill(u64::MAX);
        }
        Ok(())
    }

    fn write_frame(&self, address: u64, offset: usize, words: &[u64]) -> Result<(), MmuError> {
        self.validate(address, offset, words.len())?;
        self.memory
            .lock()
            .unwrap()
            .entry(address)
            .or_insert_with(|| Box::new([u64::MAX; PAGE_SIZE / 8]))[offset..offset + words.len()]
            .copy_from_slice(words);
        Ok(())
    }

    fn zero_frame(&self, address: u64) -> Result<(), MmuError> {
        self.validate(address, 0, PAGE_SIZE / 8)?;
        let remaining = self.zeros_until_failure.load(Ordering::Relaxed);
        if remaining != NO_FAILURE {
            if remaining == 0 {
                self.zeros_until_failure
                    .store(NO_FAILURE, Ordering::Relaxed);
                return Err(MmuError::InvalidTable);
            }
            self.zeros_until_failure
                .store(remaining - 1, Ordering::Relaxed);
        }
        self.memory
            .lock()
            .unwrap()
            .insert(address, Box::new([0; PAGE_SIZE / 8]));
        Ok(())
    }

    fn layout(&self) -> PageTableLayout {
        PageTableLayout {
            levels: self.levels.load(Ordering::Relaxed),
            index_bits: 9,
            window: TableWindow {
                virtual_address: usize::MAX & !(PAGE_SIZE - 1),
                table: Frame::new(0x1000).unwrap(),
                level: 1,
            },
        }
    }

    fn is_valid_virtual_address(&self, address: usize) -> bool {
        (((address as i64) << 25) >> 25) as usize == address
    }

    fn read_entry(
        &self,
        table: Frame,
        index: usize,
        level: usize,
    ) -> Result<PageTableEntry, MmuError> {
        if level >= self.layout().levels || index >= PAGE_SIZE / 8 {
            return Err(MmuError::InvalidRange);
        }
        let mut words = [0];
        self.read_frame(table.address(), index, &mut words)?;
        let word = words[0];
        if word == 0 {
            return Ok(PageTableEntry::Empty);
        }
        let frame = Frame::new(word & !(PAGE - 1)).unwrap();
        if level > 0 {
            return Ok(PageTableEntry::Table(frame));
        }
        Ok(PageTableEntry::Mapping(PageMapping {
            physical_address: frame.address(),
            attributes: PageAttributes {
                permissions: match (word >> 1) & 3 {
                    0 => PagePermissions::ReadOnly,
                    1 => PagePermissions::ReadWrite,
                    2 => PagePermissions::ReadExecute,
                    _ => PagePermissions::ExecuteOnly,
                },
                memory_type: kernel::hardware::mmu::MemoryType::Normal,
                user_accessible: word & 16 != 0,
            },
        }))
    }

    fn write_entry(
        &self,
        table: Frame,
        index: usize,
        level: usize,
        entry: PageTableEntry,
    ) -> Result<(), MmuError> {
        if level >= self.layout().levels || index >= PAGE_SIZE / 8 {
            return Err(MmuError::InvalidRange);
        }
        let remaining = self.writes_until_failure.load(Ordering::Relaxed);
        if remaining != NO_FAILURE {
            if remaining == 0 {
                self.writes_until_failure
                    .store(NO_FAILURE, Ordering::Relaxed);
                return Err(MmuError::InvalidTable);
            }
            self.writes_until_failure
                .store(remaining - 1, Ordering::Relaxed);
        }
        let word = match entry {
            PageTableEntry::Empty => 0,
            PageTableEntry::Table(frame) if level > 0 => frame.address() | 1,
            PageTableEntry::Mapping(mapping) if level == 0 => {
                self.validate(mapping.physical_address, 0, 1)?;
                let permissions = match mapping.attributes.permissions {
                    PagePermissions::ReadOnly => 0,
                    PagePermissions::ReadWrite => 1,
                    PagePermissions::ReadExecute => 2,
                    PagePermissions::ExecuteOnly => 3,
                };
                mapping.physical_address
                    | 1
                    | (permissions << 1)
                    | if mapping.attributes.user_accessible {
                        16
                    } else {
                        0
                    }
            }
            _ => return Err(MmuError::InvalidTable),
        };
        self.write_frame(table.address(), index, &[word])
    }

    fn switch_root(&self, _: Frame) -> Result<(), MmuError> {
        Err(MmuError::Unsupported)
    }
    fn invalidate_page(&self, _: usize) -> Result<(), MmuError> {
        self.invalidate_all();
        Ok(())
    }
    fn invalidate_all(&self) {
        self.invalidations.fetch_add(1, Ordering::Relaxed);
    }
    fn synchronize_code(&self, _: std::ops::Range<usize>) -> Result<(), MmuError> {
        Err(MmuError::Unsupported)
    }
}

fn region(start: u64, frames: u64, state: MemoryRegionState) -> MemoryRegion {
    MemoryRegion {
        range: PhysRange {
            start,
            end: start + frames * PAGE,
        },
        state,
    }
}

fn memory(user_frames: u64) -> Vec<MemoryRegion> {
    vec![region(
        BASE,
        KERNEL_FRAMES + user_frames,
        MemoryRegionState::Available,
    )]
}
