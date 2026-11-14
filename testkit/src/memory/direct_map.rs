// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use kernel::memory::{FrameAccess, FrameOwner, FramePurpose, ReferenceKind};

use super::*;

pub(super) fn run(regions: &[MemoryRegion]) {
    let manager = memory::manager();
    let initial = manager.stats();
    let kernel = manager.kernel_address_space();
    let aperture = manager.direct_map_range().unwrap();
    let allocation = manager
        .allocate(FrameOwner::User(1), FramePurpose::Data, 2)
        .unwrap();
    let frame = first_frame(&allocation);
    let writer = allocation
        .access(frame, ReferenceKind::Temporary, FrameAccess::Write)
        .unwrap();
    assert_eq!(writer.virtual_address(), None);
    kernel.activate().unwrap();
    let address = aperture.start + (frame.address() - regions[0].range.start) as usize;
    assert_eq!(writer.virtual_address(), Some(address));
    let user = AddressSpace::new().unwrap();
    user.activate().unwrap();
    writer.write(PAGE_SIZE / 8 - 1, &[73]).unwrap();
    let mut word = [0];
    writer.read(PAGE_SIZE / 8 - 1, &mut word).unwrap();
    assert_eq!(word, [73]);
    assert_eq!(
        lookup(&user, address).unwrap().physical_address,
        frame.address()
    );
    assert_eq!(user.translate(address), Err(MemoryError::InvalidRequest));
    assert!(writer.write(PAGE_SIZE / 8, &[1]).is_err());
    drop(allocation);
    assert_eq!(
        manager.stats().user.free_frames,
        initial.user.free_frames - 2
    );
    writer.write(0, &[42]).unwrap();
    writer.read(0, &mut word).unwrap();
    assert_eq!(word, [42]);
    drop(writer);
    kernel.activate().unwrap();
    drop(user);
    assert_eq!(manager.stats(), initial);
    assert_eq!(kernel.unreserve(aperture), Err(MemoryError::InUse));
    assert_eq!(kernel.reserve_at(aperture), Err(MemoryError::AlreadyMapped));
    for region in regions {
        let address = aperture.start + (region.range.start - regions[0].range.start) as usize;
        let mapping = kernel.query(address).unwrap().and_then(|info| info.mapping);
        if region.state == MemoryRegionState::NoMap {
            assert!(mapping.is_none());
        } else {
            let mapping = mapping.unwrap();
            assert_eq!(mapping.physical_address, region.range.start);
            assert!(!mapping.attributes.user_accessible);
            if matches!(
                region.state,
                MemoryRegionState::Reserved | MemoryRegionState::BootReserved
            ) {
                assert_eq!(mapping.attributes.permissions, PagePermissions::ReadOnly);
            }
        }
    }
    for adjacent in regions.windows(2) {
        if adjacent[0].range.end < adjacent[1].range.start {
            let address =
                aperture.start + (adjacent[0].range.end - regions[0].range.start) as usize;
            assert!(lookup(kernel, address).is_none());
        }
    }
}
