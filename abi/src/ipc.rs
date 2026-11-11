// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use crate::{Error, Handle, ObjectType, Rights};

pub const MESSAGE_WORDS: usize = 8;
pub const MESSAGE_HANDLES: usize = 8;
pub const READABLE: u64 = 1;
pub const WRITABLE: u64 = 2;
pub const PEER_CLOSED: u64 = 4;
pub const EXITED: u64 = 1;

#[derive(Clone, Copy, Debug)]
pub struct HandleDescriptor {
    pub handle: Handle,
    pub kind: ObjectType,
    pub rights: Rights,
}
#[derive(Clone, Copy)]
pub struct Message {
    pub tag: u64,
    pub words: [u64; MESSAGE_WORDS],
    pub word_count: usize,
    pub handles: [Option<HandleDescriptor>; MESSAGE_HANDLES],
    pub handle_count: usize,
    pub has_reply: bool,
}
impl Message {
    pub const SIZE: usize = 32 + MESSAGE_WORDS * 8 + MESSAGE_HANDLES * 24;
    pub const fn new(tag: u64) -> Self {
        Self {
            tag,
            words: [0; MESSAGE_WORDS],
            word_count: 0,
            handles: [None; MESSAGE_HANDLES],
            handle_count: 0,
            has_reply: false,
        }
    }
    pub fn encode(&self) -> Result<[u8; Self::SIZE], Error> {
        if self.word_count > MESSAGE_WORDS
            || self.handle_count > MESSAGE_HANDLES
            || (self.has_reply && self.handle_count == 0)
        {
            return Err(Error::InvalidArgument);
        }
        let mut words = [0_u64; Self::SIZE / 8];
        words[..4].copy_from_slice(&[
            self.tag,
            self.word_count as u64,
            self.handle_count as u64,
            u64::from(self.has_reply),
        ]);
        words[4..4 + self.word_count].copy_from_slice(&self.words[..self.word_count]);
        for index in 0..self.handle_count {
            let descriptor = self.handles[index].ok_or(Error::InvalidArgument)?;
            words[12 + index * 3..15 + index * 3].copy_from_slice(&[
                descriptor.handle.raw(),
                descriptor.kind as u64,
                descriptor.rights.bits(),
            ]);
        }
        let mut bytes = [0; Self::SIZE];
        for (chunk, word) in bytes.as_chunks_mut::<8>().0.iter_mut().zip(words) {
            *chunk = word.to_le_bytes();
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8; Self::SIZE]) -> Result<Self, Error> {
        let words: [u64; Self::SIZE / 8] =
            core::array::from_fn(|index| u64::from_le_bytes(bytes.as_chunks::<8>().0[index]));
        if words[1] > MESSAGE_WORDS as u64
            || words[2] > MESSAGE_HANDLES as u64
            || words[3] > 1
            || (words[3] == 1 && words[2] == 0)
        {
            return Err(Error::InvalidArgument);
        }
        let mut message = Self::new(words[0]);
        message.word_count = words[1] as usize;
        message.handle_count = words[2] as usize;
        message.has_reply = words[3] != 0;
        message.words.copy_from_slice(&words[4..12]);
        for index in 0..message.handle_count {
            let first = 12 + index * 3;
            message.handles[index] = Some(HandleDescriptor {
                handle: Handle::from_raw(words[first]),
                kind: ObjectType::from_raw(words[first + 1]).ok_or(Error::InvalidArgument)?,
                rights: Rights::from_bits(words[first + 2]).ok_or(Error::InvalidArgument)?,
            });
        }
        if message.has_reply && message.handles[0].unwrap().kind != ObjectType::Reply {
            return Err(Error::InvalidArgument);
        }
        Ok(message)
    }
}

#[derive(Clone, Copy)]
pub struct WaitItem {
    pub handle: Handle,
    pub conditions: u64,
}
impl WaitItem {
    pub const SIZE: usize = 16;
    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        bytes[..8].copy_from_slice(&self.handle.raw().to_le_bytes());
        bytes[8..].copy_from_slice(&self.conditions.to_le_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8; Self::SIZE]) -> Self {
        Self {
            handle: Handle::from_raw(u64::from_le_bytes(bytes[..8].try_into().unwrap())),
            conditions: u64::from_le_bytes(bytes[8..].try_into().unwrap()),
        }
    }
}
