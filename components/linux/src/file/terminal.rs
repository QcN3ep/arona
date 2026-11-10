// SPDX-License-Identifier: MPL-2.0

//! 终端会话、行规程与输入缓存。

use alloc::{collections::VecDeque, vec::Vec};
use core::cell::{Cell, Ref, RefCell};
use runtime::{Call, Handle};

pub(crate) struct Terminal {
    pub output: devmgr::uart::Session,
    source: Option<devmgr::uart::Session>,
    input: RefCell<Option<Input>>,
    state: RefCell<State>,
    control: Cell<Option<(usize, usize)>>,
}

struct State {
    termios: [u8; 36],
    window: [u8; 8],
    ready: VecDeque<VecDeque<u8>>,
    line: Vec<u8>,
    failed: bool,
}

pub(crate) struct Input {
    request: InputRequest,
    echo: VecDeque<u8>,
}

enum InputRequest {
    Read(devmgr::uart::PendingRead),
    Echo(devmgr::device::Pending<usize, usize>),
}

impl Terminal {
    pub fn new(output: devmgr::uart::Session, input: Option<devmgr::uart::Session>) -> Self {
        // REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/asm-generic/termbits.h?h=v6.12
        let mut termios = [0; 36];
        termios[..4].copy_from_slice(&0x500u32.to_le_bytes());
        termios[4..8].copy_from_slice(&5u32.to_le_bytes());
        termios[8..12].copy_from_slice(&0xbfu32.to_le_bytes());
        termios[12..16].copy_from_slice(&0x803bu32.to_le_bytes());
        termios[17..27].copy_from_slice(&[3, 28, 127, 21, 4, 0, 1, 0, 17, 19]);
        let window = [24, 0, 80, 0, 0, 0, 0, 0];
        Self {
            output,
            control: Cell::new(None),
            source: input,
            input: RefCell::new(None),
            state: RefCell::new(State {
                termios,
                window,
                ready: VecDeque::new(),
                line: Vec::new(),
                failed: false,
            }),
        }
    }

    pub fn failed(&self) -> bool {
        self.state.borrow().failed
    }
    pub fn control(&self) -> Option<(usize, usize)> {
        self.control.get()
    }
    pub fn set_control(&self, control: Option<(usize, usize)>) {
        self.control.set(control);
    }
    pub fn disconnect(&self) {
        self.state.borrow_mut().failed = true;
    }

    pub fn has_input(&self) -> bool {
        self.source.is_some()
    }
    pub fn termios(&self) -> [u8; 36] {
        self.state.borrow().termios
    }
    pub fn set_termios(&self, value: [u8; 36], flush: bool) {
        let mut state = self.state.borrow_mut();
        state.termios = value;
        if value[12] & 2 == 0 && !state.line.is_empty() && !flush {
            let line = core::mem::take(&mut state.line);
            state.ready.push_back(line.into());
        }
        if flush {
            state.ready.clear();
            state.line.clear();
        }
    }
    pub fn window(&self) -> [u8; 8] {
        self.state.borrow().window
    }
    pub fn set_window(&self, value: [u8; 8]) {
        self.state.borrow_mut().window = value;
    }
    pub fn available(&self) -> usize {
        self.state.borrow().ready.iter().map(VecDeque::len).sum()
    }
    pub fn readable(&self) -> bool {
        !self.state.borrow().ready.is_empty()
    }
    pub fn snapshot(&self, length: usize) -> Vec<u8> {
        self.state
            .borrow()
            .ready
            .front()
            .map_or_else(Vec::new, |line| line.iter().copied().take(length).collect())
    }
    pub fn consume(&self, length: usize) {
        let mut state = self.state.borrow_mut();
        if let Some(line) = state.ready.front_mut() {
            line.drain(..length);
            if line.is_empty() {
                state.ready.pop_front();
            }
        }
    }

    pub fn input(&self) -> Option<Ref<'_, Input>> {
        Ref::filter_map(self.input.borrow(), Option::as_ref).ok()
    }

    pub fn poll(&self) -> bool {
        let Some(source) = &self.source else {
            return false;
        };
        if self.failed() {
            return false;
        }
        let mut input = self.input.borrow_mut();
        let progress = if let Some(input) = input.as_mut() {
            input.poll(self, source)
        } else {
            source.start_read(devmgr::uart::MAX_BYTES).map(|request| {
                *input = Some(Input {
                    request: InputRequest::Read(request),
                    echo: VecDeque::new(),
                });
                true
            })
        };
        progress.unwrap_or_else(|_| {
            self.disconnect();
            *input = None;
            true
        })
    }

    pub fn output(&self, bytes: &[u8]) -> (Vec<u8>, usize) {
        let flags = u32::from_le_bytes(self.state.borrow().termios[4..8].try_into().unwrap());
        let mut output = Vec::new();
        let mut count = 0;
        for &byte in bytes {
            let newline = flags & 5 == 5 && byte == b'\n';
            if output.len() + 1 + usize::from(newline) > devmgr::uart::MAX_BYTES {
                break;
            }
            if newline {
                output.push(b'\r');
            }
            output.push(byte);
            count += 1;
        }
        (output, count)
    }

    fn receive(&self, bytes: Vec<u8>) -> Vec<u8> {
        let mut echo = Vec::new();
        let mut state = self.state.borrow_mut();
        let input_flags = u32::from_le_bytes(state.termios[..4].try_into().unwrap());
        let local = u32::from_le_bytes(state.termios[12..16].try_into().unwrap());
        for mut byte in bytes {
            if byte == b'\r' {
                if input_flags & 0x80 != 0 {
                    continue;
                }
                if input_flags & 0x100 != 0 {
                    byte = b'\n';
                }
            } else if byte == b'\n' && input_flags & 0x40 != 0 {
                byte = b'\r';
            }
            if input_flags & 0x20 != 0 {
                byte &= 0x7f;
            }
            if local & 8 != 0
                && byte != state.termios[21]
                && byte != state.termios[19]
                && byte != state.termios[20]
                || local & 0x40 != 0 && byte == b'\n'
            {
                if byte == b'\n' {
                    echo.push(b'\r');
                }
                echo.push(byte);
            }
            if local & 2 != 0 {
                if byte == state.termios[19] {
                    if state.line.pop().is_some() && local & 8 != 0 {
                        if local & 0x10 != 0 {
                            echo.extend_from_slice(b"\x08 \x08");
                        } else {
                            echo.push(byte);
                        }
                    }
                    continue;
                }
                if byte == state.termios[20] {
                    state.line.clear();
                    if local & 8 != 0 && local & 0x20 != 0 {
                        echo.extend_from_slice(b"\r\n");
                    }
                    continue;
                }
                if byte == state.termios[21] {
                    let line = core::mem::take(&mut state.line);
                    state.ready.push_back(line.into());
                    continue;
                }
                state.line.push(byte);
                if byte == b'\n' || state.termios[28] != 0 && byte == state.termios[28] {
                    let line = core::mem::take(&mut state.line);
                    state.ready.push_back(line.into());
                }
            } else {
                if let Some(line) = state.ready.back_mut() {
                    line.push_back(byte);
                } else {
                    state.ready.push_back(alloc::vec![byte].into());
                }
            }
        }
        echo
    }
}

impl Input {
    pub fn call(&self) -> &Handle<Call> {
        match &self.request {
            InputRequest::Read(request) => request.call(),
            InputRequest::Echo(request) => request.call(),
        }
    }
    fn poll(
        &mut self,
        terminal: &Terminal,
        source: &devmgr::uart::Session,
    ) -> Result<bool, devmgr::Error> {
        let result = match &mut self.request {
            InputRequest::Read(request) => request.try_complete().map(|(bytes, _)| {
                self.echo.extend(terminal.receive(bytes));
            }),
            InputRequest::Echo(request) => request.try_complete().and_then(|count| {
                if count == 0 || count > self.echo.len() {
                    return Err(devmgr::Error::Runtime(runtime::Error::Protocol));
                }
                self.echo.drain(..count);
                Ok(())
            }),
        };
        match result {
            Err(devmgr::Error::Runtime(runtime::Error::Kernel(
                runtime::KernelError::WouldBlock,
            ))) => return Ok(false),
            Err(error) => return Err(error),
            Ok(()) => {}
        }
        self.request = if self.echo.is_empty() {
            InputRequest::Read(source.start_read(devmgr::uart::MAX_BYTES)?)
        } else {
            let bytes: Vec<_> = self
                .echo
                .iter()
                .copied()
                .take(devmgr::uart::MAX_BYTES)
                .collect();
            InputRequest::Echo(terminal.output.start_write(&bytes)?)
        };
        Ok(true)
    }
}
