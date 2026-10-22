// SPDX-License-Identifier: MPL-2.0

use arch::boot::dtb::Dtb;

pub fn boot(cpu_id: usize, dtb: usize) -> (u64, Dtb<'static>) {
    let pointer = (0xffff_ffc0_0040_0000 + (dtb & 0x1f_ffff)) as *const u8;
    // SAFETY: 固件 DTB 须完整映射到启动固件窗口且保持不变。
    (cpu_id as u64, unsafe { Dtb::new(pointer) }.unwrap())
}
