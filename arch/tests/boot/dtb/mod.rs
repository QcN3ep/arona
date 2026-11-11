// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

mod devices;
mod memory;

use super::{BOOT_CPU_ID, image};

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

use arch::boot::{
    dtb::{
        Dtb,
        devices::{
            RegisterEndian,
            flash::FlashInfo,
            serial::{SerialSettings, UartInfo},
            syscon::SysconInfo,
        },
    },
    prepare_boot_info,
};
use kernel::{
    CpuInfo, CpuInterruptTarget, DeviceUsePolicy, DmaCapability, HardwareDeviceInstanceId,
    HardwareDeviceLocation, HardwareInfo, InterruptPolarity, InterruptTrigger, PciAddressSpace,
    memory::{MemoryRegion, MemoryRegionState, PhysRange},
};

// dtc -I dts -O dtb -o arch/tests/fixtures/boot.dtb arch/tests/fixtures/boot.dts
const DTB: &[u8] = include_bytes!("../../fixtures/boot.dtb");
const LOONGARCH_DTB: &[u8] = include_bytes!("../../fixtures/loongarch.dtb");
const RISCV_DTB: &[u8] = include_bytes!("../../fixtures/riscv.dtb");

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

// SAFETY: Allocation results must satisfy the layout, deallocation must return memory to the original allocator, and no method may unwind the stack.
unsafe impl GlobalAlloc for CountingAllocator {
    /// # Safety
    ///
    /// `layout.size()` must be greater than zero.
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocation();
        // SAFETY: layout.size() must be greater than zero.
        unsafe { System.alloc(layout) }
    }

    /// # Safety
    ///
    /// `layout.size()` must be greater than zero.
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_allocation();
        // SAFETY: layout.size() must be greater than zero.
        unsafe { System.alloc_zeroed(layout) }
    }

    /// # Safety
    ///
    /// `ptr` must be an allocation result not yet deallocated by this allocator, and `layout` must match the one used at allocation.
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: ptr must be an allocation result not yet deallocated by System, and layout must match the one used at allocation.
        unsafe { System.dealloc(ptr, layout) }
    }

    /// # Safety
    ///
    /// `ptr` and `layout` must refer to a valid allocation of this allocator, and `new_size` must be non-zero and satisfy the `Layout` constraints for the original alignment.
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count_allocation();
        // SAFETY: ptr and layout must refer to a valid allocation of System, and new_size must be non-zero and satisfy the Layout constraints for the original alignment.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn parse_hardware(blob: &[u8]) -> Box<HardwareInfo> {
    let mut hardware = Box::new(const { HardwareInfo::new() });
    Dtb::from_bytes(blob).unwrap().dump(&mut hardware).unwrap();
    hardware
}

#[test]
fn dtb_produces_complete_boot_data() {
    let blob = DTB.to_vec();
    let hardware = parse_hardware(&blob);
    drop(blob);
    assert_eq!(
        hardware.cpus.as_slice(),
        &[CpuInfo { hardware_id: 0 }, CpuInfo { hardware_id: 7 }]
    );
    let device = |identity| {
        hardware
            .devices
            .iter()
            .find(|device| device.has_identity("devicetree", identity))
            .unwrap()
    };
    let (plic, pci, uart, virtio) = (
        device("riscv,plic0"),
        device("pci-host-ecam-generic"),
        device("ns16550a"),
        device("virtio,mmio"),
    );
    assert_eq!(plic.interrupt_lines, Some(1..96));
    assert_eq!(
        (uart.mmio_regions[0].start, uart.mmio_regions[0].end),
        (0x10000000, 0x10000100)
    );
    let serial = UartInfo::from_device(uart, &hardware).unwrap().unwrap();
    assert_eq!(
        (
            serial.register_shift,
            serial.register_width,
            serial.register_offset
        ),
        (2, 4, 8)
    );
    assert_eq!(serial.clock_frequency_hz, Some(3_686_400));
    assert_eq!(hardware.console.as_ref().unwrap().device, uart.instance_id);
    let settings =
        SerialSettings::from_options(&hardware.console.as_ref().unwrap().options).unwrap();
    assert_eq!(
        (
            settings.baud_rate,
            settings.data_bits,
            settings.flow_control
        ),
        (115200, 8, true)
    );
    assert_eq!(
        (uart.interrupts[0].controller, uart.interrupts[0].line),
        (plic.instance_id, 10)
    );
    assert_eq!(
        (virtio.interrupts[0].controller, virtio.interrupts[0].line),
        (plic.instance_id, 1)
    );
    assert_eq!(virtio.use_policy, DeviceUsePolicy::UserAssignable);
    let DmaCapability::Present(dma) = virtio.dma else {
        panic!("missing DMA capability")
    };
    assert_eq!(dma.address_bits, 64);
    let HardwareDeviceLocation::PciHostBridge {
        segment,
        first_bus,
        last_bus,
        ecam,
        windows,
        iommu,
    } = &pci.location
    else {
        panic!("missing PCI host")
    };
    assert_eq!(
        (*segment, *first_bus, *last_bus, ecam.start, ecam.end),
        (0, 0, 255, 0x30000000, 0x40000000)
    );
    assert_eq!(
        windows
            .iter()
            .map(|window| (window.bus_address, window.cpu_range.start, window.space))
            .collect::<Vec<_>>(),
        [(0x40000000, 0x40000000, PciAddressSpace::Memory32)]
    );
    assert!(pci.mmio_regions.is_empty());
    assert!(iommu.is_none());
    let memory = hardware
        .memory
        .regions
        .iter()
        .map(|region| (region.range.start, region.range.end, region.state))
        .collect::<Vec<_>>();
    assert_eq!(
        memory,
        [
            (0x80000000, 0x80002000, MemoryRegionState::Reserved),
            (0x80002000, 0x80003000, MemoryRegionState::NoMap),
            (0x80003000, 0x80004000, MemoryRegionState::BootReserved),
            (0x80004000, 0x88000000, MemoryRegionState::Available),
            (0xa0000000, 0xa0100000, MemoryRegionState::Available),
        ]
    );
}

#[test]
fn pointer_and_slice_inputs_produce_the_same_hardware() {
    #[repr(align(8))]
    struct AlignedDtb([u8; DTB.len()]);
    let blob = AlignedDtb(DTB.try_into().unwrap());
    let from_slice = Dtb::from_bytes(&blob.0).unwrap();
    assert_eq!(from_slice.total_size(), DTB.len());
    // SAFETY: blob must cover the complete DTB and remain readable and unchanged while from_ptr is alive.
    let from_ptr = unsafe { Dtb::new(blob.0.as_ptr()) }.unwrap();
    let mut expected = Box::new(const { HardwareInfo::new() });
    let mut actual = Box::new(const { HardwareInfo::new() });
    from_slice.dump(&mut expected).unwrap();
    from_ptr.dump(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn conversion_runs_with_a_64_kib_stack() {
    let mut hardware = Box::new(const { HardwareInfo::new() });
    std::thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(move || {
            Dtb::from_bytes(DTB).unwrap().dump(&mut hardware).unwrap();
            Dtb::from_bytes(include_bytes!("../../fixtures/nested.dtb"))
                .unwrap()
                .dump(&mut hardware)
                .unwrap();
            Dtb::from_bytes(LOONGARCH_DTB)
                .unwrap()
                .dump(&mut hardware)
                .unwrap();
            Dtb::from_bytes(RISCV_DTB)
                .unwrap()
                .dump(&mut hardware)
                .unwrap();
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn deep_buses_translate_addresses_and_inherit_interrupt_parents() {
    // dtc -I dts -O dtb -o arch/tests/fixtures/nested.dtb arch/tests/fixtures/nested.dts
    let hardware = parse_hardware(include_bytes!("../../fixtures/nested.dtb"));
    let plic = hardware
        .devices
        .iter()
        .find(|device| device.has_identity("devicetree", "riscv,plic0"))
        .unwrap();
    let uart = hardware
        .devices
        .iter()
        .find(|device| device.has_identity("devicetree", "ns16550a"))
        .unwrap();
    assert!(uart.parent.is_some());
    assert!(
        hardware
            .devices
            .iter()
            .all(|device| !device.has_identity("devicetree", "virtio,mmio"))
    );
    assert!(plic.has_identity("devicetree", "riscv,plic0"));
    assert!(uart.has_identity("devicetree", "ns16550a"));
    assert_eq!(
        uart.mmio_regions.as_slice(),
        &[PhysRange {
            start: 0x10000000,
            end: 0x10000100
        }]
    );
    assert_eq!(uart.interrupts[0].controller, plic.instance_id);
    assert_eq!(uart.interrupts[0].line, 10);
}

#[test]
fn device_ids_follow_full_paths_and_preserve_compatible_order() {
    // dtc -I dts -O dtb -o arch/tests/fixtures/ordering.dtb arch/tests/fixtures/ordering.dts
    let hardware = parse_hardware(include_bytes!("../../fixtures/ordering.dtb"));
    let [bus, uart, virtio] = hardware.devices.as_slice() else {
        panic!("expected bus, UART and Virtio devices");
    };
    // `/bus-aux@10002000` is ordered before `/bus/device@10001000`, differing from the depth-first traversal order.
    assert!(uart.has_identity("devicetree", "ns16550a"));
    assert_eq!(uart.instance_id, HardwareDeviceInstanceId(2));
    assert!(virtio.has_identity("devicetree", "virtio,mmio"));
    assert_eq!(bus.instance_id, HardwareDeviceInstanceId(1));
    assert_eq!(virtio.instance_id, HardwareDeviceInstanceId(3));
    assert_eq!(virtio.parent, Some(bus.instance_id));
    assert_eq!(
        virtio
            .identities
            .iter()
            .map(|identity| identity.value.as_str())
            .collect::<Vec<_>>(),
        ["virtio,mmio", "ns16550a"]
    );
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
    let mut hardware = Box::new(const { HardwareInfo::new() });
    for bytes in [
        DTB,
        LOONGARCH_DTB,
        RISCV_DTB,
        include_bytes!("../../fixtures/nested.dtb").as_slice(),
        include_bytes!("../../fixtures/ordering.dtb").as_slice(),
    ] {
        ALLOCATIONS.set(Some(0));
        Dtb::from_bytes(bytes).unwrap().dump(&mut hardware).unwrap();
        let start = hardware
            .memory
            .regions
            .iter()
            .find(|region| region.state == MemoryRegionState::Available)
            .unwrap()
            .range
            .start;
        let boot_cpu_id = hardware.cpus[0].hardware_id;
        prepare_boot_info(
            boot_cpu_id,
            image(PhysRange {
                start,
                end: start + 5 * 4096,
            }),
            &mut hardware,
        )
        .unwrap();
        let allocations = ALLOCATIONS.replace(None).unwrap();
        assert_eq!(allocations, 0);
    }
}

#[test]
fn loongarch_dtb_preserves_register_spaces_and_interrupt_routes() {
    // dtc -I dts -O dtb -o arch/tests/fixtures/loongarch.dtb arch/tests/fixtures/loongarch.dts
    let hardware = parse_hardware(LOONGARCH_DTB);
    assert_eq!(hardware.cpus.as_slice(), &[CpuInfo { hardware_id: 0 }]);
    let device = |identity| {
        hardware
            .devices
            .iter()
            .find(|device| device.has_identity("devicetree", identity))
            .unwrap()
    };
    let (eiointc, flash, fw_cfg, syscon, msi, pci, pic, rtc) = (
        device("loongson,ls2k2000-eiointc"),
        device("cfi-flash"),
        device("qemu,fw-cfg-mmio"),
        device("syscon"),
        device("loongson,pch-msi-1.0"),
        device("pci-host-ecam-generic"),
        device("loongson,pch-pic-1.0"),
        device("loongson,ls7a-rtc"),
    );
    let uarts = hardware
        .devices
        .iter()
        .filter(|device| device.has_identity("devicetree", "ns16550a"))
        .collect::<Vec<_>>();
    assert_eq!(
        eiointc.location,
        HardwareDeviceLocation::Iocsr {
            registers: 0x1400..0x1c00
        }
    );
    assert!(eiointc.mmio_regions.is_empty());
    assert_eq!(
        (
            hardware.cpu_interrupts[0].target,
            hardware.cpu_interrupts[0].line
        ),
        (CpuInterruptTarget::AllCpus, 3)
    );
    assert_eq!(
        FlashInfo::from_device(flash, &hardware)
            .unwrap()
            .unwrap()
            .bank_width,
        4
    );
    assert_eq!(flash.mmio_regions.len(), 2);
    let DmaCapability::Present(dma) = fw_cfg.dma else {
        panic!("missing DMA capability")
    };
    assert!(dma.coherent);
    assert_eq!(msi.msi_vectors, Some(32..256));
    assert_eq!(msi.interrupt_route.unwrap().controller, eiointc.instance_id);
    let control = SysconInfo::from_device(syscon, &hardware).unwrap().unwrap();
    assert_eq!(
        (control.register_width, control.endian),
        (1, RegisterEndian::Little)
    );
    assert_eq!(
        (
            control.poweroff.unwrap().offset,
            control.poweroff.unwrap().value,
            control.poweroff.unwrap().mask
        ),
        (0, 0x34, 0xff)
    );
    assert_eq!(
        (
            control.reboot.unwrap().offset,
            control.reboot.unwrap().value
        ),
        (2, 0x42)
    );
    let HardwareDeviceLocation::PciHostBridge { windows, .. } = &pci.location else {
        panic!("missing PCI host")
    };
    assert_eq!(
        (
            windows[0].bus_address,
            windows[0].cpu_range.start,
            windows[0].space
        ),
        (0x40000000, 0x40000000, PciAddressSpace::Memory32)
    );
    assert_eq!(windows.len(), 1);
    assert_eq!(hardware.pci.interrupts.len(), 16);
    assert_eq!(hardware.pci.msi_maps.len(), 1);
    assert_eq!(
        (
            hardware.pci.msi_maps[0].controller,
            hardware.pci.msi_maps[0].requester_count
        ),
        (msi.instance_id, 65536)
    );
    for route in &hardware.pci.interrupts {
        assert_eq!(
            (route.host, route.interrupt.controller),
            (pci.instance_id, pic.instance_id)
        );
        assert_eq!(
            route.interrupt.line,
            16 + ((u32::from(route.requester_id >> 3) + u32::from(route.pin) - 1) % 4)
        );
    }
    assert_eq!(
        (pic.mmio_regions[0].start, pic.mmio_regions[0].end),
        (0x10000000, 0x10000400)
    );
    assert_eq!(
        (
            pic.interrupt_route.unwrap().controller,
            pic.interrupt_route.unwrap().base_line
        ),
        (eiointc.instance_id, 0)
    );
    assert_eq!(
        (rtc.mmio_regions[0].start, rtc.interrupts[0].line),
        (0x100d0100, 6)
    );
    assert_eq!(
        hardware.console.as_ref().unwrap().device,
        uarts[0].instance_id
    );
    assert_eq!(uarts.len(), 4);
    for (index, uart) in uarts.iter().enumerate() {
        let start = 0x1fe001e0 + index as u64 * 0x100;
        assert_eq!(
            (uart.mmio_regions[0].start, uart.mmio_regions[0].end),
            (start, start + 0x100)
        );
        assert_eq!(
            (uart.interrupts[0].controller, uart.interrupts[0].line),
            (pic.instance_id, index as u32 + 2)
        );
        assert_eq!(
            (uart.interrupts[0].trigger, uart.interrupts[0].polarity),
            (
                Some(InterruptTrigger::Level),
                Some(InterruptPolarity::ActiveHigh)
            )
        );
    }
    let mut blob = LOONGARCH_DTB.to_vec();
    let header = *fdt::Fdt::new_unaligned(&blob).unwrap().header();
    let strings = header.strings_offset as usize;
    let offset = strings
        + blob[strings..]
            .windows(6)
            .position(|bytes| bytes == b"value\0")
            .unwrap();
    blob[offset..offset + 5].copy_from_slice(b"mask\0");
    let hardware = parse_hardware(&blob);
    let control = hardware
        .devices
        .iter()
        .find_map(|device| SysconInfo::from_device(device, &hardware).unwrap())
        .unwrap();
    assert_eq!(
        (
            control.poweroff.unwrap().mask,
            control.poweroff.unwrap().value
        ),
        (0xff, 0x34)
    );
}

#[test]
fn riscv_dtb_preserves_cpu_contexts_and_platform_devices() {
    let mut hardware = parse_hardware(RISCV_DTB);
    prepare_boot_info(
        0,
        image(PhysRange {
            start: 0x80200000,
            end: 0x80400000,
        }),
        &mut hardware,
    )
    .unwrap();
    assert_eq!(hardware.timebase_frequency_hz, Some(10_000_000));
    assert_eq!(hardware.cpu_interrupts.len(), 4);
    let plic = hardware
        .devices
        .iter()
        .find(|device| device.has_identity("devicetree", "riscv,plic0"))
        .unwrap();
    assert_eq!(plic.interrupt_lines, Some(1..96));
    let contexts = hardware
        .cpu_interrupts
        .iter()
        .filter(|route| route.controller == plic.instance_id)
        .collect::<Vec<_>>();
    assert_eq!((contexts[0].output, contexts[0].line), (0, 11));
    assert_eq!((contexts[1].output, contexts[1].line), (1, 9));
    assert_eq!(contexts[1].target, CpuInterruptTarget::Cpu(0));
    let uart = hardware
        .devices
        .iter()
        .find(|device| device.has_identity("devicetree", "ns16550a"))
        .unwrap();
    assert_eq!(hardware.console.as_ref().unwrap().device, uart.instance_id);
    let serial = UartInfo::from_device(uart, &hardware).unwrap().unwrap();
    assert_eq!(serial.clock_frequency_hz, Some(3_686_400));
    let syscon = hardware
        .devices
        .iter()
        .find(|device| device.has_identity("devicetree", "syscon"))
        .unwrap();
    let control = SysconInfo::from_device(syscon, &hardware).unwrap().unwrap();
    assert_eq!(control.register_width, 4);
    assert_eq!(control.poweroff.unwrap().value, 0x5555);
    assert_eq!(control.reboot.unwrap().value, 0x7777);
    assert_eq!(hardware.pci.interrupts.len(), 16);
    assert!(hardware.pci.msi_maps.is_empty());
    let pci = hardware
        .devices
        .iter()
        .find(|device| device.has_identity("devicetree", "pci-host-ecam-generic"))
        .unwrap();
    let HardwareDeviceLocation::PciHostBridge { windows, .. } = &pci.location else {
        panic!("missing PCI host")
    };
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[1].bus_address, 0x4_00000000);
    assert_eq!(windows[1].space, PciAddressSpace::Memory64);
    for route in &hardware.pci.interrupts {
        assert_eq!(route.interrupt.controller, plic.instance_id);
        assert_eq!(
            route.interrupt.line,
            32 + ((u32::from(route.requester_id >> 3) + u32::from(route.pin) - 1) % 4)
        );
    }
    let mut blob = RISCV_DTB.to_vec();
    let tree = fdt::Fdt::new_unaligned(&blob).unwrap();
    let property = tree
        .find_node("/soc/interrupt-controller@c000000")
        .unwrap()
        .raw_property("interrupts-extended")
        .unwrap();
    let offset = property.value.as_ptr() as usize - blob.as_ptr() as usize + 4;
    blob[offset..offset + 4].copy_from_slice(&u32::MAX.to_be_bytes());
    let hardware = parse_hardware(&blob);
    let contexts = hardware
        .cpu_interrupts
        .iter()
        .filter(|route| route.line == 9)
        .collect::<Vec<_>>();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0].output, 1);
}
