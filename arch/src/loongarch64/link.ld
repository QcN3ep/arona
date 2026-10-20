/* SPDX-License-Identifier: MPL-2.0 */

ENTRY(_start)

PHDRS
{
    entry PT_LOAD FLAGS(5);
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

_kernel_phys_start = 0x00200000;
_kernel_offset = 0xffff800000000000;

SECTIONS
{
    . = _kernel_phys_start;

    .text.entry :
    {
        KEEP(*(.text.entry))
        . = ALIGN(64);
        KEEP(*(.text.entry.refill))
        *(.text.entry.rust)
    } :entry
    ASSERT(SIZEOF(.text.entry) <= 4096, "boot entry exceeds one page")

    . = ALIGN(4096) + _kernel_offset;

    .text : AT(ADDR(.text) - _kernel_offset)
    {
        KEEP(*(.text.boot.trap))
        *(.text .text.*)
        . = ALIGN(4096);
    } :text

    .rodata : AT(ADDR(.rodata) - _kernel_offset)
    {
        _kernel_image = .;
        QUAD(_kernel_phys_start)
        QUAD(_kernel_phys_end)
        _kernel_text = .;
        QUAD(LOADADDR(.text))
        QUAD(LOADADDR(.text) + SIZEOF(.text))
        QUAD(ADDR(.text))
        _kernel_rodata = .;
        QUAD(LOADADDR(.rodata))
        QUAD(LOADADDR(.rodata) + SIZEOF(.rodata))
        QUAD(ADDR(.rodata))
        _kernel_data = .;
        QUAD(LOADADDR(.data))
        QUAD(_kernel_phys_end)
        QUAD(ADDR(.data))
        _kernel_boot_tables = .;
        QUAD(_boot_tables_phys)
        QUAD(_boot_tables_phys_end)
        _kernel_boot_stack = .;
        QUAD(_boot_stack_bottom - _kernel_offset)
        QUAD(_boot_stack_top - _kernel_offset)
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
        . = ALIGN(8);
        __start_arona_hardware = .;
        KEEP(*(arona_hardware))
        __stop_arona_hardware = .;
        . = ALIGN(4096);
    } :rodata

    .data : AT(ADDR(.data) - _kernel_offset)
    {
        *(.data .data.*)
        *(.sdata .sdata.*)
        *(.got .got.*)
    } :data

    .bss (NOLOAD) : AT(ADDR(.bss) - _kernel_offset)
    {
        . = ALIGN(16);
        _sbss = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)
        . = ALIGN(8);
        _ebss = .;
    } :data

    .boot_tables ALIGN(4096) (NOLOAD) : AT(ADDR(.boot_tables) - _kernel_offset)
    {
        KEEP(*(.boot_tables))
    } :data
    _boot_tables_phys = LOADADDR(.boot_tables);
    _boot_tables_phys_end = LOADADDR(.boot_tables) + SIZEOF(.boot_tables);

    .boot_stack (NOLOAD) : AT(ADDR(.boot_stack) - _kernel_offset)
    {
        _boot_stack_bottom = .;
        . += 0x10000;
        _boot_stack_top = .;
    } :data

    _boot_stack_phys_top = _boot_stack_top - _kernel_offset;

    _kernel_phys_end = . - _kernel_offset;
    ASSERT(_kernel_phys_end - _kernel_phys_start <= 0x200000, "kernel exceeds boot mapping")

    /DISCARD/ :
    {
        *(.eh_frame)
        *(.eh_frame_hdr)
        *(.comment)
    }
}
