// SPDX-License-Identifier: MPL-2.0

use crate::{
    hardware::{self, cpu::InterruptGuard},
    ipc::EventReference,
    memory::{self, MemoryError, UserDeviceMapping, VirtRange},
    task::Task,
};
use core::sync::atomic::Ordering;

use super::{DeviceError, DeviceReference, IrqRoute, MappedRegion, registry, same_source};

impl DeviceReference {
    pub fn map(&self, task: &Task, id: u32, region: usize) -> Result<VirtRange, DeviceError> {
        let _interrupts = InterruptGuard::new();
        let space = task.address_space().map_err(|_| DeviceError::Closed)?;
        let registry = registry()?;
        let mut state = registry.state.lock();
        let grant = state.grant(self)?;
        let device = registry.member(grant.group, id)?;
        let physical = *device
            .mmio_regions
            .get(region)
            .ok_or(DeviceError::InvalidArgument)?;
        let envelope = registry.envelope(grant.group, physical)?;
        let grant = state
            .grants
            .iter_mut()
            .find(|grant| grant.token.0.id == self.0.id)
            .unwrap();
        grant
            .mappings
            .try_reserve(1)
            .map_err(|_| MemoryError::NoMemory)?;
        let shared = grant
            .mappings
            .iter()
            .find(|mapped| mapped.mapping.belongs_to(&space) && mapped.mapping.contains(envelope));
        let mapping = if let Some(shared) = shared {
            shared.mapping.clone()
        } else {
            memory::manager().allocate_arc(UserDeviceMapping::new(space, envelope)?)?
        };
        let range = mapping.range(physical);
        grant.mappings.push(MappedRegion { mapping, range });
        Ok(range)
    }

    pub fn unmap(&self, task: &Task, start: usize) -> Result<(), DeviceError> {
        let _interrupts = InterruptGuard::new();
        let space = task.address_space().map_err(|_| DeviceError::Closed)?;
        let mut state = registry()?.state.lock();
        let grant = state
            .grants
            .iter_mut()
            .find(|grant| grant.token.0.id == self.0.id)
            .ok_or(DeviceError::Closed)?;
        let index = grant
            .mappings
            .iter()
            .position(|mapped| mapped.mapping.belongs_to(&space) && mapped.range.start == start)
            .ok_or(DeviceError::InvalidArgument)?;
        grant.mappings.swap_remove(index);
        state.remove_released();
        Ok(())
    }

    pub fn bind(
        &self,
        id: u32,
        index: usize,
        event: EventReference,
        mask: u64,
    ) -> Result<(), DeviceError> {
        if mask == 0 {
            return Err(DeviceError::InvalidArgument);
        }
        let _interrupts = InterruptGuard::new();
        let registry = registry()?;
        let mut state = registry.state.lock();
        let group = state.grant(self)?.group;
        let interrupt = registry.interrupt(group, id, index)?;
        if event.is_closed() {
            return Err(DeviceError::Closed);
        }
        if state
            .routes
            .iter()
            .any(|route| same_source(route.interrupt, interrupt))
        {
            return Err(DeviceError::Busy);
        }
        state
            .routes
            .try_reserve(1)
            .map_err(|_| MemoryError::NoMemory)?;
        registry.pci_interrupts(group, interrupt, true)?;
        if let Err(error) = hardware::interrupt_controller().unmask(interrupt) {
            registry.pci_interrupts(group, interrupt, false)?;
            return Err(error.into());
        }
        state.routes.push(IrqRoute {
            owner: self.0.id,
            group,
            interrupt,
            event,
            mask,
            pending: false,
        });
        Ok(())
    }

    pub fn unbind(&self, id: u32, index: usize) -> Result<(), DeviceError> {
        let _interrupts = InterruptGuard::new();
        let registry = registry()?;
        let mut state = registry.state.lock();
        let interrupt = registry.interrupt(state.grant(self)?.group, id, index)?;
        let position = state
            .routes
            .iter()
            .position(|route| route.owner == self.0.id && same_source(route.interrupt, interrupt))
            .ok_or(DeviceError::InvalidArgument)?;
        registry.pci_interrupts(state.routes[position].group, interrupt, false)?;
        hardware::interrupt_controller().mask(interrupt.into())?;
        state.routes.swap_remove(position);
        Ok(())
    }

    pub fn acknowledge(&self, id: u32, index: usize) -> Result<(), DeviceError> {
        let _interrupts = InterruptGuard::new();
        let registry = registry()?;
        let mut state = registry.state.lock();
        let interrupt = registry.interrupt(state.grant(self)?.group, id, index)?;
        let route = state
            .routes
            .iter_mut()
            .find(|route| route.owner == self.0.id && same_source(route.interrupt, interrupt))
            .ok_or(DeviceError::InvalidArgument)?;
        if !route.pending {
            return Err(DeviceError::Busy);
        }
        if route.event.is_closed() {
            return Err(DeviceError::Closed);
        }
        hardware::interrupt_controller().unmask(interrupt)?;
        route.pending = false;
        Ok(())
    }

    pub fn revoke(&self) -> Result<(), DeviceError> {
        let _interrupts = InterruptGuard::new();
        let registry = registry()?;
        let mut state = registry.state.lock();
        let Some(index) = state
            .grants
            .iter()
            .position(|grant| grant.token.0.id == self.0.id)
        else {
            return Ok(());
        };
        if !self.0.revoked.load(Ordering::Acquire) {
            registry.pci_memory(state.grants[index].group, false)?;
            for route in state.routes.iter().filter(|route| route.owner == self.0.id) {
                hardware::interrupt_controller().mask(route.interrupt.into())?;
            }
            state.routes.retain(|route| route.owner != self.0.id);
            for mapped in &state.grants[index].mappings {
                if let Err(error) = mapped.mapping.revoke() {
                    log::error!("Failed to revoke user device mapping: {error:?}");
                    hardware::cpu().halt();
                }
            }
            self.0.revoked.store(true, Ordering::Release);
        }
        if self.0.credits.load(Ordering::Acquire) == 0 {
            state.grants[index].mappings.clear();
        }
        state.remove_released();
        Ok(())
    }
}
