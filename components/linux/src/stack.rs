// SPDX-License-Identifier: MPL-2.0

use crate::memory::{AddressSpace, PageBacking};
use alloc::rc::Rc;
use alloc::vec::Vec;
use runtime::{Rights, memory::Permissions};

use crate::{
    Error,
    elf::{Image, PAGE_SIZE},
    protocol::Arguments,
};

const STACK_SIZE: usize = 64 * 1024;

// REFERENCE: https://refspecs.linuxfoundation.org/elf/gabi4+/ch5.pheader.html
pub(crate) fn build(
    space: &AddressSpace,
    image: &Image<'_>,
    arguments: &Arguments,
) -> Result<usize, Error> {
    let task = &space.native;
    let (phdr, top) = load_headers(space, image)?;
    let auxv = auxiliary(image, phdr);
    // TODO: 接入熵源后提供 AT_RANDOM。
    let strings = core::iter::once(&arguments.path)
        .chain(&arguments.argv)
        .chain(&arguments.envp);
    let string_size = strings
        .clone()
        .try_fold(0usize, |size, string| {
            size.checked_add(string.len())?.checked_add(1)
        })
        .ok_or(Error::InvalidRequest)?;
    let words = arguments
        .argv
        .len()
        .checked_add(arguments.envp.len())
        .and_then(|size| size.checked_add(3 + (auxv.len() + 2) * 2))
        .and_then(|size| size.checked_mul(8))
        .ok_or(Error::InvalidRequest)?;
    let length = STACK_SIZE
        .checked_add(string_size)
        .and_then(|size| size.checked_add(words))
        .and_then(|size| size.checked_add(15))
        .and_then(|size| size.checked_next_multiple_of(PAGE_SIZE))
        .ok_or(Error::InvalidRequest)?;
    let reserved = length.checked_add(PAGE_SIZE).ok_or(Error::InvalidRequest)?;
    let start = top.checked_sub(reserved).ok_or(Error::InvalidRequest)?;
    let mut reservation = task.task().reserve(reserved, PAGE_SIZE, Some(start))?;
    let range = reservation.range().ok_or(runtime::Error::Protocol)?;
    reservation.release()?;
    task.reserve(PAGE_SIZE, PAGE_SIZE, Some(range.start))?;
    let base = range.start + PAGE_SIZE;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| Error::NoMemory)?;
    bytes.resize(length, 0);
    let mut cursor = length;
    let pointers: Vec<_> = strings
        .map(|string| {
            cursor -= string.len() + 1;
            bytes[cursor..cursor + string.len()].copy_from_slice(string);
            base + cursor
        })
        .collect();
    let mut values = Vec::new();
    values.push(arguments.argv.len());
    values.extend_from_slice(&pointers[1..1 + arguments.argv.len()]);
    values.push(0);
    values.extend_from_slice(&pointers[1 + arguments.argv.len()..]);
    values.push(0);
    for (kind, value) in auxv {
        values.extend_from_slice(&[kind, value]);
    }
    values.extend_from_slice(&[31, pointers[0], 0, 0]);
    let stack = (cursor - values.len() * 8) & !15;
    for (chunk, value) in bytes[stack..].as_chunks_mut::<8>().0.iter_mut().zip(values) {
        chunk.copy_from_slice(&(value as u64).to_le_bytes());
    }
    let count = (length / PAGE_SIZE) as u64;
    let mut frames = task
        .task()
        .allocate(count, Rights::READ_WRITE | Rights::MAP | Rights::EXECUTE)?;
    frames.write(stack, &bytes[stack..])?;
    space.map_frames(
        Rc::new(frames),
        0,
        count,
        base,
        Permissions::ReadWrite,
        PageBacking::Anonymous,
    )?;
    Ok(base + stack)
}

fn load_headers(space: &AddressSpace, image: &Image<'_>) -> Result<(usize, usize), Error> {
    let task = &space.native;
    let top = task.task().address_range()?.end;
    Ok(if let Some(address) = image.phdr {
        (address, top)
    } else {
        let count = image.headers.len().div_ceil(PAGE_SIZE) as u64;
        let address = top
            .checked_sub(count as usize * PAGE_SIZE)
            .ok_or(Error::InvalidRequest)?;
        let mut frames = task
            .task()
            .allocate(count, Rights::READ_WRITE | Rights::MAP)?;
        frames.write(0, image.headers)?;
        space.map_frames(
            Rc::new(frames),
            0,
            count,
            address,
            Permissions::ReadOnly,
            PageBacking::File,
        )?;
        (address, address)
    })
}

fn auxiliary(image: &Image<'_>, phdr: usize) -> [(usize, usize); 7] {
    [
        (3, phdr),
        (4, 56),
        (5, image.headers.len() / 56),
        (6, PAGE_SIZE),
        (7, 0),
        (8, 0),
        (9, image.entry),
    ]
}
