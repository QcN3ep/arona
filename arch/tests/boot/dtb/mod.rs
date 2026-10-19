// SPDX-License-Identifier: MPL-2.0

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

use arch::boot::dtb::Dtb;
use kernel::{
    BootInfo, CpuInfo, DeviceUsePolicy, HardwareDeviceInstanceId, HardwareDeviceKind,
    HardwareDeviceLocation, HardwareInfo, MemoryRegion, MemoryRegionState, PhysRange,
};

// dtc -I dts -O dtb -o arch/tests/fixtures/boot.dtb arch/tests/fixtures/boot.dts
const DTB: &[u8] = include_bytes!("../../fixtures/boot.dtb");
const BOOT_CPU_ID: u64 = 7;

thread_local! {
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

struct CountingAllocator;

fn count_allocation() {
    let _ = ALLOCATIONS.try_with(|count| {
        if let Some(value) = count.get() {
            count.set(Some(value + 1));
        }
    });
}

// SAFETY: 分配结果须满足布局，释放须归还原分配器，且所有方法均不得展开栈。
unsafe impl GlobalAlloc for CountingAllocator {
    /// # Safety
    ///
    /// `layout.size()` 须大于零。
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocation();
        // SAFETY: layout.size() 须大于零。
        unsafe { System.alloc(layout) }
    }

    /// # Safety
    ///
    /// `layout.size()` 须大于零。
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_allocation();
        // SAFETY: layout.size() 须大于零。
        unsafe { System.alloc_zeroed(layout) }
    }

    /// # Safety
    ///
    /// `ptr` 须为此分配器尚未释放的分配结果，`layout` 须与分配时一致。
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: ptr 须为 System 尚未释放的分配结果，layout 须与分配时一致。
        unsafe { System.dealloc(ptr, layout) }
    }

    /// # Safety
    ///
    /// `ptr` 和 `layout` 须对应此分配器的有效分配，`new_size` 须非零且满足原对齐下的 `Layout` 限制。
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count_allocation();
        // SAFETY: ptr 和 layout 须对应 System 的有效分配，new_size 须非零且满足原对齐下的 Layout 限制。
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn parse_hardware(blob: &[u8]) -> Box<HardwareInfo> {
    let mut hardware = Box::new(HardwareInfo::new());
    Dtb::from_bytes(blob).unwrap().dump(&mut hardware).unwrap();
    hardware
}

#[test]
fn dtb_produces_complete_boot_data() {
    let blob = DTB.to_vec();
    let hardware = parse_hardware(&blob);
    drop(blob);
    BootInfo {
        boot_cpu_id: BOOT_CPU_ID,
        kernel_image: PhysRange {
            start: 0x80200000,
            end: 0x80300000,
        },
        hardware: &hardware,
    }
    .validate();
    assert_eq!(
        hardware.cpus.as_slice(),
        &[CpuInfo { hardware_id: 0 }, CpuInfo { hardware_id: 7 }]
    );
    let [plic, pci, uart, virtio] = hardware.devices.as_slice() else {
        unreachable!()
    };
    assert_eq!(plic.kind, HardwareDeviceKind::RISCV_PLIC);
    assert_eq!(plic.instance_id, HardwareDeviceInstanceId(1));
    assert_eq!(uart.kind, HardwareDeviceKind::UART_16550);
    assert_eq!(uart.instance_id, HardwareDeviceInstanceId(3));
    assert_eq!(uart.use_policy, DeviceUsePolicy::KernelReserved);
    assert_eq!(
        uart.mmio_regions.as_slice(),
        &[PhysRange {
            start: 0x10000000,
            end: 0x10001000
        }]
    );
    assert_eq!(uart.interrupts[0].controller, plic.instance_id);
    assert_eq!(uart.interrupts[0].line, 10);
    assert_eq!(virtio.interrupts[0].controller, plic.instance_id);
    assert_eq!(virtio.interrupts[0].line, 1);
    assert_eq!(virtio.use_policy, DeviceUsePolicy::UserAssignable);
    assert_eq!(virtio.dma.unwrap().address_bits, 64);
    let HardwareDeviceLocation::PciHostBridge {
        segment,
        first_bus,
        last_bus,
        ecam,
        memory_windows,
        iommu,
    } = &pci.location
    else {
        unreachable!()
    };
    assert_eq!((*segment, *first_bus, *last_bus), (0, 0, 255));
    assert_eq!(
        *ecam,
        PhysRange {
            start: 0x30000000,
            end: 0x40000000
        }
    );
    assert_eq!(
        memory_windows.as_slice(),
        &[PhysRange {
            start: 0x40000000,
            end: 0x80000000
        }]
    );
    assert!(pci.mmio_regions.is_empty());
    assert!(iommu.is_none());
    let memory = [
        (0x80000000, 0x80002000, MemoryRegionState::Reserved),
        (0x80002000, 0x80003000, MemoryRegionState::NoMap),
        (0x80003000, 0x80004000, MemoryRegionState::BootReserved),
        (0x80004000, 0x88000000, MemoryRegionState::Available),
        (0xa0000000, 0xa0100000, MemoryRegionState::Available),
    ]
    .map(|(start, end, state)| MemoryRegion {
        range: PhysRange { start, end },
        state,
    });
    assert_eq!(hardware.memory.regions.as_slice(), memory);
}

#[test]
fn pointer_and_slice_inputs_produce_the_same_hardware() {
    #[repr(align(8))]
    struct AlignedDtb([u8; DTB.len()]);
    let blob = AlignedDtb(DTB.try_into().unwrap());
    let from_slice = Dtb::from_bytes(&blob.0).unwrap();
    // SAFETY: blob 须覆盖完整 DTB，并在 from_ptr 存续期间保持可读且不变。
    let from_ptr = unsafe { Dtb::new(blob.0.as_ptr()) }.unwrap();
    let mut expected = Box::new(HardwareInfo::new());
    let mut actual = Box::new(HardwareInfo::new());
    from_slice.dump(&mut expected).unwrap();
    from_ptr.dump(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn rejects_firmware_assigning_the_same_memory_to_two_devices() {
    let mut blob = DTB.to_vec();
    let fdt = fdt::Fdt::new_unaligned(&blob).unwrap();
    let reg = fdt
        .find_node("/soc@10000000/virtio@1000")
        .unwrap()
        .raw_property("reg")
        .unwrap();
    let offset = reg.value.as_ptr() as usize - blob.as_ptr() as usize;
    blob[offset..offset + 8].copy_from_slice(&0_u64.to_be_bytes());
    let mut hardware = Box::new(HardwareInfo::new());
    assert!(Dtb::from_bytes(&blob).unwrap().dump(&mut hardware).is_err());
}

#[test]
fn conversion_runs_with_a_64_kib_stack() {
    let mut hardware = Box::new(HardwareInfo::new());
    std::thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(move || {
            Dtb::from_bytes(DTB).unwrap().dump(&mut hardware).unwrap();
            assert_eq!(hardware.cpus.len(), 2);
            assert_eq!(hardware.devices.len(), 4);
            Dtb::from_bytes(include_bytes!("../../fixtures/nested.dtb"))
                .unwrap()
                .dump(&mut hardware)
                .unwrap();
            assert_eq!(hardware.devices.len(), 2);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn deep_buses_translate_addresses_and_inherit_interrupt_parents() {
    // dtc -I dts -O dtb -o arch/tests/fixtures/nested.dtb arch/tests/fixtures/nested.dts
    let hardware = parse_hardware(include_bytes!("../../fixtures/nested.dtb"));
    let [plic, uart] = hardware.devices.as_slice() else {
        panic!("expected only the enabled PLIC and UART");
    };
    assert_eq!(plic.kind, HardwareDeviceKind::RISCV_PLIC);
    assert_eq!(uart.kind, HardwareDeviceKind::UART_16550);
    assert_eq!(
        uart.mmio_regions.as_slice(),
        &[PhysRange {
            start: 0x10000000,
            end: 0x10001000
        }]
    );
    assert_eq!(uart.interrupts[0].controller, plic.instance_id);
    assert_eq!(uart.interrupts[0].line, 10);
}

#[test]
fn device_ids_follow_full_paths_and_bindings_follow_compatible_order() {
    // dtc -I dts -O dtb -o arch/tests/fixtures/ordering.dtb arch/tests/fixtures/ordering.dts
    let hardware = parse_hardware(include_bytes!("../../fixtures/ordering.dtb"));
    let [uart, virtio] = hardware.devices.as_slice() else {
        panic!("expected UART and Virtio devices");
    };
    // `/bus-aux@10002000` 排在 `/bus/device@10001000` 前，与深度优先遍历的次序不同。
    assert_eq!(uart.kind, HardwareDeviceKind::UART_16550);
    assert_eq!(uart.instance_id, HardwareDeviceInstanceId(1));
    assert_eq!(virtio.kind, HardwareDeviceKind::VIRTIO_MMIO);
    assert_eq!(virtio.instance_id, HardwareDeviceInstanceId(2));
}

#[test]
fn block_order_does_not_change_hardware_or_device_ids() {
    let header = *fdt::Fdt::new_unaligned(DTB).unwrap().header();
    let structures =
        header.structs_offset as usize..(header.structs_offset + header.structs_size) as usize;
    let strings =
        header.strings_offset as usize..(header.strings_offset + header.strings_size) as usize;
    let mut blob = DTB[..structures.start].to_vec();
    let strings_offset = blob.len();
    blob.extend_from_slice(&DTB[strings]);
    blob.resize(blob.len().next_multiple_of(4), 0);
    let structures_offset = blob.len();
    blob.extend_from_slice(&DTB[structures]);
    for (offset, value) in [
        (4, blob.len()),
        (8, structures_offset),
        (12, strings_offset),
    ] {
        blob[offset..offset + 4].copy_from_slice(&(value as u32).to_be_bytes());
    }
    assert_eq!(parse_hardware(&blob), parse_hardware(DTB));
}

#[test]
fn dtb_conversion_does_not_allocate() {
    let mut hardware = Box::new(HardwareInfo::new());
    for bytes in [
        DTB,
        include_bytes!("../../fixtures/nested.dtb").as_slice(),
        include_bytes!("../../fixtures/ordering.dtb").as_slice(),
    ] {
        ALLOCATIONS.set(Some(0));
        Dtb::from_bytes(bytes).unwrap().dump(&mut hardware).unwrap();
        let allocations = ALLOCATIONS.replace(None).unwrap();
        assert_eq!(allocations, 0);
    }
}
