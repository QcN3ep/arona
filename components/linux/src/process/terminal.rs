// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Controlling terminal association, foreground process group, and background access.

use super::{
    namespace::PidNamespace,
    signal::{self, Info, bit},
};
use crate::{
    file::terminal::Terminal,
    syscall::abi::{EIO, ENOTTY, ENXIO, EPERM, ESRCH, Errno},
};
use alloc::rc::Rc;

#[derive(Clone, Copy)]
pub(crate) enum Access {
    Read,
    Write,
    Change,
}

impl PidNamespace {
    pub fn controlling_terminal(&self, pid: usize) -> Result<Rc<Terminal>, Errno> {
        self.processes
            .borrow()
            .get(&pid)
            .and_then(|record| record.terminal.clone())
            .ok_or(ENXIO)
    }

    pub fn attach_terminal(&self, pid: usize, terminal: &Rc<Terminal>) -> Result<(), Errno> {
        let mut records = self.processes.borrow_mut();
        let record = records.get_mut(&pid).ok_or(ESRCH)?;
        if record.session != pid
            || record
                .terminal
                .as_ref()
                .is_some_and(|current| !Rc::ptr_eq(current, terminal))
            || terminal
                .control()
                .is_some_and(|(session, _)| session != record.session)
        {
            return Err(EPERM);
        }
        if terminal.control().is_none() {
            terminal.set_control(Some((record.session, record.group)));
        }
        record.terminal = Some(Rc::clone(terminal));
        Ok(())
    }

    pub fn foreground(&self, pid: usize, terminal: &Rc<Terminal>) -> Result<(usize, usize), Errno> {
        let associated = self
            .processes
            .borrow()
            .get(&pid)
            .and_then(|record| record.terminal.clone());
        if !associated.is_some_and(|current| Rc::ptr_eq(&current, terminal)) {
            return Err(ENOTTY);
        }
        terminal.control().ok_or(ENOTTY)
    }

    pub fn set_foreground(
        &self,
        pid: usize,
        terminal: &Rc<Terminal>,
        group: usize,
    ) -> Result<(), Errno> {
        let (session, _) = self.foreground(pid, terminal)?;
        if !self
            .processes
            .borrow()
            .values()
            .any(|record| record.group == group && record.session == session)
        {
            return Err(EPERM);
        }
        terminal.set_control(Some((session, group)));
        Ok(())
    }

    pub fn detach_terminal(&self, pid: usize, terminal: &Rc<Terminal>) -> Result<(), Errno> {
        let (session, _) = self.foreground(pid, terminal)?;
        if session == pid {
            self.hangup_terminal(terminal);
        } else if let Some(record) = self.processes.borrow_mut().get_mut(&pid) {
            record.terminal = None;
        }
        Ok(())
    }

    pub fn release_terminal(&self, pid: usize) {
        if let Ok(terminal) = self.controlling_terminal(pid)
            && terminal
                .control()
                .is_some_and(|(session, _)| session == pid)
        {
            self.hangup_terminal(&terminal);
        }
    }

    pub fn hangup_terminal(&self, terminal: &Rc<Terminal>) -> bool {
        let Some((session, group)) = terminal.control() else {
            return false;
        };
        terminal.set_control(None);
        for record in self.processes.borrow_mut().values_mut() {
            if record
                .terminal
                .as_ref()
                .is_some_and(|current| Rc::ptr_eq(current, terminal))
            {
                record.terminal = None;
            }
        }
        for number in [1, signal::CONT] {
            self.signal_group(session, group, number);
        }
        true
    }

    pub fn signal_group(&self, session: usize, group: usize, number: u8) {
        let members: alloc::vec::Vec<_> = self
            .processes
            .borrow()
            .iter()
            .filter_map(|(&pid, record)| {
                (record.session == session && record.group == group && record.status.is_none())
                    .then_some(pid)
            })
            .collect();
        for pid in members {
            let _ = self.signal_process(
                pid,
                Info {
                    number,
                    code: 128,
                    sender: 0,
                    uid: 0,
                    address: 0,
                    value: 0,
                },
            );
        }
    }

    // REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/drivers/tty/tty_jobctrl.c?h=v6.12
    pub fn terminal_access(
        &self,
        pid: usize,
        tid: usize,
        terminal: &Rc<Terminal>,
        access: Access,
    ) -> Result<bool, Errno> {
        if matches!(access, Access::Write) && terminal.termios()[13] & 1 == 0 {
            return Ok(true);
        }
        let Ok((session, foreground)) = self.foreground(pid, terminal) else {
            return Ok(true);
        };
        let group = self.group(pid);
        if group == foreground {
            return Ok(true);
        }
        let read = matches!(access, Access::Read);
        let number = if read { 21 } else { 22 };
        let ignored = self.processes.borrow().get(&pid).is_some_and(|record| {
            record.signals.borrow().actions[usize::from(number) - 1].handler == 1
        });
        let blocked = self
            .threads
            .borrow()
            .get(&tid)
            .and_then(|thread| thread.signals.upgrade())
            .is_some_and(|signals| signals.borrow().mask & bit(number) != 0);
        if ignored || blocked {
            return if read { Err(EIO) } else { Ok(true) };
        }
        if self.orphaned(group) {
            return Err(EIO);
        }
        self.signal_group(session, group, number);
        Ok(false)
    }
}
