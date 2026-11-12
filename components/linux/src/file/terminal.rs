// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Terminal sessions, line discipline, pseudo-terminals, and input buffering.

// TODO: Support pseudo-terminal packet mode and full session handover.

use alloc::{
    collections::{BTreeMap, VecDeque},
    rc::{Rc, Weak},
    vec::Vec,
};
use core::cell::{Cell, Ref, RefCell};
use runtime::{Call, Event, Handle, Rights};

/// Capacity of each pseudo-terminal data direction in bytes.
const PTY_CAPACITY: usize = 4096;

pub(crate) struct Terminal {
    exclusive: Rc<Cell<bool>>,
    stopped: Rc<Cell<bool>>,
    resume: Option<Handle<Event>>,
    backend: Backend,
    state: Rc<RefCell<State>>,
    control: Rc<Cell<Option<(usize, usize)>>>,
    input: RefCell<Option<Input>>,
}

enum Backend {
    Console {
        output: devmgr::uart::Session,
        source: Option<devmgr::uart::Session>,
    },
    Pty {
        pair: Rc<PtyPair>,
        side: Side,
    },
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Side {
    Master,
    Slave,
}

/// The two ends and the shared line discipline of a pseudo-terminal.
pub(crate) struct PtyPair {
    id: u32,
    unlocked: Cell<bool>,
    exclusive: Rc<Cell<bool>>,
    stopped: Rc<Cell<bool>>,
    state: Rc<RefCell<State>>,
    control: Rc<Cell<Option<(usize, usize)>>>,
    open: RefCell<PtyOpen>,
}

#[derive(Default)]
struct PtyOpen {
    master: bool,
    slaves: usize,
    slave_opened: bool,
}

/// Allocates pseudo-terminal numbers and tracks live pairs.
#[derive(Default)]
pub(crate) struct PtyRegistry {
    entries: RefCell<BTreeMap<u32, Weak<PtyPair>>>,
}

impl PtyRegistry {
    pub fn create(&self) -> Result<Rc<PtyPair>, ()> {
        let mut entries = self.entries.borrow_mut();
        entries.retain(|_, pair| pair.strong_count() != 0);
        let mut id = 0;
        while entries.contains_key(&id) {
            id = id.checked_add(1).ok_or(())?;
        }
        let pair = Rc::new(PtyPair::new(id));
        entries.insert(id, Rc::downgrade(&pair));
        Ok(pair)
    }

    pub fn slave(&self, id: u32) -> Option<Rc<PtyPair>> {
        self.entries.borrow().get(&id)?.upgrade()
    }

    pub fn live(&self) -> Vec<Rc<PtyPair>> {
        self.entries
            .borrow()
            .values()
            .filter_map(Weak::upgrade)
            .collect()
    }
}

impl PtyPair {
    fn new(id: u32) -> Self {
        Self {
            id,
            unlocked: Cell::new(false),
            exclusive: Rc::default(),
            stopped: Rc::default(),
            state: Rc::new(RefCell::new(State::new())),
            control: Rc::default(),
            open: RefCell::new(PtyOpen::default()),
        }
    }

    pub fn number(&self) -> u32 {
        self.id
    }

    pub fn unlocked(&self) -> bool {
        self.unlocked.get()
    }

    pub fn set_unlocked(&self, value: bool) {
        self.unlocked.set(value);
    }

    pub fn control(&self) -> Option<(usize, usize)> {
        self.control.get()
    }

    pub fn take_signal(&self) -> Option<u8> {
        self.state.borrow_mut().signals.pop_front()
    }

    fn master_closed(&self) -> bool {
        !self.open.borrow().master
    }

    fn slave_closed(&self) -> bool {
        let open = self.open.borrow();
        open.slave_opened && open.slaves == 0
    }

    fn input_free(&self) -> usize {
        let state = self.state.borrow();
        let used = state.line.len()
            + state
                .ready
                .iter()
                .map(|line| line.len().max(1))
                .sum::<usize>();
        PTY_CAPACITY.saturating_sub(used)
    }

    fn master_free(&self) -> usize {
        PTY_CAPACITY.saturating_sub(self.state.borrow().master.len())
    }

    fn write_master(&self, bytes: &[u8]) {
        let mut state = self.state.borrow_mut();
        let space = PTY_CAPACITY - state.master.len();
        state.master.extend(bytes.iter().copied().take(space));
    }
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
    master: VecDeque<u8>,
}

impl State {
    fn new() -> Self {
        // REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/asm-generic/termbits.h?h=v6.12
        let mut termios = [0; 36];
        termios[..4].copy_from_slice(&0x500u32.to_le_bytes());
        termios[4..8].copy_from_slice(&5u32.to_le_bytes());
        termios[8..12].copy_from_slice(&0xbfu32.to_le_bytes());
        termios[12..16].copy_from_slice(&0x823bu32.to_le_bytes());
        termios[17..34]
            .copy_from_slice(&[3, 28, 127, 21, 4, 0, 1, 0, 17, 19, 26, 0, 18, 15, 23, 22, 0]);
        Self {
            termios,
            window: [24, 0, 80, 0, 0, 0, 0, 0],
            ready: VecDeque::new(),
            line: Vec::new(),
            failed: false,
            signals: VecDeque::new(),
            received: 0,
            literal: false,
            master: VecDeque::new(),
        }
    }
}

pub(crate) struct Input {
    request: devmgr::uart::PendingRead,
    echo_request: Option<devmgr::device::Pending<usize, usize>>,
    echo: VecDeque<u8>,
}

impl Terminal {
    pub fn new(
        task: &Handle<runtime::Task>,
        output: devmgr::uart::Session,
        input: Option<devmgr::uart::Session>,
    ) -> Result<Self, runtime::Error> {
        Ok(Self {
            exclusive: Rc::default(),
            stopped: Rc::default(),
            resume: Some(task.event(Rights::WAIT | Rights::SIGNAL)?),
            backend: Backend::Console {
                output,
                source: input,
            },
            state: Rc::new(RefCell::new(State::new())),
            control: Rc::default(),
            input: RefCell::new(None),
        })
    }

    pub fn pty(pair: Rc<PtyPair>, side: Side) -> Self {
        match side {
            Side::Master => pair.open.borrow_mut().master = true,
            Side::Slave => {
                let mut open = pair.open.borrow_mut();
                open.slaves += 1;
                open.slave_opened = true;
            }
        }
        Self {
            exclusive: Rc::clone(&pair.exclusive),
            stopped: Rc::clone(&pair.stopped),
            resume: None,
            state: Rc::clone(&pair.state),
            control: Rc::clone(&pair.control),
            backend: Backend::Pty { pair, side },
            input: RefCell::new(None),
        }
    }

    pub fn is_pty(&self) -> bool {
        matches!(self.backend, Backend::Pty { .. })
    }

    pub fn is_master(&self) -> bool {
        matches!(
            self.backend,
            Backend::Pty {
                side: Side::Master,
                ..
            }
        )
    }

    pub fn is_slave(&self) -> bool {
        matches!(
            self.backend,
            Backend::Pty {
                side: Side::Slave,
                ..
            }
        )
    }

    pub fn failed(&self) -> bool {
        if self.state.borrow().failed {
            return true;
        }
        match &self.backend {
            Backend::Console { .. } => false,
            Backend::Pty { pair, side } => match side {
                Side::Master => pair.slave_closed(),
                Side::Slave => pair.master_closed(),
            },
        }
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
        matches!(&self.backend, Backend::Console { source, .. } if source.is_some())
    }

    pub fn pty_number(&self) -> Option<u32> {
        match &self.backend {
            Backend::Pty { pair, .. } => Some(pair.number()),
            Backend::Console { .. } => None,
        }
    }

    pub fn set_unlocked(&self, value: bool) -> Option<()> {
        match &self.backend {
            Backend::Pty { pair, .. } => {
                pair.set_unlocked(value);
                Some(())
            }
            Backend::Console { .. } => None,
        }
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

    /// Discards queued pseudo-terminal data for one or both directions.
    ///
    /// `queue` follows `TCFLSH`: zero clears input, one clears output, and any
    /// other value clears both.
    pub fn flush(&self, queue: u32) {
        if !self.is_pty() {
            self.set_termios(self.termios(), true);
            return;
        }
        let mut state = self.state.borrow_mut();
        let input = queue != 1;
        let output = queue != 0;
        if input {
            if self.is_master() {
                state.master.clear();
            } else {
                state.line.clear();
                state.ready.clear();
            }
        }
        if output {
            if self.is_master() {
                state.line.clear();
                state.ready.clear();
            } else {
                state.master.clear();
            }
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
        let state = self.state.borrow();
        if self.is_master() {
            state.master.len()
        } else {
            state.ready.iter().map(VecDeque::len).sum()
        }
    }

    pub fn received(&self) -> u64 {
        self.state.borrow().received
    }

    pub fn take_signal(&self) -> Option<u8> {
        self.state.borrow_mut().signals.pop_front()
    }

    pub fn readable(&self) -> bool {
        let state = self.state.borrow();
        if self.is_master() {
            return !state.master.is_empty();
        }
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
        if self.is_master() {
            return state.master.iter().copied().take(length).collect();
        }
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
        if self.is_master() {
            let count = length.min(state.master.len());
            state.master.drain(..count);
            return;
        }
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
        let Backend::Console {
            source: Some(source),
            ..
        } = &self.backend
        else {
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
                    request,
                    echo_request: None,
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
        self.translate(bytes, devmgr::uart::MAX_BYTES)
    }

    /// Accepts bytes written to either pseudo-terminal end.
    pub fn write_pty(&self, bytes: &[u8]) -> usize {
        let Backend::Pty { pair, side } = &self.backend else {
            return 0;
        };
        match side {
            Side::Master => {
                let mut count = 0;
                for &byte in bytes {
                    let (canonical, line_length) = {
                        let state = self.state.borrow();
                        (state.termios[12] & 2 != 0, state.line.len())
                    };
                    if pair.input_free() == 0 {
                        break;
                    }
                    if canonical && line_length != 0 && pair.input_free() == 1 {
                        let state = self.state.borrow();
                        let control =
                            byte == b'\n' || byte == b'\r' || state.termios[17..].contains(&byte);
                        if !control {
                            count += 1;
                            continue;
                        }
                    }
                    let echo = self.receive(alloc::vec![byte]);
                    pair.write_master(&echo);
                    count += 1;
                }
                count
            }
            Side::Slave => {
                let (output, count) = self.translate(bytes, pair.master_free());
                pair.write_master(&output);
                count
            }
        }
    }

    fn translate(&self, bytes: &[u8], limit: usize) -> (Vec<u8>, usize) {
        let flags = u32::from_le_bytes(self.state.borrow().termios[4..8].try_into().unwrap());
        let mut output = Vec::new();
        let mut count = 0;
        for &byte in bytes {
            let newline = flags & 5 == 5 && byte == b'\n';
            if output.len() + 1 + usize::from(newline) > limit {
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
        if let Some(resume) = &self.resume {
            let _ = resume.signal(1);
        }
    }

    pub fn resume_event(&self) -> Option<&Handle<Event>> {
        self.resume.as_ref()
    }

    pub fn exclusive(&self) -> bool {
        self.exclusive.get()
    }
    pub fn set_exclusive(&self, value: bool) {
        self.exclusive.set(value);
    }
    pub fn writable(&self) -> bool {
        match &self.backend {
            Backend::Console { .. } => !self.stopped(),
            Backend::Pty {
                pair,
                side: Side::Master,
            } => pair.input_free() != 0,
            Backend::Pty {
                pair,
                side: Side::Slave,
            } => !self.stopped() && !pair.master_closed() && pair.master_free() >= 2,
        }
    }

    pub fn output_pending(&self) -> bool {
        match &self.backend {
            Backend::Console { .. } => false,
            Backend::Pty {
                side: Side::Slave, ..
            } => !self.state.borrow().master.is_empty(),
            Backend::Pty {
                side: Side::Master, ..
            } => false,
        }
    }
    pub fn inject(&self, byte: u8) -> Result<(), devmgr::Error> {
        let echo = self.receive(alloc::vec![byte]);
        if let Backend::Pty { pair, .. } = &self.backend {
            pair.write_master(&echo);
            return Ok(());
        }
        if echo.is_empty() {
            return Ok(());
        }
        let mut input = self.input.borrow_mut();
        if let Some(current) = input.as_mut() {
            current.echo.extend(echo);
            return Ok(());
        }
        let Backend::Console { output, source } = &self.backend else {
            unreachable!()
        };
        let Some(source) = source else {
            output.write_all(&echo)?;
            output.flush()?;
            return Ok(());
        };
        *input = Some(Input {
            request: source.start_read(devmgr::uart::MAX_BYTES)?,
            echo_request: None,
            echo: echo.into(),
        });
        Ok(())
    }

    pub fn uart_output(&self) -> &devmgr::uart::Session {
        match &self.backend {
            Backend::Console { output, .. } => output,
            Backend::Pty { .. } => unreachable!(),
        }
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

impl Drop for Terminal {
    fn drop(&mut self) {
        let Backend::Pty { pair, side } = &self.backend else {
            return;
        };
        let mut open = pair.open.borrow_mut();
        let closed = match side {
            Side::Master => {
                open.master = false;
                true
            }
            Side::Slave => {
                open.slaves = open.slaves.saturating_sub(1);
                false
            }
        };
        drop(open);
        if closed {
            let mut state = pair.state.borrow_mut();
            // Closing the master hangs up the slave: discard its pending input
            // so blocked readers observe end-of-file rather than stale data.
            state.line.clear();
            state.ready.clear();
            for number in [1, crate::process::signal::CONT] {
                state.signals.push_back(number);
            }
        }
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
        self.request.call()
    }

    pub fn echo_call(&self) -> Option<&Handle<Call>> {
        Some(self.echo_request.as_ref()?.call())
    }

    fn poll(
        &mut self,
        terminal: &Terminal,
        source: &devmgr::uart::Session,
    ) -> Result<bool, devmgr::Error> {
        let mut progress = false;
        match self.request.try_complete() {
            Ok((bytes, _)) => {
                self.echo.extend(terminal.receive(bytes));
                self.request = source.start_read(devmgr::uart::MAX_BYTES)?;
                progress = true;
            }
            Err(devmgr::Error::Runtime(runtime::Error::Kernel(
                runtime::KernelError::WouldBlock,
            ))) => {}
            Err(error) => return Err(error),
        }
        if let Some(request) = &mut self.echo_request {
            match request.try_complete() {
                Ok(count) => {
                    if count == 0 || count > self.echo.len().min(devmgr::uart::MAX_BYTES) {
                        return Err(devmgr::Error::Runtime(runtime::Error::Protocol));
                    }
                    self.echo.drain(..count);
                    self.echo_request = None;
                    progress = true;
                }
                Err(devmgr::Error::Runtime(runtime::Error::Kernel(
                    runtime::KernelError::WouldBlock,
                ))) => {}
                Err(error) => return Err(error),
            }
        }
        if self.echo_request.is_none() && !self.echo.is_empty() {
            let bytes: Vec<_> = self
                .echo
                .iter()
                .copied()
                .take(devmgr::uart::MAX_BYTES)
                .collect();
            self.echo_request = Some(terminal.uart_output().start_write(&bytes)?);
            progress = true;
        }
        Ok(progress)
    }
}
