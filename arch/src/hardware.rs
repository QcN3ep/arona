// SPDX-License-Identifier: MPL-2.0

use core::mem::{align_of, size_of};

// 空数组确保没有能力声明时，链接段及其边界仍然存在。
#[used]
#[unsafe(link_section = "arona_hardware")]
static ANCHOR: [fn(); 0] = [];

unsafe extern "C" {
    static __start_arona_hardware: u8;
    static __stop_arona_hardware: u8;
}

/// 遍历静态绑定记录。
pub fn bind() {
    let start = core::ptr::addr_of!(__start_arona_hardware);
    let end = core::ptr::addr_of!(__stop_arona_hardware);
    let length = (end as usize)
        .checked_sub(start as usize)
        .expect("invalid hardware binding boundaries");
    assert!(
        (start as usize).is_multiple_of(align_of::<fn()>())
            && length.is_multiple_of(size_of::<fn()>()),
        "invalid hardware binding layout"
    );
    // SAFETY: 边界须覆盖连续、按函数指针对齐且永久只读的 fn() 记录。
    let bindings =
        unsafe { core::slice::from_raw_parts(start.cast::<fn()>(), length / size_of::<fn()>()) };
    for bind in bindings {
        bind();
    }
}

/// 由链接器收集绑定记录。
#[macro_export]
macro_rules! declare_hardware {
    ($visibility:vis static $name:ident: $ty:ty = $value:expr => $bind:path $(;)?) => {
        $visibility static $name: $ty = $value;
        const _: () = {
            #[used]
            #[unsafe(link_section = "arona_hardware")]
            static BIND: fn() = || {
                $bind(&$name).expect(concat!("hardware capability already bound: ", stringify!($name)));
            };
        };
    };
}
