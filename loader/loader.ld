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
        /* Zero-initialised statics are carried in the blob as zero bytes:
           nothing zeroes a .bss for it (the launcher copies the blob bytes
           only), so a separate .bss would read whatever the previous program
           left in RAM. The SDK's polled CD reader keeps one such word
           (psx_io::irq::POLLING_KEEP). */
        *(.bss .bss.*);
        *(COMMON);
        . = ALIGN(4);
    } > RAM

    /DISCARD/ : {
        *(.MIPS.abiflags)
        *(.reginfo)
        *(.eh_frame_hdr)
        *(.eh_frame)
        *(.comment)
        *(.note.*)
    }
}
