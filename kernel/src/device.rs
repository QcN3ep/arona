// SPDX-License-Identifier: MPL-2.0

//! 设备资源授权、用户映射与中断投递。

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::{Mutex, Once};

use crate::{
    DeviceProperty, DeviceUsePolicy, DmaCapability, HardwareDeviceInfo, HardwareDeviceInstanceId,
    HardwareDeviceLocation, HardwareInfo, HardwareInterruptInfo,
    hardware::{
        self,
        cpu::InterruptGuard,
        interrupt::{InterruptError, InterruptSource},
    },
    ipc::EventReference,
    memory::{self, AddressSpace, HeapArc, MemoryError, PhysRange, UserDeviceMapping, VirtRange},
    task::Task,
};
use abi::device::{DeviceField, DeviceInfo};

mod pci;

static REGISTRY: Once<Registry> = Once::new();
static GRANT_TOKENS: AtomicUsize = AtomicUsize::new(0);

pub fn active_grant_tokens() -> usize {
    GRANT_TOKENS.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceError {
    NotInitialized,
    AlreadyInitialized,
    InvalidArgument,
    Denied,
    Busy,
    Closed,
    Memory(MemoryError),
    Interrupt(InterruptError),
    Pci(hardware::pci::PciConfigError),
}

impl From<MemoryError> for DeviceError {
    fn from(error: MemoryError) -> Self {
        Self::Memory(error)
    }
}
impl From<InterruptError> for DeviceError {
    fn from(error: InterruptError) -> Self {
        Self::Interrupt(error)
    }
}

/// # Safety
///
/// 操作须在中断控制器所属处理器执行并保留内核映射。
#[derive(Clone)]
pub struct DeviceSet(());

/// # Safety
///
/// 操作及析构须在中断控制器所属处理器执行并保留内核映射。
pub struct Device {
    reference: DeviceReference,
}

/// 不计入开放句柄，最后一个 Device 析构时撤销授权。
#[derive(Clone)]
pub struct DeviceReference(HeapArc<GrantToken>);

struct GrantToken {
    id: u64,
    credits: AtomicUsize,
    revoked: AtomicBool,
}

struct Descriptor {
    hardware: HardwareDeviceInfo,
    group: Option<u32>,
}

struct Registry {
    devices: Vec<Descriptor>,
    properties: Vec<DeviceProperty>,
    console: Option<HardwareDeviceInstanceId>,
    state: Mutex<DeviceState>,
}

struct DeviceState {
    grants: Vec<Grant>,
    routes: Vec<IrqRoute>,
    next_id: u64,
}

struct Grant {
    token: DeviceReference,
    group: u32,
    mappings: Vec<MappedRegion>,
}

struct MappedRegion {
    mapping: HeapArc<UserDeviceMapping>,
    range: VirtRange,
}

struct IrqRoute {
    owner: u64,
    interrupt: HardwareInterruptInfo,
    event: EventReference,
    mask: u64,
    pending: bool,
}

impl DeviceSet {
    /// # Safety
    ///
    /// 初始化及设备操作须在中断控制器所属处理器执行并保留内核映射。
    pub fn initialize(hardware: &HardwareInfo) -> Result<Self, DeviceError> {
        let _interrupts = InterruptGuard::new();
        let mut initialized = false;
        REGISTRY.try_call_once(|| {
            let registry = Registry::new(hardware)?;
            initialized = true;
            Ok::<_, DeviceError>(registry)
        })?;
        if initialized {
            Ok(Self(()))
        } else {
            Err(DeviceError::AlreadyInitialized)
        }
    }

    pub fn next(&self, after: u32) -> Result<Option<u32>, DeviceError> {
        let registry = registry()?;
        Ok(registry
            .devices
            .iter()
            .map(|device| device.hardware.instance_id.0)
            .filter(|id| *id > after)
            .min())
    }

    pub fn describe(&self, id: u32) -> Result<DeviceInfo, DeviceError> {
        let registry = registry()?;
        let device = registry.device(id)?;
        let hardware = &device.hardware;
        Ok(DeviceInfo {
            id,
            parent: hardware.parent.map(|id| id.0),
            group: device.group,
            mmio_regions: hardware.mmio_regions.len(),
            interrupts: hardware.interrupts.len(),
            identities: hardware.identities.len(),
            properties: registry
                .properties
                .iter()
                .filter(|property| property.device == hardware.instance_id)
                .count(),
            console: registry.console == Some(hardware.instance_id),
        })
    }

    pub fn read(
        &self,
        id: u32,
        field: DeviceField,
        index: usize,
    ) -> Result<&'static [u8], DeviceError> {
        let registry = registry()?;
        let descriptor = registry.device(id)?;
        let device = &descriptor.hardware;
        let bytes = match field {
            DeviceField::Name if index == 0 => device.name.as_bytes(),
            DeviceField::IdentityNamespace | DeviceField::Identity => {
                let identity = device
                    .identities
                    .get(index)
                    .ok_or(DeviceError::InvalidArgument)?;
                if field == DeviceField::IdentityNamespace {
                    identity.namespace.as_bytes()
                } else {
                    identity.value.as_bytes()
                }
            }
            DeviceField::PropertyNamespace
            | DeviceField::PropertyName
            | DeviceField::PropertyValue => {
                let property = registry
                    .properties
                    .iter()
                    .filter(|property| property.device.0 == id)
                    .nth(index)
                    .ok_or(DeviceError::InvalidArgument)?;
                match field {
                    DeviceField::PropertyNamespace => property.namespace.as_bytes(),
                    DeviceField::PropertyName => property.name.as_bytes(),
                    _ => property.value.as_slice(),
                }
            }
            _ => return Err(DeviceError::InvalidArgument),
        };
        Ok(bytes)
    }

    pub fn claim_group(&self, group: u32) -> Result<Device, DeviceError> {
        let _interrupts = InterruptGuard::new();
        let registry = registry()?;
        if !registry
            .devices
            .iter()
            .any(|device| device.group == Some(group))
        {
            return Err(DeviceError::Denied);
        }
        let mut state = registry.state.lock();
        if state
            .grants
            .iter()
            .any(|grant| grant.group == group && !grant.token.0.revoked.load(Ordering::Acquire))
        {
            return Err(DeviceError::Busy);
        }
        state
            .grants
            .try_reserve(1)
            .map_err(|_| MemoryError::NoMemory)?;
        let id = state.next_id;
        state.next_id = id.checked_add(1).ok_or(MemoryError::NoMemory)?;
        let reference = DeviceReference(memory::manager().allocate_arc(GrantToken {
            id,
            credits: AtomicUsize::new(1),
            revoked: AtomicBool::new(false),
        })?);
        GRANT_TOKENS.fetch_add(1, Ordering::Relaxed);
        state.grants.push(Grant {
            token: reference.clone(),
            group,
            mappings: Vec::new(),
        });
        Ok(Device { reference })
    }
}

impl Drop for GrantToken {
    fn drop(&mut self) {
        GRANT_TOKENS.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Device {
    pub fn reference(&self) -> DeviceReference {
        self.reference.clone()
    }

    pub fn try_clone(&self) -> Result<Self, DeviceError> {
        if self.reference.0.revoked.load(Ordering::Acquire) {
            return Err(DeviceError::Closed);
        }
        self.reference
            .0
            .credits
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |credits| {
                credits.checked_add(1)
            })
            .map_err(|_| MemoryError::ReferenceOverflow)?;
        Ok(Self {
            reference: self.reference(),
        })
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        if self.reference.0.credits.fetch_sub(1, Ordering::AcqRel) == 1
            && let Err(error) = self.reference.revoke()
        {
            log::error!("Failed to release device grant: {error:?}");
            hardware::cpu().halt();
        }
    }
}

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
        let interrupt = registry.interrupt(state.grant(self)?.group, id, index)?;
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
        hardware::interrupt_controller().unmask(interrupt)?;
        state.routes.push(IrqRoute {
            owner: self.0.id,
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
        let mut state = registry()?.state.lock();
        let Some(index) = state
            .grants
            .iter()
            .position(|grant| grant.token.0.id == self.0.id)
        else {
            return Ok(());
        };
        for route in state.routes.iter().filter(|route| route.owner == self.0.id) {
            hardware::interrupt_controller().mask(route.interrupt.into())?;
        }
        state.routes.retain(|route| route.owner != self.0.id);
        let grant = &mut state.grants[index];
        for mapped in &grant.mappings {
            if let Err(error) = mapped.mapping.revoke() {
                log::error!("Failed to revoke user device mapping: {error:?}");
                hardware::cpu().halt();
            }
        }
        self.0.revoked.store(true, Ordering::Release);
        if self.0.credits.load(Ordering::Acquire) == 0 {
            grant.mappings.clear();
        }
        state.remove_released();
        Ok(())
    }
}

impl Registry {
    fn new(hardware: &HardwareInfo) -> Result<Self, DeviceError> {
        let mut devices = Vec::new();
        devices
            .try_reserve_exact(hardware.devices.len())
            .map_err(|_| MemoryError::NoMemory)?;
        for device in &hardware.devices {
            devices.push(Descriptor {
                hardware: device.clone(),
                group: Some(device.instance_id.0),
            });
        }
        // 共享 MMIO 页或 IRQ 的设备属于同一授权组。
        for left in 0..devices.len() {
            for right in 0..left {
                if shares_resources(&devices[left].hardware, &devices[right].hardware)? {
                    let from = devices[left].group;
                    let to = devices[right].group;
                    for device in &mut devices {
                        if device.group == from {
                            device.group = to;
                        }
                    }
                }
            }
        }
        for index in 0..devices.len() {
            let device = &devices[index].hardware;
            // TODO: 验证并授权 PCI BAR 和中断资源。
            // TODO: 接入 IOMMU 域及 DMA 映射、同步和撤销后授权 DMA 设备。
            if device.use_policy != DeviceUsePolicy::UserAssignable
                || device.dma != DmaCapability::None
                || device.location != HardwareDeviceLocation::Platform
            {
                let group = devices[index].group;
                if group.is_some() {
                    for device in &mut devices {
                        if device.group == group {
                            device.group = None;
                        }
                    }
                }
            }
        }
        let first_pci_id = devices
            .iter()
            .map(|device| device.hardware.instance_id.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(DeviceError::InvalidArgument)?;
        let mut properties = Vec::new();
        properties
            .try_reserve_exact(hardware.device_properties.len())
            .map_err(|_| MemoryError::NoMemory)?;
        properties.extend(hardware.device_properties.iter().cloned());
        let pci_devices = pci::enumerate(hardware, first_pci_id, &mut properties)?;
        log::info!("Enumerated {} PCI devices", pci_devices.len());
        devices
            .try_reserve(pci_devices.len())
            .map_err(|_| MemoryError::NoMemory)?;
        devices.extend(pci_devices.into_iter().map(|device| Descriptor {
            hardware: device,
            group: None,
        }));
        Ok(Self {
            devices,
            properties,
            console: hardware.console.as_ref().map(|console| console.device),
            state: Mutex::new(DeviceState {
                grants: Vec::new(),
                routes: Vec::new(),
                next_id: 1,
            }),
        })
    }

    fn device(&self, id: u32) -> Result<&Descriptor, DeviceError> {
        self.devices
            .iter()
            .find(|device| device.hardware.instance_id.0 == id)
            .ok_or(DeviceError::InvalidArgument)
    }

    fn member(&self, group: u32, id: u32) -> Result<&HardwareDeviceInfo, DeviceError> {
        let device = self.device(id)?;
        if device.group != Some(group) {
            return Err(DeviceError::Denied);
        }
        Ok(&device.hardware)
    }

    fn interrupt(
        &self,
        group: u32,
        id: u32,
        index: usize,
    ) -> Result<HardwareInterruptInfo, DeviceError> {
        self.member(group, id)?
            .interrupts
            .get(index)
            .copied()
            .ok_or(DeviceError::InvalidArgument)
    }

    fn envelope(&self, group: u32, physical: PhysRange) -> Result<PhysRange, DeviceError> {
        let mut envelope = physical
            .page_envelope()
            .map_err(|_| DeviceError::InvalidArgument)?;
        loop {
            let previous = envelope;
            for device in self
                .devices
                .iter()
                .filter(|device| device.group == Some(group))
            {
                for range in &device.hardware.mmio_regions {
                    let pages = range
                        .page_envelope()
                        .map_err(|_| DeviceError::InvalidArgument)?;
                    if envelope.overlaps(pages) {
                        envelope.start = envelope.start.min(pages.start);
                        envelope.end = envelope.end.max(pages.end);
                    }
                }
            }
            if envelope == previous {
                return Ok(envelope);
            }
        }
    }
}

impl DeviceState {
    fn grant(&self, reference: &DeviceReference) -> Result<&Grant, DeviceError> {
        if reference.0.revoked.load(Ordering::Acquire) {
            return Err(DeviceError::Closed);
        }
        self.grants
            .iter()
            .find(|grant| grant.token.0.id == reference.0.id)
            .ok_or(DeviceError::Closed)
    }

    fn remove_released(&mut self) {
        self.grants.retain(|grant| {
            !grant.token.0.revoked.load(Ordering::Acquire) || !grant.mappings.is_empty()
        });
    }
}

pub(crate) fn dispatch_interrupt() -> Result<(), DeviceError> {
    let _interrupts = InterruptGuard::new();
    let Some(claim) = hardware::interrupt_controller().claim()? else {
        return Ok(());
    };
    let notification = if let Some(registry) = REGISTRY.get() {
        let mut state = registry.state.lock();
        if let Some(route) = state
            .routes
            .iter_mut()
            .find(|route| InterruptSource::from(route.interrupt) == claim.source())
        {
            hardware::interrupt_controller().mask(claim.source())?;
            claim.complete()?;
            route.pending = true;
            Some((route.event.clone(), route.mask))
        } else {
            None
        }
    } else {
        None
    };
    if let Some((event, mask)) = notification {
        let _ = event.signal(mask);
    }
    Ok(())
}

pub(crate) fn release_space(space: &HeapArc<AddressSpace>) {
    let _interrupts = InterruptGuard::new();
    if let Some(registry) = REGISTRY.get() {
        let mut state = registry.state.lock();
        for grant in &mut state.grants {
            grant
                .mappings
                .retain(|mapped| !mapped.mapping.belongs_to(space));
        }
        state.remove_released();
    }
}

pub(crate) fn close_event(event: &EventReference) {
    let _interrupts = InterruptGuard::new();
    if let Some(registry) = REGISTRY.get() {
        let mut state = registry.state.lock();
        for route in state.routes.iter().filter(|route| route.event.same(event)) {
            if let Err(error) = hardware::interrupt_controller().mask(route.interrupt.into()) {
                log::error!("Failed to detach device interrupt: {error:?}");
                hardware::cpu().halt();
            }
        }
        state.routes.retain(|route| !route.event.same(event));
    }
}

fn registry() -> Result<&'static Registry, DeviceError> {
    REGISTRY.get().ok_or(DeviceError::NotInitialized)
}

fn same_source(left: HardwareInterruptInfo, right: HardwareInterruptInfo) -> bool {
    InterruptSource::from(left) == InterruptSource::from(right)
}

fn shares_resources(
    left: &HardwareDeviceInfo,
    right: &HardwareDeviceInfo,
) -> Result<bool, DeviceError> {
    for a in resource_ranges(left) {
        for b in resource_ranges(right) {
            if a.page_envelope()
                .map_err(|_| DeviceError::InvalidArgument)?
                .overlaps(
                    b.page_envelope()
                        .map_err(|_| DeviceError::InvalidArgument)?,
                )
            {
                return Ok(true);
            }
        }
    }
    Ok(left
        .interrupts
        .iter()
        .any(|a| right.interrupts.iter().any(|b| same_source(*a, *b))))
}

fn resource_ranges(device: &HardwareDeviceInfo) -> impl Iterator<Item = PhysRange> + '_ {
    let (ecam, windows): (_, &[_]) = match &device.location {
        HardwareDeviceLocation::PciHostBridge { ecam, windows, .. } => {
            (Some(*ecam), windows.as_slice())
        }
        _ => (None, &[]),
    };
    device
        .mmio_regions
        .iter()
        .copied()
        .chain(ecam)
        .chain(windows.iter().map(|window| window.cpu_range))
}
