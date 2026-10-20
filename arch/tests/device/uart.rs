// SPDX-License-Identifier: MPL-2.0

use core::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

use arch::device::uart::Uart;

#[test]
fn reentrant_output_fails_while_emergency_output_bypasses_the_lock() {
    struct Reentrant<'a>(&'a Uart);
    impl fmt::Display for Reentrant<'_> {
        fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
            assert!(self.0.write(format_args!("nested")).is_err());
            assert!(self.0.flush().is_err());
            self.0.emergency_write(format_args!("panic"))?;
            output.write_str("outer")
        }
    }

    struct Panicking;
    impl fmt::Display for Panicking {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            panic!("formatting failed");
        }
    }

    let mut registers = [0u8; 8];
    registers[5] = 0x20;
    // SAFETY: 寄存器存储须持续有效，且测试期间仅由此实例执行普通访问。
    let uart = unsafe { Uart::new(registers.as_mut_ptr() as usize) };
    uart.init().unwrap();
    assert_eq!(registers[3], 3);
    assert_eq!(registers[1], 0);
    uart.write(format_args!("{}", Reentrant(&uart))).unwrap();
    assert_eq!(registers[0], b'r');
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _ = uart.write(format_args!("{Panicking}"));
        }))
        .is_err()
    );
    uart.write(format_args!("recovered")).unwrap();
    assert_eq!(registers[0], b'd');
}

#[test]
fn unavailable_uart_and_oversized_output_return_errors() {
    let mut registers = [0u8; 8];
    // SAFETY: 寄存器存储须持续有效，且测试期间仅由此实例执行普通访问。
    let uart = unsafe { Uart::new(registers.as_mut_ptr() as usize) };
    uart.init().unwrap();
    assert!(uart.write(format_args!("unavailable")).is_err());
    assert!(uart.emergency_write(format_args!("unavailable")).is_err());
    assert!(uart.flush().is_err());
    assert_eq!(registers[0], 0);

    registers[5] = 0x20;
    assert!(uart.flush().is_err());
    registers[5] = 0x60;
    uart.flush().unwrap();
    let oversized = "x".repeat(1025);
    assert!(uart.write(format_args!("{oversized}")).is_err());
    assert!(uart.emergency_write(format_args!("{oversized}")).is_err());
    uart.write(format_args!("ready\n")).unwrap();
    assert_eq!(registers[0], b'\n');
}
