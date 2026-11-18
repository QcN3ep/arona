// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! System V shared memory: `shmget`, `shmat`, `shmdt`, and `shmctl`.

use super::abi::{
    EACCES, EEXIST, EFAULT, EINVAL, EIO, ENOENT, ENOMEM, EPERM, Errno, SyscallResult,
};
use crate::{
    elf::PAGE_SIZE,
    memory::PageBacking,
    process::Process,
    shm::{Attach, MAX_SEGMENTS, MAX_SIZE, Segment},
};
use alloc::rc::Rc;
use runtime::{FrameAllocation, Handle, Rights, memory::Permissions};

// REFERENCE: https://man7.org/linux/man-pages/man2/shmget.2.html
const IPC_CREAT: u32 = 0x200;
const IPC_EXCL: u32 = 0x400;
// REFERENCE: https://man7.org/linux/man-pages/man2/shmctl.2.html
const IPC_RMID: i32 = 0;
const IPC_SET: i32 = 1;
const IPC_STAT: i32 = 2;
const IPC_INFO: i32 = 3;
const SHM_INFO: i32 = 14;
// REFERENCE: https://man7.org/linux/man-pages/man2/shmat.2.html
const SHM_RDONLY: u32 = 0x1000;
const SHM_RND: u32 = 0x2000;

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/uapi/linux/shm.h
// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/include/uapi/linux/ipc.h
/// `struct shmid64_ds` on 64-bit Linux.
const STAT_SIZE: usize = 112;
/// `struct ipc64_perm` prefix accepted by `IPC_SET`.
const SET_SIZE: usize = 48;
/// `struct shminfo`; `shmall` is expressed in pages.
const INFO_SIZE: usize = 72;
/// `struct shm_info`; segment and resident sizes are measured in pages.
const SHM_INFO_SIZE: usize = 48;

pub(super) fn shmget(process: &Process<'_>, arguments: [usize; 6]) -> SyscallResult {
    let key = arguments[0] as u32 as i32;
    let size = arguments[1];
    let flags = arguments[2] as u32;
    let mode = (flags & 0o777) as u16;
    let create = flags & IPC_CREAT != 0;
    let exclusive = flags & IPC_EXCL != 0;
    let uid = process.credentials.euid;
    let gid = process.credentials.egid;
    let shm = Rc::clone(&process.shm);
    let mut table = shm.borrow_mut();
    if key != 0
        && let Some(segment) = table.by_key(key)
    {
        if create && exclusive {
            return Err(EEXIST);
        }
        if size > segment.size {
            return Err(EINVAL);
        }
        if !segment.permits(uid, gid, (flags & 0o777) as u16) {
            return Err(EACCES);
        }
        return Ok(segment.id);
    }
    if !create && key != 0 {
        return Err(ENOENT);
    }
    if size == 0 || size > MAX_SIZE {
        return Err(EINVAL);
    }
    let length = size.checked_next_multiple_of(PAGE_SIZE).ok_or(EINVAL)?;
    if length > MAX_SIZE {
        return Err(EINVAL);
    }
    if table.len() >= MAX_SEGMENTS {
        return Err(ENOMEM);
    }
    let id = table.allocate_id().ok_or(ENOMEM)?;
    let now = now()?;
    table.insert(Segment {
        id,
        key: (key != 0).then_some(key),
        size,
        mapped_length: length,
        mode,
        uid,
        gid,
        cuid: uid,
        cgid: gid,
        cpid: process.pid,
        lpid: process.pid,
        nattch: 0,
        atime: now,
        dtime: now,
        ctime: now,
        removed: false,
        frames: None,
    });
    Ok(id)
}

pub(super) fn shmat(process: &mut Process<'_>, arguments: [usize; 6]) -> SyscallResult {
    let id = arguments[0] as u32 as usize;
    let address = arguments[1];
    let flags = arguments[2] as u32;
    if flags & !(SHM_RDONLY | SHM_RND) != 0 {
        return Err(EINVAL);
    }
    let readonly = flags & SHM_RDONLY != 0;
    let address = if address == 0 {
        None
    } else if flags & SHM_RND != 0 {
        Some(address & !(PAGE_SIZE - 1))
    } else if address.is_multiple_of(PAGE_SIZE) {
        Some(address)
    } else {
        return Err(EINVAL);
    };
    let uid = process.credentials.euid;
    let gid = process.credentials.egid;
    let shm = Rc::clone(&process.shm);
    let mut table = shm.borrow_mut();
    let Some(segment) = table.get_mut(id) else {
        return Err(EINVAL);
    };
    let requested = if readonly { 0o444 } else { 0o666 };
    if !segment.permits(uid, gid, requested) {
        return Err(EACCES);
    }
    let length = segment.mapped_length;
    let count = (length / PAGE_SIZE) as u64;
    let frames = attach_frames(process, segment)?;
    let permissions = if readonly {
        Permissions::ReadOnly
    } else {
        Permissions::ReadWrite
    };
    let address = attach_address(process, address, length)?;
    // The handle rights cap the mapping's maximum permissions, so a read-only
    // attachment cannot become writable through mprotect.
    let map_rights = if readonly {
        Rights::READ | Rights::MAP
    } else {
        Rights::READ_WRITE | Rights::MAP
    };
    let mapped = frames.try_clone(map_rights).map_err(mapping_error)?;
    process
        .address_space
        .map_frames(
            Rc::new(mapped),
            0,
            count,
            address,
            permissions,
            PageBacking::Shared,
            None,
            0,
        )
        .map_err(mapping_error)?;
    let now = now().unwrap_or(0);
    // A successful mapping proves the range was free; a surviving record for
    // this address belongs to a mapping the guest removed with munmap.
    if let Some(stale) = process.attaches.remove(&address)
        && stale.counted
    {
        table.detach(stale.id, process.pid, now);
    }
    table.attach(id, process.pid, now);
    drop(table);
    process.attaches.insert(
        address,
        Attach {
            id,
            mapped_length: length,
            counted: true,
        },
    );
    Ok(address)
}

fn attach_frames(
    process: &Process<'_>,
    segment: &mut Segment,
) -> Result<Rc<Handle<FrameAllocation>>, Errno> {
    if let Some(frames) = &segment.frames {
        return Ok(Rc::clone(frames));
    }
    let count = (segment.mapped_length / PAGE_SIZE) as u64;
    let frames = Rc::new(
        process
            .address_space
            .native
            .task()
            .allocate(count, Rights::DUPLICATE | Rights::READ_WRITE | Rights::MAP)
            .map_err(mapping_error)?,
    );
    segment.frames = Some(Rc::clone(&frames));
    Ok(frames)
}

fn attach_address(
    process: &Process<'_>,
    address: Option<usize>,
    length: usize,
) -> Result<usize, Errno> {
    let Some(address) = address else {
        let mut reservation = process
            .address_space
            .native
            .task()
            .reserve(length, PAGE_SIZE, None)
            .map_err(mapping_error)?;
        let range = reservation.range().ok_or(ENOMEM)?;
        reservation.release().map_err(mapping_error)?;
        return Ok(range.start);
    };
    Ok(address)
}

pub(super) fn dispatch(
    process: &mut Process<'_>,
    number: usize,
    arguments: [usize; 6],
) -> SyscallResult {
    match number {
        super::abi::SHMGET => shmget(process, arguments),
        super::abi::SHMAT => shmat(process, arguments),
        super::abi::SHMDT => shmdt(process, arguments[0]),
        super::abi::SHMCTL => shmctl(process, arguments),
        _ => Err(EINVAL),
    }
}

pub(super) fn shmdt(process: &mut Process<'_>, address: usize) -> SyscallResult {
    if !address.is_multiple_of(PAGE_SIZE) {
        return Err(EINVAL);
    }
    let Some(attach) = process.attaches.get(&address) else {
        return Err(EINVAL);
    };
    let end = address.saturating_add(attach.mapped_length);
    // munmap or mremap may have moved the mapping; the recorded attachment then
    // no longer describes the address the guest asked to detach.
    if process.address_space.snapshot(&(address..end)).is_err() {
        return Err(EINVAL);
    }
    let attach = process.attaches.remove(&address).unwrap();
    if process.address_space.unmap(address..end).is_err() {
        process.attaches.insert(address, attach);
        return Err(EINVAL);
    }
    if attach.counted {
        let shm = Rc::clone(&process.shm);
        shm.borrow_mut()
            .detach(attach.id, process.pid, now().unwrap_or(0));
    }
    Ok(0)
}

pub(super) fn shmctl(process: &Process<'_>, arguments: [usize; 6]) -> SyscallResult {
    let id = arguments[0] as u32 as usize;
    let command = arguments[1] as i32;
    let address = arguments[2];
    match command {
        IPC_RMID => rmid(process, id),
        IPC_SET => set(process, id, address),
        IPC_STAT => stat(process, id, address),
        IPC_INFO => info(process, address),
        SHM_INFO => shm_info(process, address),
        _ => Err(EINVAL),
    }
}

fn rmid(process: &Process<'_>, id: usize) -> SyscallResult {
    let shm = Rc::clone(&process.shm);
    let mut table = shm.borrow_mut();
    let Some(segment) = table.get(id) else {
        return Err(EINVAL);
    };
    if process.credentials.euid != 0
        && process.credentials.euid != segment.uid
        && process.credentials.euid != segment.cuid
    {
        return Err(EPERM);
    }
    let now = now().unwrap_or(0);
    table.mark_removed(id, process.pid, now);
    Ok(0)
}

fn stat(process: &Process<'_>, id: usize, address: usize) -> SyscallResult {
    if address == 0 {
        return Err(EFAULT);
    }
    let shm = Rc::clone(&process.shm);
    let table = shm.borrow();
    let Some(segment) = table.get(id) else {
        return Err(EINVAL);
    };
    if !segment.permits(process.credentials.euid, process.credentials.egid, 0o444) {
        return Err(EACCES);
    }
    let bytes = encode(segment);
    drop(table);
    process
        .address_space
        .write(address, &bytes)
        .map_err(|_| EFAULT)?;
    Ok(0)
}

fn set(process: &Process<'_>, id: usize, address: usize) -> SyscallResult {
    if address == 0 {
        return Err(EFAULT);
    }
    let shm = Rc::clone(&process.shm);
    {
        let table = shm.borrow();
        let Some(segment) = table.get(id) else {
            return Err(EINVAL);
        };
        if process.credentials.euid != 0
            && process.credentials.euid != segment.uid
            && process.credentials.euid != segment.cuid
        {
            return Err(EPERM);
        }
    }
    let mut bytes = [0; SET_SIZE];
    process
        .address_space
        .read(address, &mut bytes)
        .map_err(|_| EFAULT)?;
    let mode = read_u32(&bytes, 20) as u16 & 0o777;
    let uid = read_u32(&bytes, 4);
    let gid = read_u32(&bytes, 8);
    let mut table = shm.borrow_mut();
    let Some(segment) = table.get_mut(id) else {
        return Err(EINVAL);
    };
    if process.credentials.euid != 0
        && (uid != segment.uid && uid != process.credentials.euid
            || gid != segment.gid && gid != process.credentials.egid)
    {
        return Err(EPERM);
    }
    segment.mode = mode;
    segment.uid = uid;
    segment.gid = gid;
    segment.ctime = now().unwrap_or(0);
    Ok(0)
}

fn info(process: &Process<'_>, address: usize) -> SyscallResult {
    if address == 0 {
        return Err(EFAULT);
    }
    let shm = Rc::clone(&process.shm);
    let table = shm.borrow();
    let mut bytes = [0; INFO_SIZE];
    // `shminfo`: maximum segment and minimum size in bytes, segment limits,
    // then the system-wide limit in pages.
    write_u64(&mut bytes, 0, MAX_SIZE as u64);
    write_u64(&mut bytes, 8, 1);
    write_u64(&mut bytes, 16, MAX_SEGMENTS as u64);
    write_u64(&mut bytes, 24, MAX_SEGMENTS as u64);
    write_u64(&mut bytes, 32, (MAX_SIZE / PAGE_SIZE) as u64);
    let id = table.max_id();
    drop(table);
    process
        .address_space
        .write(address, &bytes)
        .map_err(|_| EFAULT)?;
    Ok(id)
}

fn shm_info(process: &Process<'_>, address: usize) -> SyscallResult {
    if address == 0 {
        return Err(EFAULT);
    }
    let shm = Rc::clone(&process.shm);
    let table = shm.borrow();
    let mut bytes = [0; SHM_INFO_SIZE];
    // `shm_info`: used identifiers, then total and resident page counts.
    write_u32(&mut bytes, 0, table.len() as u32);
    write_u64(&mut bytes, 8, (table.total_size() / PAGE_SIZE) as u64);
    write_u64(&mut bytes, 16, table.resident_pages() as u64);
    let id = table.max_id();
    drop(table);
    process
        .address_space
        .write(address, &bytes)
        .map_err(|_| EFAULT)?;
    Ok(id)
}

fn encode(segment: &Segment) -> [u8; STAT_SIZE] {
    let mut bytes = [0; STAT_SIZE];
    // `ipc64_perm`: key, owner and creator identities, then mode. Bytes 24-47
    // are the sequence and padding fields, which stay zero.
    write_u32(&mut bytes, 0, segment.key.unwrap_or(0) as u32);
    write_u32(&mut bytes, 4, segment.uid);
    write_u32(&mut bytes, 8, segment.gid);
    write_u32(&mut bytes, 12, segment.cuid);
    write_u32(&mut bytes, 16, segment.cgid);
    write_u32(
        &mut bytes,
        20,
        u32::from(segment.mode) | if segment.removed { 0o1000 } else { 0 },
    );
    // `segsz` is the requested byte count; the timestamps are seconds.
    write_u64(&mut bytes, 48, segment.size as u64);
    write_u64(&mut bytes, 56, seconds(segment.atime));
    write_u64(&mut bytes, 64, seconds(segment.dtime));
    write_u64(&mut bytes, 72, seconds(segment.ctime));
    write_u32(&mut bytes, 80, segment.cpid as u32);
    write_u32(&mut bytes, 84, segment.lpid as u32);
    write_u64(&mut bytes, 88, segment.nattch as u64);
    bytes
}

fn seconds(nanoseconds: u64) -> u64 {
    nanoseconds / 1_000_000_000
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn now() -> Result<u64, Errno> {
    super::wait::now()
}

fn mapping_error(error: runtime::Error) -> Errno {
    match error {
        runtime::Error::Kernel(runtime::KernelError::Denied) => EACCES,
        runtime::Error::Kernel(
            runtime::KernelError::NoMemory | runtime::KernelError::BadAddress,
        ) => ENOMEM,
        runtime::Error::Kernel(
            runtime::KernelError::Busy | runtime::KernelError::InvalidArgument,
        ) => EINVAL,
        _ => EIO,
    }
}
