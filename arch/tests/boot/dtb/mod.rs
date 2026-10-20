// SPDX-License-Identifier: MPL-2.0

mod devices;
mod memory;

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

use arch::boot::dtb::Dtb;
use kernel::{
    BootInfo, ConsoleInfo, CpuInfo, CpuInterruptTarget, DeviceConfiguration, DeviceUsePolicy,
    HardwareDeviceInstanceId, HardwareDeviceKind, HardwareDeviceLocation, HardwareInfo,
    InterruptPolarity, InterruptTrigger, MemoryRegion, MemoryRegionState, PciAddressSpace,
    PhysRange, RegisterEndian,
};

// dtc -I dts -O dtb -o arch/tests/fixtures/boot.dtb arch/tests/fixtures/boot.dts
const DTB: &[u8] = include_bytes!("../../fixtures/boot.dtb");
const LOONGARCH_DTB: &[u8] = include_bytes!("../../fixtures/loongarch.dtb");
const RISCV_DTB: &[u8] = include_bytes!("../../fixtures/riscv.dtb");
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
    let mut hardware = parse_hardware(&blob);
    drop(blob);
    assert_eq!(
        hardware.cpus.as_slice(),
        &[CpuInfo { hardware_id: 0 }, CpuInfo { hardware_id: 7 }]
    );
    let [plic, pci, uart, virtio] = hardware.devices.as_slice() else {
        panic!("missing boot devices")
    };
    assert_eq!(
        plic.configuration,
        DeviceConfiguration::InterruptController { lines: 1..96 }
    );
    assert_eq!(
        (uart.mmio_regions[0].start, uart.mmio_regions[0].end),
        (0x10000000, 0x10000100)
    );
    let DeviceConfiguration::Uart(serial) = uart.configuration else {
        panic!("missing UART configuration")
    };
    assert_eq!(
        (
            serial.register_shift,
            serial.register_width,
            serial.register_offset
        ),
        (2, 4, 8)
    );
    assert_eq!(serial.clock_frequency_hz, Some(3_686_400));
    let ConsoleInfo::Uart {
        device,
        settings: Some(settings),
    } = hardware.console
    else {
        panic!("missing console settings")
    };
    assert_eq!(device, uart.instance_id);
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
    assert_eq!(virtio.dma.unwrap().address_bits, 64);
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
    let kernel_image = PhysRange {
        start: 0x80200000,
        end: 0x80300000,
    };
    BootInfo::new(BOOT_CPU_ID, kernel_image, &mut hardware).validate();
    assert!(hardware.memory.regions.contains(&MemoryRegion {
        range: kernel_image,
        state: MemoryRegionState::Reserved
    }));
    assert_eq!(
        hardware.memory.regions[..3]
            .iter()
            .map(|region| (region.range.start, region.range.end, region.state))
            .collect::<Vec<_>>(),
        memory[..3]
    );
}

#[test]
fn pointer_and_slice_inputs_produce_the_same_hardware() {
    #[repr(align(8))]
    struct AlignedDtb([u8; DTB.len()]);
    let blob = AlignedDtb(DTB.try_into().unwrap());
    let from_slice = Dtb::from_bytes(&blob.0).unwrap();
    assert_eq!(from_slice.total_size(), DTB.len());
    // SAFETY: blob 须覆盖完整 DTB，并在 from_ptr 存续期间保持可读且不变。
    let from_ptr = unsafe { Dtb::new(blob.0.as_ptr()) }.unwrap();
    let mut expected = Box::new(HardwareInfo::new());
    let mut actual = Box::new(HardwareInfo::new());
    from_slice.dump(&mut expected).unwrap();
    from_ptr.dump(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn rejects_conflicting_device_resources() {
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

    let mut hardware = parse_hardware(LOONGARCH_DTB);
    let first = hardware
        .devices
        .iter()
        .position(|device| device.kind == HardwareDeviceKind::UART_16550)
        .unwrap()
        + 1;
    let second = first + 1;
    hardware.devices[first].use_policy = DeviceUsePolicy::UserAssignable;
    assert_eq!(
        hardware.validate_resources(),
        Err("assignable device shares a resource page")
    );
    hardware.devices[first].use_policy = DeviceUsePolicy::KernelReserved;
    let registers = hardware.devices[second].mmio_regions[0];
    hardware.devices[second].mmio_regions[0] = hardware.devices[first].mmio_regions[0];
    assert_eq!(
        hardware.validate_resources(),
        Err("device register ranges overlap")
    );
    hardware.devices[second].mmio_regions[0] = registers;
    hardware.devices[second].interrupts[0] = hardware.devices[first].interrupts[0];
    assert_eq!(
        hardware.validate_resources(),
        Err("shared IRQs are not supported")
    );
}

#[test]
fn conversion_runs_with_a_64_kib_stack() {
    let mut hardware = Box::new(HardwareInfo::new());
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
    let [plic, uart] = hardware.devices.as_slice() else {
        panic!("expected only the enabled PLIC and UART");
    };
    assert_eq!(plic.kind, HardwareDeviceKind::RISCV_PLIC);
    assert_eq!(uart.kind, HardwareDeviceKind::UART_16550);
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
        LOONGARCH_DTB,
        RISCV_DTB,
        include_bytes!("../../fixtures/nested.dtb").as_slice(),
        include_bytes!("../../fixtures/ordering.dtb").as_slice(),
    ] {
        ALLOCATIONS.set(Some(0));
        Dtb::from_bytes(bytes).unwrap().dump(&mut hardware).unwrap();
        let allocations = ALLOCATIONS.replace(None).unwrap();
        assert_eq!(allocations, 0);
    }
}

#[test]
fn loongarch_dtb_preserves_register_spaces_and_interrupt_routes() {
    // dtc -I dts -O dtb -o arch/tests/fixtures/loongarch.dtb arch/tests/fixtures/loongarch.dts
    let hardware = parse_hardware(LOONGARCH_DTB);
    assert_eq!(hardware.cpus.as_slice(), &[CpuInfo { hardware_id: 0 }]);
    let [
        eiointc,
        flash,
        fw_cfg,
        syscon,
        msi,
        pci,
        pic,
        rtc,
        uarts @ ..,
    ] = hardware.devices.as_slice()
    else {
        panic!("missing LoongArch devices")
    };
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
        flash.configuration,
        DeviceConfiguration::Flash { bank_width: 4 }
    );
    assert_eq!(flash.mmio_regions.len(), 2);
    assert!(fw_cfg.dma.unwrap().coherent);
    assert_eq!(
        msi.configuration,
        DeviceConfiguration::Msi { vectors: 32..256 }
    );
    assert_eq!(msi.interrupt_route.unwrap().controller, eiointc.instance_id);
    let DeviceConfiguration::Syscon(control) = syscon.configuration else {
        panic!("missing syscon configuration")
    };
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
    assert!(
        matches!(hardware.console, ConsoleInfo::Uart { device, .. } if device == uarts[0].instance_id)
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
        .find_map(|device| match device.configuration {
            DeviceConfiguration::Syscon(info) => Some(info),
            _ => None,
        })
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
    BootInfo::new(
        0,
        PhysRange {
            start: 0x80200000,
            end: 0x80400000,
        },
        &mut hardware,
    )
    .validate();
    assert_eq!(hardware.timebase_frequency_hz, Some(10_000_000));
    assert_eq!(hardware.cpu_interrupts.len(), 4);
    let plic = hardware
        .devices
        .iter()
        .find(|device| device.kind == HardwareDeviceKind::RISCV_PLIC)
        .unwrap();
    assert_eq!(
        plic.configuration,
        DeviceConfiguration::InterruptController { lines: 1..96 }
    );
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
        .find(|device| device.kind == HardwareDeviceKind::UART_16550)
        .unwrap();
    assert!(
        matches!(hardware.console, ConsoleInfo::Uart { device, .. } if device == uart.instance_id)
    );
    let DeviceConfiguration::Uart(serial) = uart.configuration else {
        panic!("missing UART configuration")
    };
    assert_eq!(serial.clock_frequency_hz, Some(3_686_400));
    let syscon = hardware
        .devices
        .iter()
        .find(|device| device.kind == HardwareDeviceKind::SYSCON)
        .unwrap();
    let DeviceConfiguration::Syscon(control) = syscon.configuration else {
        panic!("missing syscon configuration")
    };
    assert_eq!(control.register_width, 4);
    assert_eq!(control.poweroff.unwrap().value, 0x5555);
    assert_eq!(control.reboot.unwrap().value, 0x7777);
    assert_eq!(hardware.pci.interrupts.len(), 16);
    assert!(hardware.pci.msi_maps.is_empty());
    let pci = hardware
        .devices
        .iter()
        .find(|device| device.kind == HardwareDeviceKind::PCI_HOST_BRIDGE)
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
