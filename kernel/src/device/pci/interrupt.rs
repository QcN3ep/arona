// SPDX-License-Identifier: MPL-2.0

use crate::{HardwareDeviceLocation, HardwareInfo, HardwareInterruptInfo};

use super::{Descriptor, DeviceError, PciHost};

pub(super) fn route(
    host: &PciHost<'_>,
    hardware: &HardwareInfo,
    devices: &[Descriptor],
    mut index: usize,
) -> Result<Option<HardwareInterruptInfo>, DeviceError> {
    let mut pin = (devices[index].pci.as_ref().unwrap().read(0x3c)? >> 8) as u8;
    if pin == 0 {
        return Ok(None);
    }
    if pin > 4 {
        return Err(DeviceError::InvalidArgument);
    }
    while let Some(parent_id) = devices[index]
        .hardware
        .parent
        .filter(|parent| *parent != host.id)
    {
        let parent = devices
            .iter()
            .position(|device| device.hardware.instance_id == parent_id)
            .ok_or(DeviceError::InvalidArgument)?;
        let HardwareDeviceLocation::Pci { slot, .. } = devices[index].hardware.location else {
            unreachable!()
        };
        pin = (pin - 1 + slot) % 4 + 1;
        index = parent;
    }
    let HardwareDeviceLocation::Pci {
        bus,
        slot,
        function,
        ..
    } = devices[index].hardware.location
    else {
        unreachable!()
    };
    let requester = u16::from(bus) << 8 | u16::from(slot) << 3 | u16::from(function);
    let mut routes = hardware.pci.interrupts.iter().filter(|route| {
        route.host == host.id
            && requester & route.requester_mask == route.requester_id & route.requester_mask
            && pin & route.pin_mask == route.pin & route.pin_mask
    });
    let route = routes.next().map(|route| route.interrupt);
    if routes.any(|other| Some(other.interrupt) != route) {
        return Err(DeviceError::InvalidArgument);
    }
    Ok(route)
}
