//! Journey observation for psoxide-test (cargo feature `journey-state`, test disc only).
//!
//! A read-only block of 32-bit words a program publishes once per frame so a journey can wait on
//! what the program itself says instead of on time. The program never reads it back and nothing
//! in it changes what the program does; the shipping disc has none of it. Layout, from the start
//! of the program's static: +0 magic (four ASCII letters, so a journey can tell the block from
//! whatever the previous program left at that address), +4 sequence (publications), +8 published
//! (1 once every field is written), +12.. the program's fields, named in its own documentation.

/// Words in a block of `N` fields.
pub const fn words(fields: usize) -> usize {
    fields + 3
}

/// The four letters of a magic word, first letter in the lowest byte.
pub const fn magic(letters: [u8; 4]) -> u32 {
    u32::from_le_bytes(letters)
}

/// Write one publication into `block`, which holds `words(N)` words.
///
/// # Safety
/// `block` points at a static of `words(N)` words that nothing else writes (single-threaded).
pub unsafe fn publish<const N: usize>(block: *mut u32, magic: u32, fields: [u32; N]) {
    block.add(2).write_volatile(0);
    block.write_volatile(magic);
    let sequence = block.add(1);
    sequence.write_volatile(sequence.read_volatile().wrapping_add(1));
    for (i, field) in fields.into_iter().enumerate() {
        block.add(3 + i).write_volatile(field);
    }
    block.add(2).write_volatile(1);
}
