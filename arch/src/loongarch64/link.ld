/* SPDX-License-Identifier: MPL-2.0 */

ENTRY(_start)

SECTIONS
{
    . = 0x00200000;
    _kernel_start = .;

    .text.entry : ALIGN(4)
    {
        KEEP(*(.text.entry))
    }

    .text : ALIGN(4)
    {
        *(.text .text.*)
    }

    .rodata : ALIGN(8)
    {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(8)
    {
        *(.data .data.*)
        *(.got .got.*)
    }

    .bss (NOLOAD) : ALIGN(16)
    {
        _sbss = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(8);
        _ebss = .;
    }

    .boot_stack (NOLOAD) : ALIGN(16)
    {
        _boot_stack_bottom = .;
        . += 0x10000;
        _boot_stack_top = .;
    }

    . = ALIGN(4096);
    _kernel_end = .;

    /DISCARD/ :
    {
        *(.eh_frame)
        *(.eh_frame_hdr)
        *(.comment)
    }
}
