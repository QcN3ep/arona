// SPDX-License-Identifier: MPL-2.0

use alloc::vec::Vec;
use arrayvec::ArrayVec;

use crate::{
    DeviceUsePolicy, HardwareDeviceLocation, HardwareInfo, PciBar,
    memory::{MemoryError, PhysRange},
};

use super::{
    Configuration, Descriptor, DeviceError, PciHost,
    aperture::{Assignment, Pool},
};

const BRIDGE_ALIGNMENT: u64 = 1 << 20;
const LIMIT_32: u64 = 1 << 32;

struct Plan {
    parent: Option<usize>,
    header: u8,
    enabled: bool,
    bars: ArrayVec<PciBar, 6>,
    assigned: [Option<Assignment>; 6],
    windows: [Window; 2],
}

#[derive(Clone, Copy)]
struct Window {
    size: u64,
    alignment: u64,
    limit: u64,
    preferred: Option<PhysRange>,
    assigned: Option<Assignment>,
}

impl Window {
    fn empty(limit: u64) -> Self {
        Self {
            size: 0,
            alignment: BRIDGE_ALIGNMENT,
            limit,
            preferred: None,
            assigned: None,
        }
    }
}

#[derive(Clone, Copy)]
enum Target {
    Bar(usize),
    Window(usize),
}

#[derive(Clone, Copy)]
struct Request {
    device: usize,
    target: Target,
    size: u64,
    alignment: u64,
    limit: u64,
    prefetchable: bool,
    preferred: Option<PhysRange>,
}

pub(super) fn configure(
    host: &PciHost<'_>,
    hardware: &HardwareInfo,
    devices: &mut [Descriptor],
) -> Result<(), DeviceError> {
    let mut plans = probe(devices)?;
    for index in (0..plans.len()).rev() {
        if plans[index].header == 1
            && plans[index].enabled
            && let Err(error) = size_bridge(host, devices, &mut plans, index)
        {
            if error != DeviceError::InvalidArgument {
                return Err(error);
            }
            disable_subtree(&mut plans, index);
            log::warn!(
                "PCI bridge memory requirements overflow on {}",
                devices[index].hardware.name
            );
        }
    }
    let mut pool = Pool::root(host, hardware)?;
    allocate_bus(&mut pool, &mut plans, None)?;
    for index in 0..plans.len() {
        if !plans[index].enabled {
            disable_subtree(&mut plans, index);
            continue;
        }
        if plans[index].header == 1 {
            let mut pool = Pool::bridge(plans[index].windows.map(|window| window.assigned))?;
            allocate_bus(&mut pool, &mut plans, Some(index))?;
        }
    }
    for index in 0..plans.len() {
        if !plans[index].enabled {
            continue;
        }
        if let Err(error) = program(devices[index].pci.as_ref().unwrap(), &plans[index]) {
            log::warn!(
                "Failed to configure {}: {error:?}",
                devices[index].hardware.name
            );
            disable_subtree(&mut plans, index);
            for child in (index..plans.len()).rev() {
                if descendant(&plans, child, index) {
                    devices[child]
                        .pci
                        .as_ref()
                        .unwrap()
                        .rollback_resources(plans[child].header)?;
                }
            }
        }
    }
    for index in 0..plans.len() {
        if !plans[index].enabled {
            continue;
        }
        publish(host, hardware, devices, &plans, index)?;
    }
    Ok(())
}

fn probe(devices: &[Descriptor]) -> Result<Vec<Plan>, DeviceError> {
    let mut plans = Vec::new();
    plans
        .try_reserve_exact(devices.len())
        .map_err(|_| MemoryError::NoMemory)?;
    for (index, device) in devices.iter().enumerate() {
        let config = device.pci.as_ref().unwrap();
        let header = (config.original(0x0c) >> 16) as u8 & 0x7f;
        let parent = device.hardware.parent.and_then(|parent| {
            devices[..index]
                .iter()
                .position(|device| device.hardware.instance_id == parent)
        });
        let mut plan = Plan {
            parent,
            header,
            enabled: header <= 1,
            bars: ArrayVec::new(),
            assigned: [None; 6],
            windows: [
                Window::empty(LIMIT_32),
                Window::empty(if config.original(0x24) & 0x0f == 1 {
                    u64::MAX
                } else {
                    LIMIT_32
                }),
            ],
        };
        if plan.enabled {
            match config.probe_bars(header) {
                Ok(bars) => plan.bars = bars,
                Err(DeviceError::InvalidArgument) => {
                    log::warn!("Ignoring invalid PCI BARs on {}", device.hardware.name);
                    plan.enabled = false;
                }
                Err(error) => return Err(error),
            }
        }
        plans.push(plan);
    }
    Ok(plans)
}

fn requests(plans: &[Plan], parent: Option<usize>) -> Result<Vec<Request>, DeviceError> {
    let mut requests = Vec::new();
    for (index, plan) in plans
        .iter()
        .enumerate()
        .filter(|(_, plan)| plan.enabled && plan.parent == parent)
    {
        requests
            .try_reserve(plan.bars.len() + 2)
            .map_err(|_| MemoryError::NoMemory)?;
        for (bar_index, bar) in plan.bars.iter().enumerate() {
            let limit = if bar.width == 32 { LIMIT_32 } else { u64::MAX };
            requests.push(Request {
                device: index,
                target: Target::Bar(bar_index),
                size: bar.size,
                alignment: bar.size,
                limit,
                prefetchable: bar.prefetchable,
                preferred: bar
                    .bus_address
                    .checked_add(bar.size)
                    .filter(|end| {
                        bar.bus_address != 0
                            && bar.bus_address.is_multiple_of(bar.size)
                            && *end <= limit
                    })
                    .map(|end| PhysRange {
                        start: bar.bus_address,
                        end,
                    }),
            });
        }
        if plan.header == 1 {
            for (kind, window) in plan
                .windows
                .iter()
                .enumerate()
                .filter(|(_, window)| window.size != 0)
            {
                requests.push(Request {
                    device: index,
                    target: Target::Window(kind),
                    size: window.size,
                    alignment: window.alignment,
                    limit: window.limit,
                    prefetchable: kind == 1,
                    preferred: window.preferred,
                });
            }
        }
    }
    Ok(requests)
}

fn size_bridge(
    host: &PciHost<'_>,
    devices: &[Descriptor],
    plans: &mut [Plan],
    index: usize,
) -> Result<(), DeviceError> {
    let mut requests = requests(plans, Some(index))?;
    requests.sort_unstable_by_key(|request| core::cmp::Reverse(request.alignment));
    for kind in 0..2 {
        let window = &mut plans[index].windows[kind];
        let decode_limit = window.limit;
        let mut cursor = 0;
        let mut preferred: Option<PhysRange> = None;
        for request in requests
            .iter()
            .filter(|request| request.prefetchable == (kind == 1))
        {
            cursor = align(cursor, request.alignment)?
                .checked_add(request.size)
                .ok_or(DeviceError::InvalidArgument)?;
            window.alignment = window.alignment.max(request.alignment);
            window.limit = window.limit.min(request.limit.max(LIMIT_32));
            if let Some(range) = request
                .preferred
                .filter(|range| host_range(host, *range).is_some())
            {
                preferred = Some(preferred.map_or(range, |other| PhysRange {
                    start: other.start.min(range.start),
                    end: other.end.max(range.end),
                }));
            }
        }
        window.size = align(cursor, BRIDGE_ALIGNMENT)?;
        let config = devices[index].pci.as_ref().unwrap();
        if let Some(original) = bridge_window(config, kind).filter(|range| {
            host_range(host, *range).is_some()
                && range.end <= decode_limit
                && range.end - range.start >= window.size
        }) {
            window.size = original.end - original.start;
            window.preferred = Some(original);
        } else if let Some(range) = preferred {
            let start = range.start & !(window.alignment - 1);
            let end = align(range.end, BRIDGE_ALIGNMENT)?;
            window.size = window.size.max(end - start);
            window.preferred = start
                .checked_add(window.size)
                .filter(|end| *end <= window.limit)
                .map(|end| PhysRange { start, end });
        }
    }
    Ok(())
}

fn bridge_window(config: &Configuration, kind: usize) -> Option<PhysRange> {
    let value = config.original(if kind == 0 { 0x20 } else { 0x24 });
    let mut start = u64::from(value & 0xfff0) << 16;
    let mut end = u64::from((value >> 16) & 0xfff0) << 16 | 0xfffff;
    if kind == 1 && value & 0xf == 1 && (value >> 16) & 0xf == 1 {
        start |= u64::from(config.original(0x28)) << 32;
        end |= u64::from(config.original(0x2c)) << 32;
    } else if value & 0x000f_000f != 0 {
        return None;
    }
    (start != 0 && start <= end).then_some(PhysRange {
        start,
        end: end.checked_add(1)?,
    })
}

fn allocate_bus(
    pool: &mut Pool,
    plans: &mut [Plan],
    parent: Option<usize>,
) -> Result<(), DeviceError> {
    let mut requests = requests(plans, parent)?;
    for request in &requests {
        if let Some(range) = request.preferred
            && let Some(assigned) = pool.claim(range, request.prefetchable)?
        {
            assign(plans, *request, assigned);
        }
    }
    requests.sort_unstable_by_key(|request| core::cmp::Reverse((request.alignment, request.size)));
    for request in requests {
        if !plans[request.device].enabled || assigned(plans, request).is_some() {
            continue;
        }
        if let Some(assignment) = pool.allocate(
            request.size,
            request.alignment,
            request.limit,
            request.prefetchable,
        )? {
            assign(plans, request, assignment);
        } else {
            log::warn!(
                "PCI memory aperture exhausted for device {}",
                request.device
            );
            disable_subtree(plans, request.device);
        }
    }
    Ok(())
}

fn assign(plans: &mut [Plan], request: Request, assignment: Assignment) {
    match request.target {
        Target::Bar(index) => plans[request.device].assigned[index] = Some(assignment),
        Target::Window(kind) => plans[request.device].windows[kind].assigned = Some(assignment),
    }
}

fn assigned(plans: &[Plan], request: Request) -> Option<Assignment> {
    match request.target {
        Target::Bar(index) => plans[request.device].assigned[index],
        Target::Window(kind) => plans[request.device].windows[kind].assigned,
    }
}

fn program(config: &Configuration, plan: &Plan) -> Result<(), DeviceError> {
    for (index, bar) in plan.bars.iter().enumerate() {
        let assignment = plan.assigned[index].ok_or(DeviceError::InvalidArgument)?;
        let offset = 0x10 + usize::from(bar.index) * 4;
        let value = (assignment.range.start as u32 & !0xf) | (config.original(offset) & 0xf);
        config.write(offset, value)?;
        if bar.width == 64 {
            config.write(offset + 4, (assignment.range.start >> 32) as u32)?;
        }
        if config.read(offset)? != value
            || bar.width == 64 && config.read(offset + 4)? != (assignment.range.start >> 32) as u32
        {
            return Err(DeviceError::InvalidArgument);
        }
    }
    if plan.header == 1 {
        for (kind, window) in plan.windows.iter().enumerate() {
            let range = window.assigned.map(|assignment| assignment.range);
            let (start, end) = range.map_or((0xfff0_0000, 0), |range| (range.start, range.end - 1));
            let offset = if kind == 0 { 0x20 } else { 0x24 };
            let value = ((start >> 16) as u32 & 0xfff0)
                | ((end as u32) & 0xfff0_0000)
                | (config.original(offset) & 0x000f_000f);
            config.write(offset, value)?;
            if config.read(offset)? != value {
                return Err(DeviceError::InvalidArgument);
            }
            if kind == 1 && config.original(0x24) & 0xf == 1 {
                config.write(0x28, (start >> 32) as u32)?;
                config.write(0x2c, (end >> 32) as u32)?;
                if config.read(0x28)? != (start >> 32) as u32
                    || config.read(0x2c)? != (end >> 32) as u32
                {
                    return Err(DeviceError::InvalidArgument);
                }
            }
        }
    }
    config.command(
        !plan.bars.is_empty() || plan.windows.iter().any(|window| window.size != 0),
        false,
    )
}

fn publish(
    host: &PciHost<'_>,
    hardware: &HardwareInfo,
    devices: &mut [Descriptor],
    plans: &[Plan],
    index: usize,
) -> Result<(), DeviceError> {
    let interrupt = match super::interrupt::route(host, hardware, devices, index) {
        Ok(interrupt) => interrupt,
        Err(DeviceError::InvalidArgument) => {
            log::warn!(
                "Ignoring invalid PCI INTx route on {}",
                devices[index].hardware.name
            );
            None
        }
        Err(error) => return Err(error),
    };
    let device = &mut devices[index];
    let config = device.pci.as_mut().unwrap();
    let HardwareDeviceLocation::Pci { bars, .. } = &mut device.hardware.location else {
        unreachable!()
    };
    for (bar_index, original) in plans[index].bars.iter().enumerate() {
        let assigned = plans[index].assigned[bar_index].unwrap();
        let window = host.windows[assigned.host_window];
        let start = window.cpu_range.start + (assigned.range.start - window.bus_address);
        let physical = PhysRange {
            start,
            end: start + original.size,
        };
        let mut bar = *original;
        bar.bus_address = assigned.range.start;
        if physical.start.is_multiple_of(4096) && physical.end.is_multiple_of(4096) {
            bar.region = Some(device.hardware.mmio_regions.len() as u8);
            device.hardware.mmio_regions.push(physical);
        } else {
            config.unmapped_bars.push(physical);
        }
        bars.push(bar);
        config.bars[usize::from(bar.index)] = Some(bar.encode());
    }
    if plans[index].header == 0 && !device.hardware.mmio_regions.is_empty() {
        device.hardware.use_policy = DeviceUsePolicy::UserAssignable;
        if let Some(interrupt) = interrupt {
            device.hardware.interrupts.push(interrupt);
        }
    }
    Ok(())
}

fn descendant(plans: &[Plan], mut child: usize, parent: usize) -> bool {
    loop {
        if child == parent {
            return true;
        }
        let Some(next) = plans[child].parent else {
            return false;
        };
        child = next;
    }
}

fn disable_subtree(plans: &mut [Plan], index: usize) {
    for child in index..plans.len() {
        if descendant(plans, child, index) {
            plans[child].enabled = false;
        }
    }
}

fn host_range(host: &PciHost<'_>, range: PhysRange) -> Option<usize> {
    host.windows.iter().position(|window| {
        window.bus_address <= range.start
            && range
                .end
                .checked_sub(window.bus_address)
                .is_some_and(|end| end <= window.cpu_range.end - window.cpu_range.start)
    })
}

fn align(value: u64, alignment: u64) -> Result<u64, DeviceError> {
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
        .ok_or(DeviceError::InvalidArgument)
}
