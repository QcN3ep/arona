// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UserEntry {
    pub pc: u64,
    pub stack_pointer: u64,
    pub thread_pointer: u64,
    pub argument: u64,
}
impl UserEntry {
    pub const SIZE: usize = 32;
    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        for (chunk, word) in bytes.as_chunks_mut::<8>().0.iter_mut().zip([
            self.pc,
            self.stack_pointer,
            self.thread_pointer,
            self.argument,
        ]) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        bytes
    }
    pub fn decode(bytes: &[u8; Self::SIZE]) -> Self {
        let word =
            |index: usize| u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().unwrap());
        Self {
            pc: word(0),
            stack_pointer: word(1),
            thread_pointer: word(2),
            argument: word(3),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum Lifecycle {
    Live = 0,
    Exited = 1,
}
