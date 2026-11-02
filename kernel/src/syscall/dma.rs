// SPDX-License-Identifier: MPL-2.0

use crate::{
    device::dma::DmaDomain,
    task::{ObjectReference, Task},
};
use abi::{Error, Handle, ObjectType, dma::DmaDirection, method::dma};

pub(super) fn invoke(
    caller: &Task,
    target: &DmaDomain,
    method: u64,
    args: [u64; 5],
) -> Result<[u64; 2], Error> {
    match method {
        dma::STATE => Ok([target.state()? as u64, 0]),
        dma::MAP => {
            let direction = DmaDirection::from_raw(args[3]).ok_or(Error::InvalidArgument)?;
            let ObjectReference::FrameAllocation(allocation) = caller.get(
                Handle::from_raw(args[0]),
                ObjectType::FrameAllocation,
                direction.required_rights(),
            )?
            else {
                unreachable!();
            };
            let frames = args[1]..args[1].checked_add(args[2]).ok_or(Error::InvalidArgument)?;
            let bus = target.map(&allocation, frames, direction)?;
            Ok([bus.start, bus.end - bus.start])
        }
        dma::UNMAP => {
            target.unmap(args[0])?;
            Ok([0; 2])
        }
        dma::RESUME => {
            target.resume()?;
            Ok([0; 2])
        }
        dma::QUIESCE => {
            target.quiesce()?;
            Ok([0; 2])
        }
        dma::REVOKE => {
            target.revoke()?;
            Ok([0; 2])
        }
        _ => Err(Error::Unsupported),
    }
}
