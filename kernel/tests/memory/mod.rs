// SPDX-License-Identifier: MPL-2.0

mod frame;
mod frame_allocator;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use kernel::{
    hardware::mmu::{Mmu, MmuError, PAGE_SIZE, PageMapping, PagePermissions, PageTableFrames},
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
}

static MMU: PhysicalMmu = PhysicalMmu {
    memory: Mutex::new(BTreeMap::new()),
    zeros_until_failure: AtomicU64::new(NO_FAILURE),
    bits: 56,
};

fn isolated(name: &str) -> bool {
    if std::env::var("ARONA_MEMORY_TEST").as_deref() == Ok(name) {
        kernel::hardware::mmu::bind(&MMU).unwrap();
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env("ARONA_MEMORY_TEST", name)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{name} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
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
        if let Some(page) = memory.get(&address) {
            words.copy_from_slice(&page[offset..offset + words.len()]);
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

    fn is_valid_virtual_address(&self, _: usize) -> bool {
        false
    }
    fn initialize(&self, _: &mut dyn PageTableFrames) -> Result<(), MmuError> {
        Err(MmuError::Unsupported)
    }
    fn map(&self, _: &mut dyn PageTableFrames, _: usize, _: PageMapping) -> Result<(), MmuError> {
        Err(MmuError::Unsupported)
    }
    fn unmap(&self, _: &mut dyn PageTableFrames, _: usize) -> Result<PageMapping, MmuError> {
        Err(MmuError::Unsupported)
    }
    fn protect(
        &self,
        _: &mut dyn PageTableFrames,
        _: usize,
        _: PagePermissions,
    ) -> Result<(), MmuError> {
        Err(MmuError::Unsupported)
    }
    fn query(&self, _: &dyn PageTableFrames, _: usize) -> Result<Option<PageMapping>, MmuError> {
        Err(MmuError::Unsupported)
    }
    fn switch_root(&self, _: &dyn PageTableFrames) -> Result<(), MmuError> {
        Err(MmuError::Unsupported)
    }
    fn invalidate_page(&self, _: usize) -> Result<(), MmuError> {
        Err(MmuError::Unsupported)
    }
    fn invalidate_all(&self) {
        panic!("unexpected virtual memory operation");
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
