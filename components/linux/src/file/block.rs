// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Block devices and MBR primary partitions exposed under `/dev`.

use alloc::{rc::Rc, vec::Vec};

use crate::{
    Services,
    process::Process,
    syscall::abi::{EBUSY, EINVAL, EIO, ENOENT, ENOTDIR, ENXIO, EROFS, Errno},
};

const VIRTIO_BLOCK_MAJOR: u32 = 254;
const O_CREAT: u32 = 0x40;
const O_EXCL: u32 = 0x80;
const O_TRUNC: u32 = 0x200;
const O_DIRECTORY: u32 = 0x10000;
const O_CLOEXEC: u32 = 0x80000;
const SEEK_SET: usize = 0;
const SEEK_CUR: usize = 1;
const SEEK_END: usize = 2;

pub(crate) struct BlockDevice {
    disk: fs::block::Disk,
    inode: u64,
    rdev: u64,
}

impl BlockDevice {
    pub fn capacity(&self) -> u64 {
        self.disk.capacity()
    }

    pub fn block_size(&self) -> u64 {
        self.disk.block_size()
    }

    pub fn inode(&self) -> u64 {
        self.inode
    }

    pub fn rdev(&self) -> u64 {
        self.rdev
    }

    pub fn read(&self, offset: u64, output: &mut [u8]) -> Result<usize, Errno> {
        if output.is_empty() {
            return Ok(0);
        }
        if offset >= self.disk.capacity() {
            return Ok(0);
        }
        let count = usize::try_from(
            (self.disk.capacity() - offset).min(u64::try_from(output.len()).map_err(|_| EINVAL)?),
        )
        .map_err(|_| EINVAL)?;
        let mut done = 0;
        while done < count {
            let position = offset + done as u64;
            let aligned = position & !(self.disk.block_size() - 1);
            let inside = usize::try_from(position - aligned).map_err(|_| EINVAL)?;
            let mut bytes = [0; fs::block::MAX_BLOCK_SIZE];
            let block_size = self.disk.block_size() as usize;
            self.disk
                .read(aligned, &mut bytes[..block_size])
                .map_err(io_error)?;
            let number = (block_size - inside).min(count - done);
            output[done..done + number].copy_from_slice(&bytes[inside..inside + number]);
            done += number;
        }
        Ok(done)
    }

    pub fn write(&self, offset: u64, input: &[u8]) -> Result<usize, Errno> {
        if input.is_empty() {
            return Ok(0);
        }
        if self.disk.read_only() {
            return Err(EROFS);
        }
        let end = offset
            .checked_add(u64::try_from(input.len()).map_err(|_| EINVAL)?)
            .ok_or(EINVAL)?;
        if offset >= self.disk.capacity() || end > self.disk.capacity() {
            return Err(EINVAL);
        }
        let mut done = 0;
        while done < input.len() {
            let position = offset + done as u64;
            let aligned = position & !(self.disk.block_size() - 1);
            let inside = usize::try_from(position - aligned).map_err(|_| EINVAL)?;
            let number = (self.disk.block_size() as usize - inside).min(input.len() - done);
            let mut bytes = [0; 4096];
            let block_size = self.disk.block_size() as usize;
            if inside != 0 || number != block_size {
                self.disk
                    .read(aligned, &mut bytes[..block_size])
                    .map_err(io_error)?;
            }
            bytes[inside..inside + number].copy_from_slice(&input[done..done + number]);
            self.disk
                .write(aligned, &bytes[..block_size])
                .map_err(io_error)?;
            done += number;
        }
        Ok(done)
    }

    pub fn seek(&self, current: u64, displacement: i64, whence: usize) -> Result<u64, Errno> {
        let base = match whence {
            SEEK_SET => 0,
            SEEK_CUR => i128::from(current),
            SEEK_END => i128::from(self.disk.capacity()),
            _ => return Err(EINVAL),
        };
        let position = base + i128::from(displacement);
        if !(0..=i128::from(self.disk.capacity())).contains(&position) {
            return Err(EINVAL);
        }
        u64::try_from(position).map_err(|_| EINVAL)
    }

    pub fn flush(&self) -> Result<(), Errno> {
        if !self.disk.supports_flush() {
            return Ok(());
        }
        self.disk.flush().map_err(io_error)
    }
}

pub(crate) fn disk_name(mut disk: usize) -> Vec<u8> {
    let mut letters = Vec::new();
    loop {
        letters.push(b'a' + (disk % 26) as u8);
        if disk < 26 {
            break;
        }
        disk = disk / 26 - 1;
    }
    letters.reverse();
    let mut name = b"vd".to_vec();
    name.extend(letters);
    name
}

pub(crate) fn open<'a>(
    process: &Process<'a>,
    services: &Services<'_>,
    path: &[u8],
    flags: u32,
) -> Option<Result<super::Descriptor<'a>, Errno>> {
    let (disk, partition) = parse(path)?;
    Some(open_at(process, services, disk, partition, flags))
}

fn open_at<'a>(
    process: &Process<'a>,
    services: &Services<'_>,
    disk: usize,
    partition: u32,
    flags: u32,
) -> Result<super::Descriptor<'a>, Errno> {
    if flags & O_DIRECTORY != 0 {
        return Err(ENOTDIR);
    }
    if flags & (O_CREAT | O_EXCL | O_TRUNC) != 0 {
        return Err(EINVAL);
    }
    let blocks = services.blocks.as_ref().ok_or(ENXIO)?;
    if disk >= blocks.disks().len() {
        return Err(ENOENT);
    }
    let id = blocks.disks()[disk];
    let session =
        devmgr::block::Session::open_partition(process.supervisor, blocks.control(), id, partition)
            .map_err(open_error)?;
    let minor = u32::try_from(disk)
        .ok()
        .and_then(|disk| disk.checked_mul(16))
        .and_then(|minor| minor.checked_add(partition))
        .ok_or(EINVAL)?;
    let rdev = u64::from(VIRTIO_BLOCK_MAJOR) << 8 | u64::from(minor & 0xff);
    Ok(super::Descriptor::block(
        Rc::new(BlockDevice {
            disk: fs::block::Disk::new(process.supervisor, session).map_err(io_error)?,
            inode: 0x1000 + u64::try_from(disk).unwrap_or(0) * 16 + u64::from(partition),
            rdev,
        }),
        flags & (3 | 0x800),
        flags & O_CLOEXEC != 0,
    ))
}

fn open_error(error: devmgr::Error) -> Errno {
    match error {
        devmgr::Error::NotFound => ENOENT,
        devmgr::Error::InvalidRequest => EINVAL,
        devmgr::Error::Busy
        | devmgr::Error::Runtime(runtime::Error::Kernel(
            runtime::KernelError::Busy | runtime::KernelError::WouldBlock,
        )) => EBUSY,
        devmgr::Error::Unsupported | devmgr::Error::Unavailable => ENXIO,
        _ => EIO,
    }
}

/// Splits `/dev/vdLETTERS[DIGITS]` into a zero-based disk index and partition number.
fn parse(path: &[u8]) -> Option<(usize, u32)> {
    let rest = path.strip_prefix(b"/dev/vd")?;
    let letters = rest
        .iter()
        .take_while(|byte| byte.is_ascii_lowercase())
        .count();
    if letters == 0 || letters > 4 {
        return None;
    }
    let mut disk = 0usize;
    for byte in &rest[..letters] {
        disk = disk
            .checked_mul(26)?
            .checked_add(usize::from(*byte - b'a') + 1)?;
    }
    let disk = disk - 1;
    let digits = &rest[letters..];
    if digits.is_empty() {
        return Some((disk, 0));
    }
    if !digits.iter().all(u8::is_ascii_digit) || digits[0] == b'0' {
        return None;
    }
    let mut partition = 0u32;
    for byte in digits {
        partition = partition
            .checked_mul(10)?
            .checked_add(u32::from(*byte - b'0'))?;
    }
    (partition != 0).then_some((disk, partition))
}

pub(crate) fn ioctl(
    device: &BlockDevice,
    command: usize,
    address: usize,
    process: &Process<'_>,
) -> Result<usize, Errno> {
    // REFERENCE: https://github.com/torvalds/linux/blob/v6.12/include/uapi/linux/fs.h
    match command {
        // BLKGETSIZE64
        0x8008_1272 => write(process, address, &device.capacity().to_le_bytes()),
        // BLKGETSIZE
        0x1260 => write(process, address, &(device.capacity() / 512).to_le_bytes()),
        // BLKSSZGET
        0x1268 => write(
            process,
            address,
            &(device.block_size() as i32).to_le_bytes(),
        ),
        // BLKROGET
        0x125e => write(
            process,
            address,
            &i32::from(device.disk.read_only()).to_le_bytes(),
        ),
        _ => Err(crate::syscall::abi::ENOTTY),
    }
}

fn write(process: &Process<'_>, address: usize, bytes: &[u8]) -> Result<usize, Errno> {
    process
        .address_space
        .write(address, bytes)
        .map_err(|_| crate::syscall::abi::EFAULT)?;
    Ok(0)
}

fn io_error(error: fs::Error) -> Errno {
    match error {
        fs::Error::ReadOnly => EROFS,
        fs::Error::InvalidRequest => EINVAL,
        _ => EIO,
    }
}
