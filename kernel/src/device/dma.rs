// SPDX-License-Identifier: MPL-2.0

//! Device DMA granting, page frame keep-alive, and start/stop lifecycle.

use alloc::vec::Vec;
use core::ops::Range;

use super::{DeviceError, DeviceReference, DeviceState, Registry, registry};
use crate::{
    hardware::{self, cpu::InterruptGuard, mmu::PAGE_SIZE},
    memory::{
        self, FrameAccess, FrameAccessReference, FrameAllocation, HeapArc, MemoryError, PhysRange,
        ReferenceKind,
    },
};
pub use abi::dma::{DmaDirection, DmaProfile, DmaState};

/// # Safety
///
/// Operations and drop must run on the processor that owns the interrupt controller and must retain kernel mappings.
#[derive(Clone)]
pub struct DmaDomain(HeapArc<DomainToken>);

struct DomainToken {
    grant: DeviceReference,
    id: u64,
}

pub(super) struct Domain {
    id: u64,
    device: u32,
    state: DmaState,
    mappings: Vec<Mapping>,
}

struct Mapping {
    bus: PhysRange,
    _frames: Vec<FrameAccessReference>,
}

impl DeviceReference {
    pub fn create_dma(&self, device: u32) -> Result<DmaDomain, DeviceError> {
        let _interrupts = InterruptGuard::new();
        let registry = registry()?;
        let id = {
            let mut state = registry.state.lock();
            state.grant(self)?;
            let id = state.next_id;
            state.next_id = id.checked_add(1).ok_or(MemoryError::NoMemory)?;
            id
        };
        let token = memory::manager().allocate_arc(DomainToken {
            grant: self.clone(),
            id,
        })?;
        let mut state = registry.state.lock();
        let grant = state.grant(self)?;
        let hardware = registry.member(grant.group, device)?;
        hardware::dma()
            .address_bits(hardware)
            .ok_or(DeviceError::Unsupported)?;
        if grant
            .domains
            .iter()
            .any(|domain| domain.device == device && domain.state != DmaState::Revoked)
        {
            return Err(DeviceError::Busy);
        }
        let grant = state
            .grants
            .iter_mut()
            .find(|grant| grant.token.0.id == self.0.id)
            .unwrap();
        grant
            .domains
            .try_reserve(1)
            .map_err(|_| MemoryError::NoMemory)?;
        grant.domains.push(Domain {
            id,
            device,
            state: DmaState::Quiesced,
            mappings: Vec::new(),
        });
        Ok(DmaDomain(token))
    }
}

impl DmaDomain {
    pub fn state(&self) -> Result<DmaState, DeviceError> {
        let _interrupts = InterruptGuard::new();
        let state = registry()?.state.lock();
        Ok(state
            .grant(&self.0.grant)?
            .domains
            .iter()
            .find(|domain| domain.id == self.0.id)
            .ok_or(DeviceError::Closed)?
            .state)
    }

    pub fn map(
        &self,
        allocation: &FrameAllocation,
        frames: Range<u64>,
        direction: DmaDirection,
    ) -> Result<PhysRange, DeviceError> {
        let _interrupts = InterruptGuard::new();
        let registry = registry()?;
        let mut state = registry.state.lock();
        let domain = self.0.domain(&mut state)?;
        domain.check_live()?;
        let mapping = Mapping::new(registry, domain.device, allocation, frames, direction)?;
        if domain
            .mappings
            .iter()
            .any(|existing| existing.bus.overlaps(mapping.bus))
        {
            return Err(DeviceError::Busy);
        }
        domain
            .mappings
            .try_reserve(1)
            .map_err(|_| MemoryError::NoMemory)?;
        let bus = mapping.bus;
        hardware::dma().synchronize();
        domain.mappings.push(mapping);
        Ok(bus)
    }

    pub fn unmap(&self, address: u64) -> Result<(), DeviceError> {
        let _interrupts = InterruptGuard::new();
        let mut state = registry()?.state.lock();
        let domain = self.0.domain(&mut state)?;
        domain.check_quiesced()?;
        let index = domain
            .mappings
            .iter()
            .position(|mapping| mapping.bus.start == address)
            .ok_or(DeviceError::InvalidArgument)?;
        hardware::dma().synchronize();
        domain.mappings.swap_remove(index);
        Ok(())
    }

    pub fn resume(&self) -> Result<(), DeviceError> {
        let _interrupts = InterruptGuard::new();
        let registry = registry()?;
        let mut state = registry.state.lock();
        let domain = self.0.domain(&mut state)?;
        domain.check_quiesced()?;
        if domain.mappings.is_empty() {
            return Err(DeviceError::InvalidArgument);
        }
        hardware::dma().synchronize();
        domain.state = DmaState::Active;
        let device = domain.device;
        if let Err(error) = set_master(registry, &state, device) {
            poison(registry, &mut state, self.0.id, error);
        }
        Ok(())
    }

    /// # Safety
    ///
    /// Device queues must already be stopped and in-flight DMA completed.
    pub fn quiesce(&self) -> Result<(), DeviceError> {
        let _interrupts = InterruptGuard::new();
        let registry = registry()?;
        let mut state = registry.state.lock();
        let domain = self.0.domain(&mut state)?;
        domain.check_live()?;
        if domain.state != DmaState::Active {
            return Err(DeviceError::Busy);
        }
        domain.state = DmaState::Quiesced;
        let device = domain.device;
        if let Err(error) = set_master(registry, &state, device) {
            poison(registry, &mut state, self.0.id, error);
        }
        hardware::dma().synchronize();
        Ok(())
    }

    pub fn revoke(&self) -> Result<(), DeviceError> {
        let _interrupts = InterruptGuard::new();
        let mut state = registry()?.state.lock();
        let domain = self.0.domain(&mut state)?;
        if domain.state == DmaState::Revoked {
            return Ok(());
        }
        domain.check_quiesced()?;
        domain.state = DmaState::Revoked;
        domain.mappings.clear();
        Ok(())
    }
}

impl Domain {
    fn check_live(&self) -> Result<(), DeviceError> {
        match self.state {
            DmaState::Quiesced | DmaState::Active => Ok(()),
            DmaState::Revoked | DmaState::Poisoned => Err(DeviceError::Closed),
        }
    }

    fn check_quiesced(&self) -> Result<(), DeviceError> {
        self.check_live()?;
        if self.state != DmaState::Quiesced {
            return Err(DeviceError::Busy);
        }
        Ok(())
    }
}

impl DomainToken {
    fn domain<'a>(&self, state: &'a mut DeviceState) -> Result<&'a mut Domain, DeviceError> {
        state.grant(&self.grant)?;
        state
            .grants
            .iter_mut()
            .find(|grant| grant.token.0.id == self.grant.0.id)
            .and_then(|grant| grant.domains.iter_mut().find(|domain| domain.id == self.id))
            .ok_or(DeviceError::Closed)
    }
}

impl Drop for DomainToken {
    fn drop(&mut self) {
        let _interrupts = InterruptGuard::new();
        let Ok(registry) = registry() else {
            return;
        };
        let mut state = registry.state.lock();
        if state
            .grants
            .iter()
            .flat_map(|grant| &grant.domains)
            .any(|domain| domain.id == self.id && domain.state == DmaState::Active)
        {
            poison(registry, &mut state, self.id, DeviceError::Busy);
        }
        if let Some(grant) = state
            .grants
            .iter_mut()
            .find(|grant| grant.token.0.id == self.grant.0.id)
        {
            grant.domains.retain(|domain| domain.id != self.id);
        }
    }
}

impl Mapping {
    fn new(
        registry: &Registry,
        device: u32,
        allocation: &FrameAllocation,
        frames: Range<u64>,
        direction: DmaDirection,
    ) -> Result<Self, DeviceError> {
        if frames.start >= frames.end || frames.end > allocation.info().frames {
            return Err(DeviceError::InvalidArgument);
        }
        let count =
            usize::try_from(frames.end - frames.start).map_err(|_| DeviceError::InvalidArgument)?;
        let skip = usize::try_from(frames.start).map_err(|_| DeviceError::InvalidArgument)?;
        let length = (count as u64)
            .checked_mul(PAGE_SIZE as u64)
            .ok_or(DeviceError::InvalidArgument)?;
        let mut references = Vec::new();
        references
            .try_reserve_exact(count)
            .map_err(|_| MemoryError::NoMemory)?;
        let mut start = None;
        for (index, frame) in allocation.frames().skip(skip).take(count).enumerate() {
            let base = *start.get_or_insert(frame.address());
            if frame.address()
                != base
                    .checked_add(index as u64 * PAGE_SIZE as u64)
                    .ok_or(DeviceError::InvalidArgument)?
            {
                return Err(DeviceError::Unsupported);
            }
            references.push(allocation.access(
                frame,
                ReferenceKind::Dma,
                if direction == DmaDirection::ToDevice {
                    FrameAccess::Read
                } else {
                    FrameAccess::Write
                },
            )?);
        }
        let start = start.ok_or(DeviceError::InvalidArgument)?;
        let physical = PhysRange {
            start,
            end: start
                .checked_add(length)
                .ok_or(DeviceError::InvalidArgument)?,
        };
        let bus = hardware::dma()
            .bus_address(&registry.device(device)?.hardware, physical)
            .ok_or(DeviceError::Unsupported)?;
        Ok(Self {
            bus: PhysRange {
                start: bus,
                end: bus
                    .checked_add(length)
                    .ok_or(DeviceError::InvalidArgument)?,
            },
            _frames: references,
        })
    }
}

pub(super) fn revoke_grant(registry: &Registry, state: &mut DeviceState, index: usize) {
    if let Some(domain) = state.grants[index]
        .domains
        .iter()
        .find(|domain| domain.state == DmaState::Active)
    {
        let id = domain.id;
        poison(registry, state, id, DeviceError::Busy);
    }
    for domain in &mut state.grants[index].domains {
        domain.state = DmaState::Revoked;
        domain.mappings.clear();
    }
}

fn set_master(
    registry: &Registry,
    state: &DeviceState,
    mut device: u32,
) -> Result<(), DeviceError> {
    loop {
        let descriptor = registry.device(device)?;
        if let Some(config) = &descriptor.pci {
            let active = state
                .grants
                .iter()
                .flat_map(|grant| &grant.domains)
                .any(|domain| {
                    domain.state == DmaState::Active && descendant(registry, domain.device, device)
                });
            config.master(active)?;
        }
        let Some(parent) = descriptor.hardware.parent else {
            break;
        };
        device = parent.0;
    }
    Ok(())
}

fn descendant(registry: &Registry, mut device: u32, ancestor: u32) -> bool {
    loop {
        if device == ancestor {
            return true;
        }
        let Ok(descriptor) = registry.device(device) else {
            return false;
        };
        let Some(parent) = descriptor.hardware.parent else {
            return false;
        };
        device = parent.0;
    }
}

fn poison(registry: &Registry, state: &mut DeviceState, id: u64, error: DeviceError) -> ! {
    if let Some(domain) = state
        .grants
        .iter_mut()
        .flat_map(|grant| &mut grant.domains)
        .find(|domain| domain.id == id)
    {
        domain.state = DmaState::Poisoned;
        let device = domain.device;
        let _ = set_master(registry, state, device);
    }
    log::error!("DMA domain poisoned; DMA frames retained: {error:?}");
    hardware::cpu().halt()
}
