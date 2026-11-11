// SPDX-License-Identifier: MPL-2.0

//! Signal dispositions, pending delivery queue, and thread signal state.

pub(crate) mod delivery;
pub(crate) mod frame;

use crate::syscall::abi::{ENOMEM, Errno};
use alloc::collections::VecDeque;

pub const KILL: u8 = 9;
pub const STOP: u8 = 19;
pub const CONT: u8 = 18;
pub const RESTART: usize = 0x10000000;
pub const NODEFER: usize = 0x40000000;
pub const RESETHAND: usize = 0x80000000;
pub const ONSTACK: usize = 0x08000000;
pub const UNBLOCKABLE: u64 = bit(KILL) | bit(STOP);

#[derive(Clone, Copy, Default)]
pub struct Action {
    pub handler: usize,
    pub flags: usize,
    pub mask: u64,
}

#[derive(Clone, Copy)]
pub struct Info {
    pub number: u8,
    pub code: i32,
    pub sender: usize,
    pub address: usize,
    pub value: u64,
}

impl Info {
    pub fn encode(self) -> [u8; 128] {
        let mut bytes = [0; 128];
        bytes[..4].copy_from_slice(&u32::from(self.number).to_le_bytes());
        bytes[8..12].copy_from_slice(&self.code.to_le_bytes());
        if self.code > 0 && matches!(self.number, 4 | 5 | 7 | 8 | 11) {
            bytes[16..24].copy_from_slice(&(self.address as u64).to_le_bytes());
        } else {
            bytes[16..20].copy_from_slice(&(self.sender as u32).to_le_bytes());
            bytes[24..32].copy_from_slice(&self.value.to_le_bytes());
        }
        bytes
    }
}

#[derive(Clone, Copy)]
pub struct AltStack {
    pub address: usize,
    pub size: usize,
    pub disabled: bool,
}

impl Default for AltStack {
    fn default() -> Self {
        Self {
            address: 0,
            size: 0,
            disabled: true,
        }
    }
}

impl AltStack {
    pub fn contains(&self, address: usize) -> bool {
        !self.disabled && address >= self.address && address - self.address < self.size
    }
}

#[derive(Default)]
pub struct Queue {
    standard: [Option<Info>; 31],
    realtime: VecDeque<Info>,
}

impl Queue {
    pub fn push(&mut self, info: Info) -> Result<(), Errno> {
        if info.number < 32 {
            self.standard[usize::from(info.number) - 1].get_or_insert(info);
        } else {
            self.realtime.try_reserve(1).map_err(|_| ENOMEM)?;
            self.realtime.push_back(info);
        }
        Ok(())
    }
    pub fn first(&self, mask: u64) -> Option<Info> {
        self.standard
            .iter()
            .flatten()
            .chain(self.realtime.iter())
            .filter(|info| mask & bit(info.number) != 0)
            .min_by_key(|info| info.number)
            .copied()
    }
    pub fn mask(&self) -> u64 {
        self.standard
            .iter()
            .flatten()
            .chain(self.realtime.iter())
            .fold(0, |mask, info| mask | bit(info.number))
    }
    pub fn take(&mut self, mask: u64) -> Option<Info> {
        let info = self.first(mask)?;
        if info.number < 32 {
            self.standard[usize::from(info.number) - 1].take()
        } else {
            let index = self
                .realtime
                .iter()
                .position(|pending| pending.number == info.number)?;
            self.realtime.remove(index)
        }
    }
    pub fn discard(&mut self, number: u8) {
        if number < 32 {
            self.standard[usize::from(number) - 1] = None;
        } else {
            self.realtime.retain(|info| info.number != number);
        }
    }
}

pub struct Signals {
    pub actions: [Action; 64],
    pub pending: Queue,
    pub stopped: bool,
}

impl Default for Signals {
    fn default() -> Self {
        Self {
            actions: [Action::default(); 64],
            pending: Queue::default(),
            stopped: false,
        }
    }
}

impl Signals {
    pub fn fork(&self) -> Self {
        Self {
            actions: self.actions,
            ..Self::default()
        }
    }
    pub fn exec(&mut self) {
        for action in &mut self.actions {
            if action.handler != 1 {
                *action = Action::default();
            }
        }
        self.stopped = false;
    }
}

#[derive(Default)]
pub struct ThreadSignals {
    pub mask: u64,
    pub pending: Queue,
    pub altstack: AltStack,
    pub suspended_mask: Option<u64>,
}

pub const fn bit(number: u8) -> u64 {
    1 << (number - 1)
}
pub fn ignored(number: u8, action: Action) -> bool {
    action.handler == 1 || action.handler == 0 && matches!(number, 17 | CONT | 23 | 28)
}
