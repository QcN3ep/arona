// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! State comparisons over the real context and continuation register interfaces.

use kernel::{
    hardware::{
        self,
        execution::{Context, ExecutionError, UserEntry},
    },
    trap::port::{ForwardError, TrapContinuationReference},
};
pub use runtime::register::UserState;

fn float_size() -> usize {
    hardware::execution()
        .register_size(abi::register::FLOAT)
        .unwrap()
}

fn read_state<E: From<ExecutionError>>(
    mut read: impl FnMut(u64, &mut [u8]) -> Result<(), E>,
) -> Result<UserState, E> {
    let general = abi::register::GENERAL;
    let floating = abi::register::FLOAT;
    let mut bytes = [0; abi::register::General::SIZE];
    read(general, &mut bytes)?;
    let mut state = UserState::from_general(abi::register::General::decode(&bytes));
    let layout = hardware::execution()
        .register_size(floating)
        .map_err(E::from)?;
    let mut bytes = [0; abi::register::ScalarFloat::LOONGARCH_SIZE];
    let output = bytes
        .get_mut(..layout)
        .ok_or_else(|| E::from(ExecutionError::InvalidLayout))?;
    read(floating, output)?;
    state.floating = Some(
        abi::register::ScalarFloat::decode(output)
            .map_err(|_| E::from(ExecutionError::InvalidState))?,
    );
    Ok(state)
}

pub trait ContextState {
    fn user_state(&self) -> Result<UserState, ExecutionError>;
    fn set_user_state(&self, state: &UserState) -> Result<(), ExecutionError>;
}
impl ContextState for Context {
    fn user_state(&self) -> Result<UserState, ExecutionError> {
        read_state(|group, output| self.read_registers(group, output))
    }
    fn set_user_state(&self, state: &UserState) -> Result<(), ExecutionError> {
        state
            .with_groups(float_size(), |groups| {
                self.set_registers(groups, &mut |_| Ok(()))
            })
            .map_err(|_| ExecutionError::InvalidState)?
    }
}

pub fn read_continuation(
    continuation: &TrapContinuationReference,
) -> Result<UserState, ForwardError> {
    read_state(|group, output| continuation.read_registers(group, output))
}

pub fn resume_continuation(
    continuation: &TrapContinuationReference,
    state: &UserState,
) -> Result<(), ForwardError> {
    state
        .with_groups(float_size(), |groups| continuation.resume_registers(groups))
        .map_err(|_| ExecutionError::InvalidState)?
}

pub fn state_entry(state: &UserState) -> UserEntry {
    let (sp, tp, argument) = if float_size() == abi::register::ScalarFloat::RISCV_SIZE {
        (2, 4, 10)
    } else {
        (3, 2, 4)
    };
    UserEntry {
        pc: state.pc,
        stack_pointer: state.registers[sp],
        thread_pointer: state.registers[tp],
        argument: state.registers[argument],
    }
}
