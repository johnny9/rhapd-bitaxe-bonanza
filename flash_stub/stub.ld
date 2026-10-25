ENTRY(run)
SECTIONS {
    . = 0x20030000;
    .text : ALIGN(4) { KEEP(*(.entry)) *(.text*) *(.rodata*) }
    /DISCARD/ : { *(.data*) *(.bss*) *(.comment*) *(.note*) *(.ARM*) *(.eh_frame*) }
}
