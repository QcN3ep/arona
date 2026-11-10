// SPDX-License-Identifier: MPL-2.0

//! execve 的参数快照与映像替换。

use crate::{
    Error, Services,
    elf::Image,
    memory::AddressSpace,
    process::{Process, Thread, image::Executable},
    protocol::Arguments,
    syscall::{
        Operation, Outcome,
        abi::{EFAULT, ENOEXEC, ENOMEM, Errno},
        file::{filesystem_error, path::Lookup},
    },
};
use alloc::{rc::Rc, vec::Vec};
use runtime::{Call, Handle, Rights, thread::UserEntry};

pub(in crate::syscall) struct Pending<'a> {
    executable: Executable<'a>,
    arguments: Arguments,
    _anchor: Option<Rc<crate::file::OpenFileDescription<'a>>>,
}

pub(in crate::syscall) fn start<'a>(
    process: &Process<'a>,
    files: &fs::Client<'a>,
    args: [usize; 6],
) -> Outcome<'a> {
    let result = (|| {
        let lookup = Lookup::new(process, (-100i32) as usize, args[0])?;
        let argv = strings(process, args[1])?;
        let envp = strings(process, args[2])?;
        let arguments = Arguments {
            path: lookup.name.clone(),
            argv: if argv.is_empty() {
                alloc::vec![Vec::new()]
            } else {
                argv
            },
            envp,
        };
        Ok(Pending {
            executable: Executable::new(files, lookup.directory(), &lookup.name).map_err(error)?,
            arguments,
            _anchor: lookup.anchor,
        })
    })();
    match result {
        Ok(pending) => Outcome::Pending(Operation::Exec(pending)),
        Err(error) => Outcome::Complete(Err(error)),
    }
}

fn strings(process: &Process<'_>, mut address: usize) -> Result<Vec<Vec<u8>>, Errno> {
    let mut values = Vec::new();
    if address == 0 {
        return Ok(values);
    }
    loop {
        let mut pointer = [0; 8];
        process
            .address_space
            .read(address, &mut pointer)
            .map_err(|_| EFAULT)?;
        let pointer = u64::from_le_bytes(pointer) as usize;
        if pointer == 0 {
            return Ok(values);
        }
        let mut value = Vec::new();
        let mut cursor = pointer;
        loop {
            let mut bytes = [0; crate::elf::PAGE_SIZE];
            let length = bytes.len() - cursor % bytes.len();
            process
                .address_space
                .read(cursor, &mut bytes[..length])
                .map_err(|_| EFAULT)?;
            let end = bytes[..length].iter().position(|byte| *byte == 0);
            let count = end.unwrap_or(length);
            value.try_reserve(count).map_err(|_| ENOMEM)?;
            value.extend_from_slice(&bytes[..count]);
            if end.is_some() {
                break;
            }
            cursor = cursor.checked_add(length).ok_or(EFAULT)?;
        }
        values.try_reserve(1).map_err(|_| ENOMEM)?;
        values.push(value);
        address = address.checked_add(8).ok_or(EFAULT)?;
    }
}

impl<'a> Pending<'a> {
    pub fn call(&self) -> Option<&Handle<Call>> {
        self.executable.call()
    }

    pub fn poll(
        &mut self,
        process: &mut Process<'a>,
        _index: usize,
        services: &Services<'_>,
    ) -> Option<Result<(), Errno>> {
        let loaded = match self.executable.poll(services) {
            Ok(Some(loaded)) => loaded,
            Ok(None) => return None,
            Err(err) => return Some(Err(error(err))),
        };
        Some((|| {
            let image = Image::parse(&loaded.bytes, process.machine).map_err(error)?;
            let (space, stack) = AddressSpace::load(
                process.supervisor,
                &process.port,
                &image,
                &self.arguments,
                loaded.random.as_ref(),
            )
            .map_err(error)?;
            let native = space
                .native
                .create_thread(
                    UserEntry {
                        pc: image.entry as u64,
                        stack_pointer: stack as u64,
                        thread_pointer: 0,
                        argument: 0,
                    },
                    Rights::WAIT
                        | Rights::START
                        | Rights::SET_STATE
                        | Rights::TERMINATE
                        | Rights::INTERRUPT,
                )
                .map_err(super::error)?;
            native
                .set_user_state(&process.machine.initial(image.entry, stack))
                .map_err(super::error)?;
            let native_id = native.id().map_err(super::error)?;
            native.start().map_err(super::error)?;
            process.threads.clear();
            process.address_space = space;
            process.threads.push(Thread {
                native,
                native_id,
                tid: process.pid,
                clear_tid: 0,
                pending: None,
            });
            process.files.close_on_exec();
            Ok(())
        })())
    }
}

fn error(error: Error) -> Errno {
    match error {
        Error::InvalidExecutable => ENOEXEC,
        Error::PermissionDenied => crate::syscall::abi::EACCES,
        Error::NoMemory
        | Error::Runtime(runtime::Error::Kernel(runtime::KernelError::NoMemory)) => ENOMEM,
        Error::Filesystem(error) => filesystem_error(error),
        _ => crate::syscall::abi::EIO,
    }
}
