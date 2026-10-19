// SPDX-License-Identifier: MPL-2.0

//! 转换受支持的设备绑定并关联中断控制器。

mod interrupts;
mod pci;

use core::ops::Range;

use arrayvec::ArrayVec;
use fdt::properties::Compatible;
use kernel::{
    DeviceUsePolicy, DmaInfo, HardwareDeviceInfo, HardwareDeviceInstanceId, HardwareDeviceKind,
    HardwareDeviceLocation, PhysRange,
    boot::{MAX_DEVICES, MAX_MMIO_REGIONS},
};

use super::{
    Dtb, Result,
    nodes::{Node, Nodes, property, registers},
    range,
};

const PAGE_SIZE: u64 = 4096;

#[derive(Debug)]
struct DeviceNode<'a> {
    // fdt 的 NodeId 假定字符串块位于结构块后，这里用节点名在 DTB 中的位置标识节点。
    name: &'a str,
    kind: HardwareDeviceKind,
}

pub(super) fn parse(
    dtb: &Dtb<'_>,
    output: &mut ArrayVec<HardwareDeviceInfo, MAX_DEVICES>,
) -> Result<()> {
    let mut nodes = ArrayVec::new();
    collect_nodes(dtb, &mut nodes)?;
    output.clear();
    let mut walk = dtb.nodes();
    while let Some(node) = walk.next()? {
        let name = node.name()?.name;
        if let Some(index) = nodes
            .iter()
            .position(|device| core::ptr::eq(device.name, name))
        {
            output.push(parse_device(dtb, &walk, index, &nodes)?);
        }
    }
    output
        .as_mut_slice()
        .sort_unstable_by_key(|device| device.instance_id.0);
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
    match kind {
        HardwareDeviceKind::UART_16550 => match (
            property::<u32>(node, "reg-shift")?.unwrap_or(0),
            property::<u32>(node, "reg-io-width")?.unwrap_or(1),
            property::<u32>(node, "reg-offset")?.unwrap_or(0),
        ) {
            (0, 1, 0) => {}
            _ => return Err("unsupported UART register layout".into()),
        },
        HardwareDeviceKind::VIRTIO_MMIO => {
            if node.raw_property("iommus")?.is_some() {
                return Err("IOMMU device bindings are not supported".into());
            }
            for bus in walk.ancestors() {
                if bus
                    .raw_property("dma-ranges")?
                    .is_some_and(|property| !property.value.is_empty())
                {
                    return Err("DMA address translation is not supported".into());
                }
            }
        }
        _ => {}
    }
    let location = if kind == HardwareDeviceKind::PCI_HOST_BRIDGE {
        pci::parse(walk)?
    } else {
        HardwareDeviceLocation::Platform
    };
    let mmio_regions = match &location {
        HardwareDeviceLocation::Platform => mmio(walk)?,
        HardwareDeviceLocation::PciHostBridge { .. } => ArrayVec::new(),
    };
    let interrupts = if kind == HardwareDeviceKind::RISCV_PLIC {
        // PLIC 到 CPU 的输出属于 ISA 入口配置，不是设备的输入中断。
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
        dma: virtio.then_some(DmaInfo {
            address_bits: 64,
            isolation: None,
        }),
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
        if !address.start.is_multiple_of(PAGE_SIZE) {
            return Err("device register base is not page-aligned".into());
        }
        let end = address
            .end
            .checked_add(PAGE_SIZE - 1)
            .ok_or("device resource envelope overflow")?
            & !(PAGE_SIZE - 1);
        output
            .try_push(PhysRange {
                start: address.start,
                end,
            })
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
        _ => None,
    }))
}
