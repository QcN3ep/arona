// SPDX-License-Identifier: MPL-2.0

//! 终端读入与终端 ioctl。

use super::{Buffer, Pending, buffer::IoVectors, fd, outcome};
use crate::{
    file::{O_NONBLOCK, OpenFileDescription, terminal::Terminal},
    process::Process,
    syscall::{
        Outcome,
        abi::{EAGAIN, EBADF, EFAULT, EINVAL, EIO, ENOTTY, EOPNOTSUPP, Errno, SyscallResult},
    },
};
use alloc::rc::Rc;

pub(in crate::syscall) struct Read<'a> {
    description: Rc<OpenFileDescription<'a>>,
    buffers: IoVectors,
}

pub(super) fn read<'a>(process: &Process<'a>, raw: usize, buffer: Buffer) -> Outcome<'a> {
    outcome((|| {
        let description = Rc::clone(&process.files.get(fd(raw)?).ok_or(EBADF)?.description);
        if description.flags() & 3 == 1 {
            return Err(EBADF);
        }
        Ok(Pending::Terminal(Read {
            description,
            buffers: buffer.import(process)?,
        }))
    })())
}

impl Read<'_> {
    pub fn poll(&mut self, process: &Process<'_>) -> Option<SyscallResult> {
        if self.buffers.length() == 0 {
            return Some(Ok(0));
        }
        let OpenFileDescription::Terminal { terminal, .. } = self.description.as_ref() else {
            unreachable!()
        };
        if terminal.readable() {
            let mut bytes = terminal.snapshot(self.buffers.length());
            return Some(
                self.buffers
                    .scatter(process, &mut bytes)
                    .inspect(|&count| terminal.consume(count)),
            );
        }
        if terminal.failed() {
            return Some(Err(EIO));
        }
        let nonblocking = self.description.flags() & O_NONBLOCK != 0;
        let settings = terminal.termios();
        let immediate = nonblocking || settings[12] & 2 == 0 && settings[23] == 0;
        if immediate {
            return Some(if nonblocking { Err(EAGAIN) } else { Ok(0) });
        }
        None
    }
}

pub(in crate::syscall) struct Change {
    request: devmgr::device::Pending<()>,
    terminal: Rc<Terminal>,
    settings: [u8; 36],
    flush: bool,
}

impl Change {
    pub fn call(&self) -> &runtime::Handle<runtime::Call> {
        self.request.call()
    }

    pub fn poll(&mut self) -> Option<SyscallResult> {
        match self.request.try_complete() {
            Err(devmgr::Error::Runtime(runtime::Error::Kernel(
                runtime::KernelError::WouldBlock,
            ))) => None,
            Err(_) => Some(Err(EIO)),
            Ok(()) => {
                self.terminal.set_termios(self.settings, self.flush);
                Some(Ok(0))
            }
        }
    }
}

pub(super) fn start<'a>(
    process: &Process<'a>,
    raw: usize,
    command: usize,
    address: usize,
) -> Outcome<'a> {
    if !matches!(command as u32, 0x5403 | 0x5404) {
        return Outcome::Complete(ioctl(process, raw, command, address));
    }
    outcome((|| {
        let descriptor = process.files.get(fd(raw)?).ok_or(EBADF)?;
        let OpenFileDescription::Terminal { terminal, .. } = descriptor.description.as_ref() else {
            return Err(ENOTTY);
        };
        let settings = settings(process, address)?;
        let request = terminal.output.start_flush().map_err(|_| EIO)?;
        Ok(Pending::TerminalChange(Change {
            request,
            terminal: Rc::clone(terminal),
            settings,
            flush: command as u32 == 0x5404,
        }))
    })())
}

fn settings(process: &Process<'_>, address: usize) -> Result<[u8; 36], Errno> {
    let mut value = [0; 36];
    process
        .address_space
        .read(address, &mut value)
        .map_err(|_| EFAULT)?;
    if value[22] != 0 || value[23] > 1 {
        return Err(EOPNOTSUPP);
    }
    Ok(value)
}

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/asm-generic/ioctls.h?h=v6.12
pub(super) fn ioctl(
    process: &Process<'_>,
    raw: usize,
    command: usize,
    address: usize,
) -> SyscallResult {
    let descriptor = process.files.get(fd(raw)?).ok_or(EBADF)?;
    if command == 0x5421 {
        let mut value = [0; 4];
        process
            .address_space
            .read(address, &mut value)
            .map_err(|_| EFAULT)?;
        let mut flags = descriptor.description.flags() & !O_NONBLOCK;
        if i32::from_le_bytes(value) != 0 {
            flags |= O_NONBLOCK;
        }
        descriptor.description.set_flags(flags);
        return Ok(0);
    }
    if command == 0x541b {
        let length = match descriptor.description.as_ref() {
            OpenFileDescription::Pipe { end, .. } => end.available(),
            OpenFileDescription::Terminal { terminal, .. } => terminal.available(),
            _ => return Err(ENOTTY),
        };
        process
            .address_space
            .write(address, &(length as i32).to_le_bytes())
            .map_err(|_| EFAULT)?;
        return Ok(0);
    }
    let OpenFileDescription::Terminal { terminal, .. } = descriptor.description.as_ref() else {
        return Err(ENOTTY);
    };
    terminal_ioctl(process, terminal, command as u32, address)
}

fn terminal_ioctl(
    process: &Process<'_>,
    terminal: &Terminal,
    command: u32,
    address: usize,
) -> SyscallResult {
    match command {
        0x5401 => write(process, address, &terminal.termios()),
        0x5402 => {
            terminal.set_termios(settings(process, address)?, false);
            Ok(0)
        }
        0x5413 => write(process, address, &terminal.window()),
        0x5414 => {
            let mut value = [0; 8];
            process
                .address_space
                .read(address, &mut value)
                .map_err(|_| EFAULT)?;
            terminal.set_window(value);
            Ok(0)
        }
        0x540b => match address {
            0 => {
                terminal.set_termios(terminal.termios(), true);
                Ok(0)
            }
            1 | 2 => Err(EOPNOTSUPP),
            _ => Err(EINVAL),
        },
        // TODO: 接入会话、前台进程组与终端信号。
        _ => Err(ENOTTY),
    }
}

fn write(process: &Process<'_>, address: usize, bytes: &[u8]) -> Result<usize, Errno> {
    process
        .address_space
        .write(address, bytes)
        .map_err(|_| EFAULT)?;
    Ok(0)
}
