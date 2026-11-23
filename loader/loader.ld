/*
 * Linker script for the chain-load blob.
 *
 * The blob lives high in RAM, above every game's loaded payload and below
 * the stack, so it survives while the target EXE is streamed into
 * 0x80010000 (which is where the launcher itself was running).
 *
 * Output is a headerless raw binary: the launcher embeds these bytes,
 * copies them to LOADER_BASE, and jumps to offset 0.
 *
 * Region map at load time:
 *   0x80010000 .. t_addr+t_size   target EXE payload (streamed in)
 *   0x801F0000 .. 0x801F8000      this blob (32 KiB budget)
 *   0x801F8000 .. 0x80200000      stack, shared with the launcher
 *
 * On the scene-core branch the launcher links with RAM_SIZE_OVERRIDE (see the
 * Makefile's CORE_RAM_SIZE), so its stack top sits at the core window's base
 * and its stack reserve now starts below 0x801F8000, overlapping the end of
 * this 32 KiB budget. The blob itself is 14,040 B and ends 8,232 B below the
 * reserve, so nothing collides today. The blob retires once the launcher
 * switches games through the core (scene-system plan, CORE-ABI.md).
 */

LOADER_BASE = 0x801F0000;
LOADER_SIZE = 32K;

ENTRY(loader_entry)

MEMORY {
    RAM (rwx) : ORIGIN = LOADER_BASE, LENGTH = LOADER_SIZE
}

SECTIONS {
    .text : {
        /* Entry must be at offset 0: the launcher jumps to LOADER_BASE. */
        KEEP(*(.text.loader_entry));
        *(.text .text.*);
        *(.rodata .rodata.*);
        *(.data .data.*);
        *(.got);
        . = ALIGN(4);
    } > RAM

    .bss (NOLOAD) : {
        __bss_start = .;
        *(.bss .bss.*);
        *(COMMON);
        . = ALIGN(4);
        __bss_end = .;
    } > RAM

    /* Nothing zeroes .bss for the blob (the launcher copies .text only),
       so any static here would be silently garbage. Keep the blob
       stack-only and let the link fail if that ever stops being true. */
    ASSERT(__bss_end == __bss_start, "loader blob must have no .bss")

    /DISCARD/ : {
        *(.MIPS.abiflags)
        *(.reginfo)
        *(.eh_frame_hdr)
        *(.eh_frame)
        *(.comment)
        *(.note.*)
    }
}
