// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Argument snapshots and image replacement for execve.

use crate::{
    Error, Services,
    elf::{Image, ImageSet},
    memory::AddressSpace,
    process::{
        Process, Thread,
        image::{Chain, Executable, MAX_INTERPRETERS, parse_script, script_arguments},
    },
    protocol::Arguments,
    syscall::{
        Operation, Outcome,
        abi::{EFAULT, ELOOP, ENOEXEC, ENOMEM, Errno},
        file::{filesystem_error, path::Lookup},
    },
};
use alloc::{boxed::Box, rc::Rc, vec::Vec};
use runtime::{Call, Handle, Rights, thread::UserEntry};

pub(in crate::syscall) struct Pending<'a> {
    executable: Chain<'a>,
    arguments: Arguments,
    depth: usize,
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
            executable: Chain::new(
                Executable::new(files, lookup.directory(), &lookup.name).map_err(error)?,
            ),
            arguments,
            depth: 0,
            _anchor: lookup.anchor,
        })
    })();
    match result {
        Ok(pending) => Outcome::Pending(Operation::Exec(Box::new(pending))),
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
    fn start_interpreter(
        &mut self,
        process: &Process<'a>,
        files: &fs::Client<'a>,
        interpreter: &[u8],
        argument: &[u8],
    ) -> Result<(), Errno> {
        if self.depth >= MAX_INTERPRETERS {
            return Err(ELOOP);
        }
        let lookup = Lookup::from_name(process, (-100i32) as usize, interpreter.to_vec())?;
        let executable = Executable::new(files, lookup.directory(), interpreter).map_err(error)?;
        self.depth += 1;
        script_arguments(&mut self.arguments, interpreter, argument);
        self.executable = Chain::new(executable);
        self._anchor = lookup.anchor;
        Ok(())
    }

    pub fn call(&self) -> Option<&Handle<Call>> {
        self.executable.call()
    }

    pub fn poll(
        &mut self,
        process: &mut Process<'a>,
        index: usize,
        files: &fs::Client<'a>,
        services: &Services<'_>,
    ) -> Option<Result<(), Errno>> {
        let loaded = match self.executable.poll(files, process.machine, services) {
            Ok(Some(loaded)) => loaded,
            Ok(None) => return None,
            Err(err) => return Some(Err(error(err))),
        };
        if loaded.main.bytes.starts_with(b"#!") {
            return match parse_script(&loaded.main.bytes).map_err(error).and_then(
                |(interpreter, argument)| {
                    self.start_interpreter(process, files, interpreter, argument)
                },
            ) {
                Ok(()) => None,
                Err(error) => Some(Err(error)),
            };
        }
        Some((|| {
            let image = Image::parse(&loaded.main.bytes, process.machine).map_err(error)?;
            let interpreter = loaded
                .interpreter
                .as_ref()
                .map(|loaded| Image::parse_at(&loaded.bytes, process.machine, image.end()))
                .transpose()
                .map_err(error)?;
            if interpreter
                .as_ref()
                .is_some_and(|interpreter| interpreter.interpreter.is_some())
            {
                return Err(ENOEXEC);
            }
            replace(
                process,
                index,
                &ImageSet { image, interpreter },
                &self.arguments,
                loaded.main.random.as_ref(),
            )
        })())
    }
}

fn replace(
    process: &mut Process<'_>,
    index: usize,
    images: &ImageSet<'_>,
    arguments: &Arguments,
    random: Option<&[u8; 16]>,
) -> Result<(), Errno> {
    let entry = images.entry();
    let (space, stack) =
        AddressSpace::load(process.supervisor, &process.port, images, arguments, random)
            .map_err(error)?;
    let native = space
        .native
        .create_thread(
            UserEntry {
                pc: entry as u64,
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
        .set_user_state(&process.machine.initial(entry, stack))
        .map_err(super::error)?;
    let native_id = native.id().map_err(super::error)?;
    native.start().map_err(super::error)?;
    let thread_signals = Rc::clone(&process.threads[index].signals);
    for thread in &process.threads {
        process.namespace.remove_thread(thread.tid);
    }
    process.threads.clear();
    process.signals.borrow_mut().exec();
    process.signal_return = None;
    {
        let mut signals = thread_signals.borrow_mut();
        signals.altstack = Default::default();
        signals.suspended_mask = None;
        signals.restart = None;
        signals.restart_frames.clear();
    }
    process
        .namespace
        .register_thread(process.pid, process.pid, &thread_signals);
    process.address_space = space;
    process.threads.push(Thread {
        native,
        native_id,
        tid: process.pid,
        clear_tid: 0,
        pending: None,
        signals: thread_signals,
        held: None,
        vfork: None,
        interrupt_requested: false,
    });
    process.files.close_on_exec();
    process.program.clone_from(&arguments.path);
    process.argv.clone_from(&arguments.argv);
    process.namespace.exec(process.pid);
    if let Some(vfork) = process.vfork.take() {
        vfork.release();
    }
    Ok(())
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
