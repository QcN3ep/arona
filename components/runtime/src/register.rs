// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Register group access and convenience state for supported native ISAs.

use crate::{BorrowedHandle, Error, Object};
pub use abi::register::{ScalarFloat, Write};
use alloc::vec::Vec;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserState {
    pub pc: usize,
    pub registers: [usize; 32],
    pub floating: Option<ScalarFloat>,
}

impl UserState {
    pub fn general(&self) -> abi::register::General {
        abi::register::General {
            pc: self.pc as u64,
            registers: self.registers.map(|value| value as u64),
        }
    }
    pub fn from_general(general: abi::register::General) -> Self {
        Self {
            pc: general.pc as usize,
            registers: general.registers.map(|value| value as usize),
            floating: None,
        }
    }

    pub fn with_groups<T>(
        &self,
        float_size: usize,
        apply: impl FnOnce(&[Write<'_>]) -> T,
    ) -> Result<T, Error> {
        let general = self.general().encode();
        let floating = self
            .floating
            .as_ref()
            .map(|floating| floating.encode(float_size))
            .transpose()
            .map_err(Error::Kernel)?;
        let mut groups = arrayvec::ArrayVec::<Write<'_>, 2>::new();
        groups.push(Write {
            group: abi::register::GENERAL,
            bytes: &general,
        });
        if let Some((bytes, length)) = &floating {
            groups.push(Write {
                group: abi::register::FLOAT,
                bytes: &bytes[..*length],
            });
        }
        Ok(apply(&groups))
    }
}

pub(crate) fn float_size() -> Result<usize, Error> {
    #[cfg(target_arch = "riscv64")]
    {
        Ok(ScalarFloat::RISCV_SIZE)
    }
    #[cfg(target_arch = "loongarch64")]
    {
        Ok(ScalarFloat::LOONGARCH_SIZE)
    }
    #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
    {
        Err(Error::Kernel(abi::Error::Unsupported))
    }
}

pub(crate) fn requests(
    groups: &[Write<'_>],
) -> Result<Vec<[u8; abi::register::WriteRequest::SIZE]>, Error> {
    let mut records = Vec::new();
    records
        .try_reserve_exact(groups.len())
        .map_err(|_| Error::Kernel(abi::Error::NoMemory))?;
    records.extend(groups.iter().map(|group| {
        abi::register::WriteRequest {
            group: group.group,
            address: group.bytes.as_ptr() as u64,
            length: group.bytes.len() as u64,
        }
        .encode()
    }));
    Ok(records)
}

pub(crate) fn state<T: Object>(
    handle: BorrowedHandle<'_, T>,
    topic: u64,
    floating: bool,
) -> Result<UserState, Error> {
    let query = abi::register::GENERAL.to_le_bytes();
    let mut bytes = [0; abi::register::General::SIZE];
    // SAFETY: The query stays borrowed and the output buffer stays exclusively borrowed for the call.
    let [written, _] = unsafe {
        handle.invoke(
            abi::method::INFO,
            [
                topic,
                query.as_ptr() as u64,
                query.len() as u64,
                bytes.as_mut_ptr() as u64,
                bytes.len() as u64,
            ],
        )
    }?;
    if written != bytes.len() as u64 {
        return Err(Error::Protocol);
    }
    let mut state = UserState::from_general(abi::register::General::decode(&bytes));
    if floating {
        state.floating = Some(read_floating(handle, topic)?);
    }
    Ok(state)
}

pub(crate) fn read_floating<T: Object>(
    handle: BorrowedHandle<'_, T>,
    topic: u64,
) -> Result<ScalarFloat, Error> {
    let query = abi::register::FLOAT.to_le_bytes();
    let mut bytes = [0; ScalarFloat::LOONGARCH_SIZE];
    // SAFETY: The query stays borrowed and the output buffer stays exclusively borrowed for the call.
    let [written, _] = unsafe {
        handle.invoke(
            abi::method::INFO,
            [
                topic,
                query.as_ptr() as u64,
                query.len() as u64,
                bytes.as_mut_ptr() as u64,
                bytes.len() as u64,
            ],
        )
    }?;
    let written = usize::try_from(written).map_err(|_| Error::Protocol)?;
    let bytes = bytes.get(..written).ok_or(Error::Protocol)?;
    ScalarFloat::decode(bytes).map_err(|_| Error::Protocol)
}
