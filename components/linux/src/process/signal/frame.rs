// SPDX-License-Identifier: MPL-2.0

//! Linux 信号帧的寄存器保存与恢复。

use super::{Action, AltStack, Info, ONSTACK};
use crate::{
    elf::Machine,
    memory::AddressSpace,
    syscall::abi::{EFAULT, EINVAL, Errno},
};
use runtime::trap::UserState;

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/arch/riscv/include/uapi/asm/ucontext.h?h=v6.12
// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/arch/loongarch/include/uapi/asm/sigcontext.h?h=v6.12
const UCONTEXT: usize = 128;
const MASK: usize = UCONTEXT + 40;
const CONTEXT: usize = UCONTEXT + 176;
const FPU_MAGIC: u32 = 0x46505501;

pub struct Restored {
    pub state: UserState,
    pub mask: u64,
    pub altstack: AltStack,
}

pub fn stack_pointer(machine: Machine, state: &UserState) -> usize {
    state.registers[stack_register(machine)]
}

fn stack_register(machine: Machine) -> usize {
    match machine {
        Machine::Riscv64 => 2,
        Machine::LoongArch64 => 3,
    }
}
fn size(machine: Machine) -> usize {
    match machine {
        Machine::Riscv64 => 1088,
        Machine::LoongArch64 => 880,
    }
}

pub struct Delivery {
    pub info: Info,
    pub action: Action,
    pub mask: u64,
    pub altstack: AltStack,
    pub trampoline: usize,
}

pub fn install(
    space: &AddressSpace,
    machine: Machine,
    state: &mut UserState,
    delivery: Delivery,
) -> Result<(), Errno> {
    let Delivery {
        info,
        action,
        mask,
        altstack,
        trampoline,
    } = delivery;
    let old_sp = stack_pointer(machine, state);
    let alternate = action.flags & ONSTACK != 0 && !altstack.disabled && !altstack.contains(old_sp);
    let top = if alternate {
        altstack.address.checked_add(altstack.size).ok_or(EFAULT)?
    } else {
        old_sp
    };
    let base = top.checked_sub(size(machine)).ok_or(EFAULT)? & !15;
    if alternate && base < altstack.address {
        return Err(EFAULT);
    }
    let mut bytes = [0; 1088];
    bytes[..128].copy_from_slice(&info.encode());
    word(&mut bytes, UCONTEXT + 16, altstack.address);
    let flags = if altstack.disabled {
        2u32
    } else {
        u32::from(altstack.contains(old_sp))
    };
    bytes[UCONTEXT + 24..UCONTEXT + 28].copy_from_slice(&flags.to_le_bytes());
    word(&mut bytes, UCONTEXT + 32, altstack.size);
    word(&mut bytes, MASK, mask as usize);
    encode(machine, state, &mut bytes);
    space
        .write(base, &bytes[..size(machine)])
        .map_err(|_| EFAULT)?;
    let first = match machine {
        Machine::Riscv64 => 10,
        Machine::LoongArch64 => 4,
    };
    state.pc = action.handler;
    state.registers[1] = trampoline;
    state.registers[stack_register(machine)] = base;
    state.registers[first..first + 3].copy_from_slice(&[
        info.number as usize,
        base,
        base + UCONTEXT,
    ]);
    Ok(())
}

fn encode(machine: Machine, state: &UserState, bytes: &mut [u8]) {
    word(bytes, CONTEXT, state.pc);
    let (registers, floating, control) = match machine {
        Machine::Riscv64 => {
            for (index, value) in state.registers.iter().enumerate().skip(1) {
                word(bytes, CONTEXT + index * 8, *value);
            }
            (None, CONTEXT + 256, CONTEXT + 512)
        }
        Machine::LoongArch64 => {
            bytes[CONTEXT + 264..CONTEXT + 268].copy_from_slice(&1u32.to_le_bytes());
            bytes[576..580].copy_from_slice(&FPU_MAGIC.to_le_bytes());
            bytes[580..584].copy_from_slice(&288u32.to_le_bytes());
            bytes[848..856].copy_from_slice(&state.floating_conditions);
            (Some(CONTEXT + 8), 592, 856)
        }
    };
    if let Some(offset) = registers {
        for (index, value) in state.registers.iter().enumerate() {
            word(bytes, offset + index * 8, *value);
        }
    }
    for (index, value) in state.floating.iter().enumerate() {
        bytes[floating + index * 8..floating + index * 8 + 8].copy_from_slice(&value.to_le_bytes());
    }
    bytes[control..control + 4].copy_from_slice(&state.floating_control.to_le_bytes());
}

pub fn restore(space: &AddressSpace, machine: Machine, address: usize) -> Result<Restored, Errno> {
    if address & 15 != 0 {
        return Err(EFAULT);
    }
    let mut bytes = [0; 1088];
    space
        .read(address, &mut bytes[..size(machine)])
        .map_err(|_| EFAULT)?;
    let mut state = UserState {
        pc: read(&bytes, CONTEXT),
        ..UserState::default()
    };
    let (floating, control) = match machine {
        Machine::Riscv64 => {
            for (index, value) in state.registers.iter_mut().enumerate().skip(1) {
                *value = read(&bytes, CONTEXT + index * 8);
            }
            if read(&bytes, 1080) != 0 {
                return Err(EINVAL);
            }
            (CONTEXT + 256, CONTEXT + 512)
        }
        Machine::LoongArch64 => {
            for (index, value) in state.registers.iter_mut().enumerate() {
                *value = read(&bytes, CONTEXT + 8 + index * 8);
            }
            if u32::from_le_bytes(bytes[576..580].try_into().unwrap()) != FPU_MAGIC
                || u32::from_le_bytes(bytes[580..584].try_into().unwrap()) != 288
                || read(&bytes, 864) != 0
            {
                return Err(EINVAL);
            }
            state.floating_conditions.copy_from_slice(&bytes[848..856]);
            (592, 856)
        }
    };
    for (index, value) in state.floating.iter_mut().enumerate() {
        *value = read(&bytes, floating + index * 8) as u64;
    }
    state.floating_control = u32::from_le_bytes(bytes[control..control + 4].try_into().unwrap());
    let flags = u32::from_le_bytes(bytes[UCONTEXT + 24..UCONTEXT + 28].try_into().unwrap());
    if flags & !3 != 0 {
        return Err(EINVAL);
    }
    let altstack = AltStack {
        address: read(&bytes, UCONTEXT + 16),
        size: read(&bytes, UCONTEXT + 32),
        disabled: flags & 2 != 0,
    };
    altstack.address.checked_add(altstack.size).ok_or(EFAULT)?;
    Ok(Restored {
        state,
        mask: read(&bytes, MASK) as u64,
        altstack,
    })
}
fn word(bytes: &mut [u8], offset: usize, value: usize) {
    bytes[offset..offset + 8].copy_from_slice(&(value as u64).to_le_bytes());
}
fn read(bytes: &[u8], offset: usize) -> usize {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap()) as usize
}

pub fn trampoline(machine: Machine) -> [u8; 8] {
    let code: [u32; 2] = match machine {
        Machine::Riscv64 => [0x08b00893, 0x00000073],
        Machine::LoongArch64 => [0x02c0000b | (139 << 10), 0x002b0000],
    };
    let mut bytes = [0; 8];
    for (part, instruction) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(code) {
        *part = instruction.to_le_bytes();
    }
    bytes
}
