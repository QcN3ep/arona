// SPDX-License-Identifier: MPL-2.0

use super::*;
use kernel::memory::{AddressSpace, MappingOptions, MappingRights, VirtRange};

fn mutable_code() -> MappingOptions {
    MappingOptions {
        permissions: PagePermissions::ReadWrite,
        maximum: MappingRights::READ | MappingRights::WRITE | MappingRights::EXECUTE,
        alignment: PAGE_SIZE,
    }
}

fn alias(frame: Frame) -> usize {
    memory::manager().direct_map_range().unwrap().start + (frame.address() - BASE) as usize
}

fn permissions(frame: Frame) -> PagePermissions {
    memory::manager()
        .kernel_address_space()
        .query(alias(frame))
        .unwrap()
        .unwrap()
        .mapping
        .unwrap()
        .attributes
        .permissions
}

#[test]
fn ram_access_shares_kernel_mappings_and_preserves_frame_lifetimes() {
    if isolated("direct_map::ram_access_shares_kernel_mappings_and_preserves_frame_lifetimes") {
        return;
    }
    let mut regions = memory(16);
    let end = regions[0].range.end;
    regions.extend([
        region(end, 2, MemoryRegionState::Reserved),
        region(end + 2 * PAGE, 2, MemoryRegionState::BootReserved),
        region(end + 4 * PAGE, 2, MemoryRegionState::NoMap),
        region(end + 8 * PAGE, 1, MemoryRegionState::Reserved),
    ]);
    let manager = initialize(&regions);
    manager.initialize_kernel_address_space().unwrap();
    let kernel = manager.kernel_address_space();
    let user = AddressSpace::new().unwrap();
    let image = manager
        .allocate(FrameOwner::Kernel, FramePurpose::Data, 1)
        .unwrap();
    let bits = 12 + 9 * MMU.layout().levels;
    let image_range = kernel
        .map_at(
            usize::MAX << (bits - 1),
            &image,
            0..1,
            MappingOptions::new(PagePermissions::ReadExecute),
        )
        .unwrap();
    let early_range = PhysRange {
        start: end + 2 * PAGE,
        end: end + 3 * PAGE,
    };
    manager
        .reclaim_reserved(early_range, FramePool::Kernel)
        .unwrap();
    let early = manager
        .allocate(FrameOwner::Kernel, FramePurpose::Data, 1)
        .unwrap();
    let early_frame = early.frames().next().unwrap();
    assert_eq!(early_frame.address(), early_range.start);
    let early_write = early
        .access(early_frame, ReferenceKind::Temporary, FrameAccess::Write)
        .unwrap();
    let before = manager.stats();
    manager.initialize_direct_map().unwrap();
    assert_eq!(
        manager.initialize_direct_map(),
        Err(MemoryError::AlreadyInitialized)
    );
    assert!(before.kernel.free_frames - manager.stats().kernel.free_frames < 64);
    let aperture = manager.direct_map_range().unwrap();
    assert!(!aperture.overlaps(image_range));
    assert_eq!(kernel.unreserve(aperture), Err(MemoryError::InUse));
    assert_eq!(kernel.reserve_at(aperture), Err(MemoryError::AlreadyMapped));
    assert_eq!(
        permissions(image.frames().next().unwrap()),
        PagePermissions::ReadOnly
    );
    for (physical, expected) in [
        (end, Some(PagePermissions::ReadOnly)),
        (end + 2 * PAGE, Some(PagePermissions::ReadWrite)),
        (end + 3 * PAGE, Some(PagePermissions::ReadOnly)),
        (end + 4 * PAGE, None),
        (end + 6 * PAGE, None),
        (end + 8 * PAGE, Some(PagePermissions::ReadOnly)),
    ] {
        let address = aperture.start + (physical - BASE) as usize;
        let mapping = MMU.lookup(user.root(), address);
        assert_eq!(
            mapping.map(|mapping| mapping.attributes.permissions),
            expected
        );
        assert_eq!(
            kernel.translate(address).unwrap(),
            expected.map(|_| physical)
        );
        assert_eq!(user.translate(address), Err(MemoryError::InvalidRequest));
        if let Some(mapping) = mapping {
            assert!(!mapping.attributes.user_accessible);
        }
    }
    let free = manager.stats().user.free_frames;
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 2)
        .unwrap();
    let frame = allocation.frames().next().unwrap();
    let write = allocation
        .access(frame, ReferenceKind::Temporary, FrameAccess::Write)
        .unwrap();
    assert_eq!(write.virtual_address(), None);
    kernel.activate().unwrap();
    assert_eq!(write.virtual_address(), Some(alias(frame)));
    user.activate().unwrap();
    early_write.write(0, &[17]).unwrap();
    drop(early_write);
    drop(early);
    write.write(PAGE_SIZE / 8 - 1, &[0xfeed_face]).unwrap();
    let mut word = [0];
    write.read(PAGE_SIZE / 8 - 1, &mut word).unwrap();
    assert_eq!(word, [0xfeed_face]);
    assert_eq!(
        write.write(PAGE_SIZE / 8, &[1]),
        Err(MemoryError::Mmu(MmuError::InvalidRange))
    );
    assert!(MMU.mapped_accesses.load(Ordering::Relaxed) >= 2);
    drop(write);
    for address in [0x10000, 0x11000] {
        user.map_at(address, &allocation, 0..1, mutable_code())
            .unwrap();
    }
    let mapped = VirtRange {
        start: 0x10000,
        end: 0x12000,
    };
    user.protect(mapped, PagePermissions::ReadExecute).unwrap();
    assert_eq!(permissions(frame), PagePermissions::ReadOnly);
    assert!(matches!(
        allocation.access(frame, ReferenceKind::Dma, FrameAccess::Write),
        Err(MemoryError::AccessConflict)
    ));
    user.unmap(mapped).unwrap();
    assert_eq!(permissions(frame), PagePermissions::ReadOnly);
    let write = allocation
        .access(frame, ReferenceKind::Temporary, FrameAccess::Write)
        .unwrap();
    assert_eq!(permissions(frame), PagePermissions::ReadWrite);
    drop(allocation);
    assert_eq!(manager.stats().user.free_frames, free - 2);
    write.write(0, &[73]).unwrap();
    write.read(0, &mut word).unwrap();
    assert_eq!(word, [73]);
    drop(write);
    assert_eq!(manager.stats().user.free_frames, free);
    let reserved = PhysRange {
        start: end + 3 * PAGE,
        end: end + 4 * PAGE,
    };
    manager.reclaim_reserved(reserved, FramePool::User).unwrap();
    let reclaimed = manager
        .allocate(FrameOwner::User(2), FramePurpose::Data, 1)
        .unwrap();
    for frame in reclaimed.frames() {
        assert!(reserved.contains(PhysRange {
            start: frame.address(),
            end: frame.address() + PAGE
        }));
        let write = reclaimed
            .access(frame, ReferenceKind::Temporary, FrameAccess::Write)
            .unwrap();
        write.write(0, &[42]).unwrap();
        assert_eq!(permissions(frame), PagePermissions::ReadWrite);
    }
    drop(reclaimed);
    kernel.activate().unwrap();
    drop(user);
    kernel.unmap(image_range).unwrap();
}

#[test]
fn failed_mapping_transactions_restore_aperture_permissions_and_resources() {
    if isolated(
        "direct_map::failed_mapping_transactions_restore_aperture_permissions_and_resources",
    ) {
        return;
    }
    let manager = initialize(&memory(4));
    manager.initialize_kernel_address_space().unwrap();
    let kernel = manager.kernel_address_space();
    let initial = manager.stats();
    for writes in [0, 1, 3, 513, 1027] {
        MMU.writes_until_failure.store(writes, Ordering::Relaxed);
        assert_eq!(
            manager.initialize_direct_map(),
            Err(MemoryError::Mmu(MmuError::InvalidTable))
        );
        assert_eq!(manager.direct_map_range(), None);
        assert_eq!(manager.stats(), initial);
    }
    manager.initialize_direct_map().unwrap();
    kernel.activate().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 2)
        .unwrap();
    let frames: Vec<_> = allocation.frames().collect();
    let range = kernel.map(&allocation, 0..2, mutable_code()).unwrap();
    let original = kernel.query(range.start).unwrap();
    let stats = manager.stats();
    for writes in 0..6 {
        MMU.writes_until_failure.store(writes, Ordering::Relaxed);
        assert_eq!(
            kernel.protect(range, PagePermissions::ReadExecute),
            Err(MemoryError::Mmu(MmuError::InvalidTable))
        );
        assert_eq!(kernel.query(range.start).unwrap(), original);
        assert_eq!(manager.stats(), stats);
        for frame in &frames {
            assert_eq!(permissions(*frame), PagePermissions::ReadWrite);
        }
    }
    MMU.fail_synchronization.store(true, Ordering::Relaxed);
    assert_eq!(
        kernel.protect(range, PagePermissions::ReadExecute),
        Err(MemoryError::Mmu(MmuError::InvalidTable))
    );
    for frame in &frames {
        assert_eq!(permissions(*frame), PagePermissions::ReadWrite);
    }
    kernel.protect(range, PagePermissions::ReadExecute).unwrap();
    let executable = kernel.query(range.start).unwrap();
    for writes in 0..6 {
        MMU.writes_until_failure.store(writes, Ordering::Relaxed);
        assert_eq!(
            kernel.protect(range, PagePermissions::ReadWrite),
            Err(MemoryError::Mmu(MmuError::InvalidTable))
        );
        assert_eq!(kernel.query(range.start).unwrap(), executable);
        for frame in &frames {
            assert_eq!(permissions(*frame), PagePermissions::ReadOnly);
        }
    }
    kernel.protect(range, PagePermissions::ReadWrite).unwrap();
    kernel.unmap(range).unwrap();
    let user = AddressSpace::new().unwrap();
    let stats = manager.stats();
    for writes in 0..(MMU.layout().levels as u64 + 3) {
        MMU.writes_until_failure.store(writes, Ordering::Relaxed);
        let mut options = mutable_code();
        options.permissions = PagePermissions::ReadExecute;
        assert_eq!(
            user.map_at(0x10000, &allocation, 0..2, options),
            Err(MemoryError::Mmu(MmuError::InvalidTable))
        );
        assert_eq!(user.query(0x10000).unwrap(), None);
        assert_eq!(allocation.info().mappings, 0);
        assert_eq!(manager.stats(), stats);
        for frame in &frames {
            assert_eq!(permissions(*frame), PagePermissions::ReadWrite);
        }
    }
    let mapped = user.map(&allocation, 0..2, mutable_code()).unwrap();
    user.protect(mapped, PagePermissions::ReadExecute).unwrap();
    user.unmap(mapped).unwrap();
    drop(allocation);
    let reused = manager
        .allocate(FrameOwner::User(2), FramePurpose::Data, 2)
        .unwrap();
    for frame in reused.frames() {
        let write = reused
            .access(frame, ReferenceKind::Temporary, FrameAccess::Write)
            .unwrap();
        let mut word = [u64::MAX];
        write.read(0, &mut word).unwrap();
        assert_eq!(word, [0]);
        write.write(0, &[1]).unwrap();
    }
}
