// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use kernel::{
    device::{
        DeviceError, DeviceSet,
        dma::{DmaDirection, DmaState},
    },
    hardware::mmu::PagePermissions,
    memory::{self, AddressSpace, FrameOwner, FramePurpose, MappingOptions, MemoryError},
};

pub(super) fn run(devices: &DeviceSet) {
    let mut cursor = 0;
    while let Some(id) = devices.next(cursor).unwrap() {
        cursor = id;
        let info = devices.describe(id).unwrap();
        let Some(group) = info.group else {
            continue;
        };
        let grant = devices.claim_group(group).unwrap();
        let reference = grant.reference();
        if info.dma.address_bits == 0 {
            assert!(matches!(
                reference.create_dma(id),
                Err(DeviceError::Unsupported)
            ));
            continue;
        }
        let domain = reference.create_dma(id).unwrap();
        assert_eq!(domain.state().unwrap(), DmaState::Quiesced);
        assert!(matches!(reference.create_dma(id), Err(DeviceError::Busy)));
        let allocation = memory::manager()
            .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
            .unwrap();
        let bus = domain
            .map(&allocation, 0..1, DmaDirection::Bidirectional)
            .unwrap();
        assert_eq!(allocation.info().dma, 1);
        assert_eq!(
            domain.map(&allocation, 0..1, DmaDirection::ToDevice),
            Err(DeviceError::Busy)
        );
        assert_eq!(allocation.info().dma, 1);
        wx(&domain, &allocation);
        domain.resume().unwrap();
        assert_eq!(domain.state().unwrap(), DmaState::Active);
        assert_eq!(domain.unmap(bus.start), Err(DeviceError::Busy));
        assert_eq!(domain.revoke(), Err(DeviceError::Busy));
        let additional = memory::manager()
            .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
            .unwrap();
        domain
            .map(&additional, 0..1, DmaDirection::ToDevice)
            .unwrap();
        drop(additional);
        domain.quiesce().unwrap();
        domain.unmap(bus.start).unwrap();
        assert_eq!(allocation.info().dma, 0);
        domain
            .map(&allocation, 0..1, DmaDirection::FromDevice)
            .unwrap();
        drop(allocation);
        domain.revoke().unwrap();
        assert_eq!(domain.state().unwrap(), DmaState::Revoked);
        assert_eq!(domain.resume(), Err(DeviceError::Closed));
        let next = reference.create_dma(id).unwrap();
        reference.revoke().unwrap();
        assert_eq!(next.state(), Err(DeviceError::Closed));
    }
}

fn wx(domain: &kernel::device::dma::DmaDomain, allocation: &kernel::memory::FrameAllocation) {
    let space = AddressSpace::new().unwrap();
    assert!(matches!(
        space.map(
            allocation,
            0..1,
            MappingOptions::new(PagePermissions::ReadExecute)
        ),
        Err(MemoryError::AccessConflict)
    ));
    let executable = memory::manager()
        .allocate(FrameOwner::User(1), FramePurpose::Data, 1)
        .unwrap();
    let range = space
        .map(
            &executable,
            0..1,
            MappingOptions::new(PagePermissions::ReadExecute),
        )
        .unwrap();
    assert_eq!(
        domain.map(&executable, 0..1, DmaDirection::FromDevice),
        Err(DeviceError::Memory(MemoryError::AccessConflict))
    );
    assert_eq!(executable.info().dma, 0);
    space.unmap(range).unwrap();
}

pub fn run_failure(devices: &DeviceSet, revoke: bool) -> ! {
    let mut cursor = 0;
    while let Some(id) = devices.next(cursor).unwrap() {
        cursor = id;
        let info = devices.describe(id).unwrap();
        let Some(group) = info.group.filter(|_| info.dma.address_bits != 0) else {
            continue;
        };
        let grant = devices.claim_group(group).unwrap();
        let domain = grant.reference().create_dma(id).unwrap();
        let allocation = memory::manager()
            .allocate(FrameOwner::Kernel, FramePurpose::Data, 1)
            .unwrap();
        domain
            .map(&allocation, 0..1, DmaDirection::Bidirectional)
            .unwrap();
        assert_eq!(allocation.info().dma, 1);
        drop(allocation);
        domain.resume().unwrap();
        if revoke {
            grant.reference().revoke().unwrap();
        } else {
            drop(domain);
        }
        panic!("active DMA teardown must halt");
    }
    panic!("DMA failure test device is missing");
}
