// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Terminal sessions, line discipline, and input buffering.

// TODO: Support pseudo-terminals and multiple independent terminal sessions.

use alloc::{collections::VecDeque, vec::Vec};
use core::cell::{Cell, Ref, RefCell};
use runtime::{Call, Event, Handle, Rights};

pub(crate) struct Terminal {
    pub output: devmgr::uart::Session,
    source: Option<devmgr::uart::Session>,
    input: RefCell<Option<Input>>,
    state: RefCell<State>,
    control: Cell<Option<(usize, usize)>>,
    exclusive: Cell<bool>,
    stopped: Cell<bool>,
    resume: Handle<Event>,
}

struct State {
    termios: [u8; 36],
    window: [u8; 8],
    ready: VecDeque<VecDeque<u8>>,
    line: Vec<u8>,
    failed: bool,
    signals: VecDeque<u8>,
    received: u64,
    literal: bool,
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
    pub fn new(
        task: &Handle<runtime::Task>,
        output: devmgr::uart::Session,
        input: Option<devmgr::uart::Session>,
    ) -> Result<Self, runtime::Error> {
        // REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/asm-generic/termbits.h?h=v6.12
        let mut termios = [0; 36];
        termios[..4].copy_from_slice(&0x500u32.to_le_bytes());
        termios[4..8].copy_from_slice(&5u32.to_le_bytes());
        termios[8..12].copy_from_slice(&0xbfu32.to_le_bytes());
        termios[12..16].copy_from_slice(&0x823bu32.to_le_bytes());
        termios[17..34]
            .copy_from_slice(&[3, 28, 127, 21, 4, 0, 1, 0, 17, 19, 26, 0, 18, 15, 23, 22, 0]);
        let window = [24, 0, 80, 0, 0, 0, 0, 0];
        Ok(Self {
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
                signals: VecDeque::new(),
                received: 0,
                literal: false,
            }),
            exclusive: Cell::new(false),
            stopped: Cell::new(false),
            resume: task.event(Rights::WAIT | Rights::SIGNAL)?,
        })
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
    pub fn exclusive(&self) -> bool {
        self.exclusive.get()
    }
    pub fn set_exclusive(&self, value: bool) {
        self.exclusive.set(value);
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
        if flush || value[12] & 2 == 0 {
            state.literal = false;
        }
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
        let mut state = self.state.borrow_mut();
        if state.window != value {
            state.window = value;
            state.signals.push_back(28);
        }
    }
    pub fn available(&self) -> usize {
        self.state.borrow().ready.iter().map(VecDeque::len).sum()
    }
    pub fn received(&self) -> u64 {
        self.state.borrow().received
    }
    pub fn take_signal(&self) -> Option<u8> {
        self.state.borrow_mut().signals.pop_front()
    }
    pub fn readable(&self) -> bool {
        let state = self.state.borrow();
        if state.termios[12] & 2 != 0 {
            !state.ready.is_empty()
        } else {
            let minimum = if state.termios[22] == 0 {
                usize::from(state.termios[23]).max(1)
            } else {
                1
            };
            state.ready.iter().map(VecDeque::len).sum::<usize>() >= minimum
        }
    }
    pub fn snapshot(&self, length: usize) -> Vec<u8> {
        let state = self.state.borrow();
        let lines = if state.termios[12] & 2 != 0 {
            1
        } else {
            usize::MAX
        };
        state
            .ready
            .iter()
            .take(lines)
            .flatten()
            .copied()
            .take(length)
            .collect()
    }
    pub fn consume(&self, mut length: usize) {
        let mut state = self.state.borrow_mut();
        while let Some(line) = state.ready.front_mut() {
            let count = length.min(line.len());
            line.drain(..count);
            length -= count;
            if line.is_empty() {
                state.ready.pop_front();
            }
            if length == 0 {
                break;
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

    pub fn stopped(&self) -> bool {
        self.stopped.get()
    }

    /// Stops terminal output; subsequent writes wait until output resumes.
    pub fn stop_output(&self) {
        self.stopped.set(true);
    }

    /// Resumes terminal output and wakes the service, which then re-polls stopped writers.
    pub fn start_output(&self) {
        self.stopped.set(false);
        let _ = self.resume.signal(1);
    }

    pub fn resume_event(&self) -> &Handle<Event> {
        &self.resume
    }

    /// Simulates terminal input: the byte passes through the line discipline, so it is
    /// echoed, completes lines, and raises control character signals like typed input.
    pub fn inject(&self, byte: u8) -> Result<(), devmgr::Error> {
        let echo = self.receive(alloc::vec![byte]);
        if echo.is_empty() {
            return Ok(());
        }
        let mut input = self.input.borrow_mut();
        if let Some(current) = input.as_mut() {
            current.echo.extend(echo);
            return Ok(());
        }
        if self.source.is_none() {
            // Without an input session nothing drives the echo queue; submit it directly.
            self.output.write_all(&echo)?;
            self.output.flush()?;
            return Ok(());
        }
        let queue: VecDeque<u8> = echo.into();
        let request = echo_request(&self.output, &queue)?;
        *input = Some(Input {
            request,
            echo: queue,
        });
        Ok(())
    }

    // TODO: Support software flow control.
    fn receive(&self, bytes: Vec<u8>) -> Vec<u8> {
        let mut echo = Vec::new();
        let mut state = self.state.borrow_mut();
        let input_flags = u32::from_le_bytes(state.termios[..4].try_into().unwrap());
        let local = u32::from_le_bytes(state.termios[12..16].try_into().unwrap());
        for mut byte in bytes {
            if input_flags & 0x20 != 0 {
                byte &= 0x7f;
            }
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
            if !state.literal && state.control(byte, local, &mut echo) {
                continue;
            }
            let literal = core::mem::take(&mut state.literal);
            if local & 8 != 0 || local & 0x40 != 0 && byte == b'\n' {
                echo_byte(byte, local, &mut echo);
            }
            state.received = state.received.wrapping_add(1);
            if local & 2 != 0 {
                state.line.push(byte);
                if !literal
                    && (byte == b'\n'
                        || state.matches(11, byte)
                        || local & 0x8000 != 0 && state.matches(16, byte))
                {
                    let line = core::mem::take(&mut state.line);
                    state.ready.push_back(line.into());
                }
            } else if let Some(line) = state.ready.back_mut() {
                line.push_back(byte);
            } else {
                state.ready.push_back(alloc::vec![byte].into());
            }
        }
        echo
    }
}

impl State {
    fn matches(&self, index: usize, byte: u8) -> bool {
        byte != 0 && self.termios[17 + index] == byte
    }

    fn control(&mut self, byte: u8, local: u32, echo: &mut Vec<u8>) -> bool {
        if local & 1 != 0 {
            let signal = [(0, 2), (1, 3), (10, 20)]
                .into_iter()
                .find_map(|(index, number)| self.matches(index, byte).then_some(number));
            if let Some(signal) = signal {
                if local & 0x80 == 0 {
                    self.line.clear();
                    self.ready.clear();
                    echo.clear();
                }
                self.signals.push_back(signal);
                if local & 8 != 0 {
                    echo_byte(byte, local, echo);
                }
                return true;
            }
        }
        if local & 2 == 0 {
            return false;
        }
        if local & 0x8000 != 0 && self.matches(15, byte) {
            self.literal = true;
            return true;
        }
        if self.matches(2, byte) {
            self.erase(local, echo);
        } else if self.matches(3, byte) {
            self.line.clear();
            if local & 0x28 == 0x28 {
                echo.extend_from_slice(b"\r\n");
            }
        } else if self.matches(4, byte) {
            let line = core::mem::take(&mut self.line);
            self.ready.push_back(line.into());
        } else if local & 0x8000 != 0 && local & 8 != 0 && self.matches(12, byte) {
            echo_byte(byte, local, echo);
            echo_byte(b'\n', local, echo);
            for &byte in &self.line {
                echo_byte(byte, local, echo);
            }
        } else if local & 0x8000 != 0 && self.matches(14, byte) {
            while self.line.last().is_some_and(u8::is_ascii_whitespace) {
                self.erase(local, echo);
            }
            while self
                .line
                .last()
                .is_some_and(|byte| !byte.is_ascii_whitespace())
            {
                self.erase(local, echo);
            }
        } else {
            return false;
        }
        true
    }

    fn erase(&mut self, local: u32, echo: &mut Vec<u8>) {
        let Some(mut start) = self.line.len().checked_sub(1) else {
            return;
        };
        // REFERENCE: https://man7.org/linux/man-pages/man3/termios.3.html
        let flags = u32::from_le_bytes(self.termios[..4].try_into().unwrap());
        if flags & 0x4000 != 0 {
            while start > 0 && self.line[start] & 0xc0 == 0x80 && self.line.len() - start < 4 {
                start -= 1;
            }
            if core::str::from_utf8(&self.line[start..]).is_err() {
                start = self.line.len() - 1;
            }
        }
        let byte = self.line[start];
        self.line.truncate(start);
        if local & 8 != 0 {
            if local & 0x10 != 0 {
                echo.extend_from_slice(b"\x08 \x08");
                if local & 0x200 != 0 && byte.is_ascii_control() && byte != b'\t' {
                    echo.extend_from_slice(b"\x08 \x08");
                }
            } else {
                echo_byte(self.termios[19], local, echo);
            }
        }
    }
}

fn echo_byte(byte: u8, local: u32, echo: &mut Vec<u8>) {
    if local & 0x200 != 0 && byte.is_ascii_control() && !matches!(byte, b'\n' | b'\t') {
        echo.extend_from_slice(&[b'^', byte ^ 0x40]);
    } else {
        if byte == b'\n' {
            echo.push(b'\r');
        }
        echo.push(byte);
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
            echo_request(&terminal.output, &self.echo)?
        };
        Ok(true)
    }
}

/// Submits the first chunk of a pending echo queue for output.
fn echo_request(
    output: &devmgr::uart::Session,
    echo: &VecDeque<u8>,
) -> Result<InputRequest, devmgr::Error> {
    let bytes: Vec<_> = echo.iter().copied().take(devmgr::uart::MAX_BYTES).collect();
    Ok(InputRequest::Echo(output.start_write(&bytes)?))
}
