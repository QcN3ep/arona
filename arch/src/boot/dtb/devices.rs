// SPDX-License-Identifier: MPL-2.0

//! 转换受支持的设备绑定并关联中断控制器。

mod interrupts;
mod pci;
mod serial;
mod syscon;

use core::ops::Range;

use arrayvec::ArrayVec;
use fdt::properties::Compatible;
use kernel::{
    DeviceConfiguration, DeviceUsePolicy, DmaInfo, HardwareDeviceInfo, HardwareDeviceInstanceId,
    HardwareDeviceKind, HardwareDeviceLocation, HardwareInfo, PhysRange,
    boot::{MAX_DEVICES, MAX_MMIO_REGIONS},
};

use super::{
    Dtb, Result,
    nodes::{Node, Nodes, registers},
    range,
};

const PAGE_SIZE: u64 = 4096;

#[derive(Debug)]
struct DeviceNode<'a> {
    // fdt 的 NodeId 假定字符串块位于结构块后，这里用节点名在 DTB 中的位置标识节点。
    name: &'a str,
    kind: HardwareDeviceKind,
}

pub(super) fn parse(dtb: &Dtb<'_>, hardware: &mut HardwareInfo) -> Result<()> {
    let mut nodes = ArrayVec::new();
    collect_nodes(dtb, &mut nodes)?;
    hardware.devices.clear();
    hardware.cpu_interrupts.clear();
    hardware.pci.interrupts.clear();
    hardware.pci.msi_maps.clear();
    let mut walk = dtb.nodes();
    while let Some(node) = walk.next()? {
        let name = node.name()?.name;
        if let Some(index) = nodes
            .iter()
            .position(|device| core::ptr::eq(device.name, name))
        {
            hardware
                .devices
                .push(parse_device(dtb, &walk, index, &nodes)?);
            match nodes[index].kind {
                HardwareDeviceKind::RISCV_PLIC
                | HardwareDeviceKind::RISCV_CLINT
                | HardwareDeviceKind::LOONGARCH_EIOINTC => {
                    interrupts::cpu_routes(
                        dtb,
                        &walk,
                        instance_id(index),
                        &mut hardware.cpu_interrupts,
                    )?;
                }
                HardwareDeviceKind::PCI_HOST_BRIDGE => {
                    pci::routes(dtb, &walk, index, &nodes, &mut hardware.pci)?
                }
                _ => {}
            }
        }
    }
    hardware
        .devices
        .as_mut_slice()
        .sort_unstable_by_key(|device| device.instance_id.0);
    syscon::operations(dtb, &nodes, &mut hardware.devices)?;
    hardware.console = serial::console(dtb, &nodes).unwrap_or_default();
    Ok(())
}

fn collect_nodes<'a>(
    dtb: &Dtb<'a>,
    output: &mut ArrayVec<DeviceNode<'a>, MAX_DEVICES>,
) -> Result<()> {
    let mut entries = ArrayVec::<(Range<usize>, DeviceNode<'a>), MAX_DEVICES>::new();
    let mut paths = ArrayVec::<u8, 8192>::new();
    let mut walk = dtb.nodes();
    while let Some(node) = walk.next()? {
        if let Some(kind) = kind(node)? {
            let start = paths.len();
            for byte in walk.path() {
                paths
                    .try_push(byte)
                    .map_err(|_| "device paths exceed 8 KiB")?;
            }
            entries
                .try_push((
                    start..paths.len(),
                    DeviceNode {
                        name: node.name()?.name,
                        kind,
                    },
                ))
                .map_err(|_| "too many devices")?;
        }
    }
    entries
        .as_mut_slice()
        .sort_unstable_by(|(a, _), (b, _)| paths[a.clone()].cmp(&paths[b.clone()]));
    if entries
        .windows(2)
        .any(|pair| paths[pair[0].0.clone()] == paths[pair[1].0.clone()])
    {
        return Err("duplicate device path".into());
    }
    output.extend(entries.drain(..).map(|(_, node)| node));
    Ok(())
}

fn parse_device(
    dtb: &Dtb<'_>,
    walk: &Nodes<'_>,
    index: usize,
    nodes: &[DeviceNode<'_>],
) -> Result<HardwareDeviceInfo> {
    let node = walk.node();
    let kind = nodes[index].kind;
    let virtio = kind == HardwareDeviceKind::VIRTIO_MMIO;
    let location = match kind {
        HardwareDeviceKind::PCI_HOST_BRIDGE => pci::parse(walk)?,
        HardwareDeviceKind::LOONGARCH_EIOINTC => {
            let mut entries = registers(node)?.iter::<u64, u64>();
            let entry = entries.next().ok_or("EIOINTC is missing reg")??;
            if entries.next().is_some() {
                return Err("EIOINTC has multiple register ranges".into());
            }
            // REFERENCE: https://docs.kernel.org/arch/loongarch/irq-chip-model.html
            let registers = range(entry.address, entry.len)?;
            let start = registers
                .start
                .try_into()
                .map_err(|_| "IOCSR address overflow")?;
            let end = registers
                .end
                .try_into()
                .map_err(|_| "IOCSR address overflow")?;
            HardwareDeviceLocation::Iocsr {
                registers: start..end,
            }
        }
        _ => HardwareDeviceLocation::Platform,
    };
    let mmio_regions = match &location {
        HardwareDeviceLocation::Platform => mmio(walk)?,
        _ => ArrayVec::new(),
    };
    let interrupts = if matches!(
        kind,
        HardwareDeviceKind::RISCV_PLIC
            | HardwareDeviceKind::RISCV_CLINT
            | HardwareDeviceKind::LOONGARCH_EIOINTC
            | HardwareDeviceKind::LOONGARCH_PCH_MSI
    ) {
        ArrayVec::new()
    } else {
        interrupts::parse(dtb, walk, nodes)?
    };
    Ok(HardwareDeviceInfo {
        instance_id: instance_id(index),
        kind,
        use_policy: if virtio {
            DeviceUsePolicy::UserAssignable
        } else {
            DeviceUsePolicy::KernelReserved
        },
        location,
        mmio_regions,
        interrupts,
        interrupt_route: if matches!(
            kind,
            HardwareDeviceKind::LOONGARCH_PCH_PIC | HardwareDeviceKind::LOONGARCH_PCH_MSI
        ) {
            Some(interrupts::cascade(dtb, walk, nodes, kind)?)
        } else {
            None
        },
        configuration: match kind {
            HardwareDeviceKind::UART_16550 => DeviceConfiguration::Uart(serial::parse(dtb, node)?),
            HardwareDeviceKind::RISCV_PLIC
            | HardwareDeviceKind::LOONGARCH_EIOINTC
            | HardwareDeviceKind::LOONGARCH_PCH_PIC
            | HardwareDeviceKind::LOONGARCH_PCH_MSI => interrupts::configuration(node, kind)?,
            HardwareDeviceKind::SYSCON => DeviceConfiguration::Syscon(syscon::parse(node)?),
            _ => DeviceConfiguration::None,
        },
        dma: dma(walk, kind)?,
    })
}

fn instance_id(index: usize) -> HardwareDeviceInstanceId {
    HardwareDeviceInstanceId((index + 1) as u32)
}

fn mmio(walk: &Nodes<'_>) -> Result<ArrayVec<PhysRange, MAX_MMIO_REGIONS>> {
    let mut output = ArrayVec::new();
    for entry in registers(walk.node())?.iter::<u64, u64>() {
        let entry = entry?;
        let address = walk.translate(range(entry.address, entry.len)?)?;
        output
            .try_push(address)
            .map_err(|_| "too many MMIO ranges")?;
    }
    Ok(output)
}

fn kind(node: Node<'_>) -> Result<Option<HardwareDeviceKind>> {
    let Some(compatible) = node.property::<Compatible>()? else {
        return Ok(None);
    };
    Ok(compatible.into_iter().find_map(|name| match name {
        "ns16550a" | "ns16550" => Some(HardwareDeviceKind::UART_16550),
        "riscv,plic0" | "sifive,plic-1.0.0" => Some(HardwareDeviceKind::RISCV_PLIC),
        "virtio,mmio" => Some(HardwareDeviceKind::VIRTIO_MMIO),
        "pci-host-ecam-generic" => Some(HardwareDeviceKind::PCI_HOST_BRIDGE),
        "loongson,ls2k2000-eiointc" => Some(HardwareDeviceKind::LOONGARCH_EIOINTC),
        "loongson,pch-pic-1.0" => Some(HardwareDeviceKind::LOONGARCH_PCH_PIC),
        "loongson,ls7a-rtc" => Some(HardwareDeviceKind::LOONGARCH_LS7A_RTC),
        "loongson,pch-msi-1.0" => Some(HardwareDeviceKind::LOONGARCH_PCH_MSI),
        "syscon" => Some(HardwareDeviceKind::SYSCON),
        "riscv,clint0" | "sifive,clint0" => Some(HardwareDeviceKind::RISCV_CLINT),
        "google,goldfish-rtc" => Some(HardwareDeviceKind::GOLDFISH_RTC),
        _ => None,
    }))
}

fn dma(walk: &Nodes<'_>, kind: HardwareDeviceKind) -> Result<Option<DmaInfo>> {
    if !matches!(
        kind,
        HardwareDeviceKind::VIRTIO_MMIO | HardwareDeviceKind::PCI_HOST_BRIDGE
    ) {
        return Ok(None);
    }
    let node = walk.node();
    if node.raw_property("iommus")?.is_some() || node.raw_property("iommu-map")?.is_some() {
        return Err("IOMMU device bindings are not supported".into());
    }
    let mut coherent = None;
    for bus in core::iter::once(node).chain(walk.ancestors()) {
        if bus
            .raw_property("dma-ranges")?
            .is_some_and(|property| !property.value.is_empty())
        {
            return Err("DMA address translation is not supported".into());
        }
        let yes = bus.raw_property("dma-coherent")?.is_some();
        let no = bus.raw_property("dma-noncoherent")?.is_some();
        if yes && no {
            return Err("conflicting DMA coherency properties".into());
        }
        if coherent.is_none() && (yes || no) {
            coherent = Some(yes);
        }
    }
    Ok(Some(DmaInfo {
        address_bits: 64,
        coherent: coherent.unwrap_or(false),
        isolation: None,
    }))
}

fn endian(node: Node<'_>) -> Result<kernel::RegisterEndian> {
    let big = node.raw_property("big-endian")?.is_some();
    let little = node.raw_property("little-endian")?.is_some();
    let native = node.raw_property("native-endian")?.is_some();
    if u8::from(big) + u8::from(little) + u8::from(native) > 1 {
        return Err("conflicting register endianness".into());
    }
    Ok(if big || native && cfg!(target_endian = "big") {
        kernel::RegisterEndian::Big
    } else {
        kernel::RegisterEndian::Little
    })
}
