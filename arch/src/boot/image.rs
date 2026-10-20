// SPDX-License-Identifier: MPL-2.0

//! 读取链接脚本提供的内核映像布局。

use kernel::{ImageSegment, KernelImage, PhysRange};

unsafe extern "C" {
    static _kernel_image: [u64; 2];
    static _kernel_text: [u64; 3];
    static _kernel_rodata: [u64; 3];
    static _kernel_data: [u64; 3];
    static _kernel_boot_tables: [u64; 2];
    static _kernel_boot_stack: [u64; 2];
}

pub fn kernel_image() -> KernelImage {
    // SAFETY: 各符号须对应链接脚本生成的只读物理范围或分段描述。
    unsafe {
        KernelImage {
            physical: range(_kernel_image),
            text: segment(_kernel_text),
            rodata: segment(_kernel_rodata),
            data: segment(_kernel_data),
            boot_tables: range(_kernel_boot_tables),
            boot_stack: range(_kernel_boot_stack),
        }
    }
}

fn range([start, end]: [u64; 2]) -> PhysRange {
    PhysRange { start, end }
}

fn segment([start, end, virtual_start]: [u64; 3]) -> ImageSegment {
    ImageSegment {
        physical: PhysRange { start, end },
        virtual_start: virtual_start as usize,
    }
}
