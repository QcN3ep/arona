/* SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later */

ENTRY(_start)

PHDRS
{
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
    tls PT_TLS FLAGS(4);
}

SECTIONS
{
    . = 0x100000;
    .text :
    {
        KEEP(*(.text.start))
        *(.text .text.*)
    } :text

    . = ALIGN(4096);
    .rodata : { *(.rodata .rodata.* .srodata .srodata.*) } :rodata

    . = ALIGN(4096);
    .data :
    {
        *(.data .data.*)
        PROVIDE(__global_pointer$ = . + 0x800);
        *(.sdata .sdata.* .got .got.*)
    } :data

    .tdata :
    {
        __tls_start = .;
        *(.tdata .tdata.*)
        __tls_file_end = .;
    } :data :tls
    .tbss (NOLOAD) :
    {
        *(.tbss .tbss.*)
        __tls_end = .;
    } :data :tls
    __tls_align = MAX(ALIGNOF(.tdata), ALIGNOF(.tbss));
    . = ADDR(.tbss) + SIZEOF(.tbss);

    .bss (NOLOAD) : { *(.bss .bss.* .sbss .sbss.*) *(COMMON) } :data

    /DISCARD/ : { *(.eh_frame .eh_frame_hdr .comment) }
}
