//! Diagnostic painting for the chain-load blob: exact-pixel rectangles
//! and a small text renderer built out of them.
//!
//! The first debug burn drew panels with GP0(02h) FillVram, whose X
//! coordinate and width snap to 16-pixel steps on hardware: shapes fused
//! and grids slid off their own columns. Everything here goes through
//! GP0(60h) monochrome rectangles instead, which honour exact
//! coordinates, at the cost of needing the drawing area configured after
//! the GPU reset `quiesce()` performs.

use psx_font::fonts::basic::BASIC_BITMAP;

/// GP0/GP1 ports.
const GP0: u32 = 0x1F80_1810;
const GP1: u32 = 0x1F80_1814;

/// One GP0 packet: wait for GPUSTAT bit 26 (ready for a new command)
/// once, then write every word of the packet back to back.
///
/// Both halves of that are silicon lessons. Unpaced bursts of hundreds
/// of words overflowed the FIFO and desynced the command stream into
/// garbage rectangles (first debug burn). But pacing EVERY word is as
/// wrong the other way: bit 26 drops while the GPU ingests the rest of
/// a multi-word packet, so a per-word wait spins its whole bound before
/// each vertex word -- the 2026-08-01 recording shows the fail panel
/// painting at seconds per cell because of exactly that, while the
/// emulator forces the bit on and renders it instantly. One wait per
/// packet is the protocol the SDK uses for every draw the launcher
/// makes on this same console. Bounded so a dead GPU cannot hang the
/// panel that is trying to report on it.
fn gp0_packet(words: &[u32]) {
    for _ in 0..1_000_000u32 {
        if unsafe { psx_io::read32(GP1) } & (1 << 26) != 0 {
            break;
        }
    }
    for &word in words {
        unsafe { psx_io::write32(GP0, word) };
    }
}

/// Configure drawing after a GP1(00h) reset: drawing area covering the
/// whole displayed framebuffer, zero offset, display area at 0,0.
pub fn setup() {
    unsafe { psx_io::write32(GP1, 0x0300_0001) }; // display off while painting
    // Program the display the way the launcher's gpu::init does, rather
    // than inheriting whatever GP1(00h) reset leaves behind. Without
    // these the loader's 320x240 image was shown through the reset
    // defaults, so a bar centred in the framebuffer landed off-centre on
    // the TV. GP1(08h) 320x240 NTSC, GP1(06h) X 0x260..0x260+320*8,
    // GP1(07h) Y 0x10..0x10+240 -- the standard centred NTSC picture.
    unsafe {
        psx_io::write32(GP1, 0x0800_0001); // display mode: 320x240, NTSC
        psx_io::write32(GP1, 0x0600_0000 | 0x260 | ((0x260 + 320 * 8) << 12));
        psx_io::write32(GP1, 0x0700_0000 | 0x10 | ((0x10 + 240) << 10));
        psx_io::write32(GP1, 0x0500_0000); // display area starts at VRAM 0,0
    }
    gp0_packet(&[0xE1_00_0000]); // texpage/draw-mode defaults
    gp0_packet(&[0xE3_00_0000]); // drawing area top-left (0,0)
    gp0_packet(&[0xE4_00_0000 | (255 << 10) | 320]); // bottom-right (320,255)
    gp0_packet(&[0xE5_00_0000]); // drawing offset 0
}

pub fn show() {
    unsafe { psx_io::write32(GP1, 0x0300_0000) }; // display on
}

/// Whether the diagnostic checklist has been revealed.
///
/// The screen itself is always on now -- a clean load shows a loading
/// bar rather than the dark screen that read as a hung console. This
/// flag only decides whether the STAGE CHECKLIST is drawn, which stays
/// out of the way until something fails. Deliberately initialised
/// non-zero: a zero-initialised static would land in .bss, and loader.ld
/// forbids .bss because nothing zeroes it for the blob.
static mut CHECKLIST_HIDDEN: u8 = 1;

pub fn checklist_shown() -> bool {
    unsafe { core::ptr::read_volatile(&raw const CHECKLIST_HIDDEN) == 0 }
}

pub fn show_checklist() {
    unsafe { core::ptr::write_volatile(&raw mut CHECKLIST_HIDDEN, 0) }
}

/// Solid rectangle at exact pixel coordinates. `rgb` is `0xBBGGRR`.
pub fn rect(x: i16, y: i16, w: i16, h: i16, rgb: u32) {
    gp0_packet(&[
        0x60_00_0000 | rgb,
        ((y as u32) << 16) | (x as u32 & 0xFFFF),
        ((h as u32) << 16) | (w as u32 & 0xFFFF),
    ]);
}

pub const WHITE: u32 = 0x00FF_FFFF;
pub const GREEN: u32 = 0x0000_D000;
pub const YELLOW: u32 = 0x0000_D8FF;
/// Pending / de-emphasised text. Light enough to survive a phone photo
/// of a CRT, dark enough to read as "not yet".
pub const DIM: u32 = 0x0080_8080;
pub const RED_BASE: u32 = 0x0000_0040;
/// The loading screen's background and its empty bar track.
pub const BLACK: u32 = 0x0000_0000;
pub const TRACK: u32 = 0x0030_3030;

/// The SDK's 8x8 public-domain font (dhepper font8x8), reused as plain
/// data: glyphs are drawn as runs of GP0 rectangles, so the blob needs
/// no VRAM upload and leaves no texture state behind for the game.
static FONT: [u8; 1024] = BASIC_BITMAP;

/// Draw `s` at (x, y), every glyph pixel a `scale`-square rectangle.
/// Returns the x just past the text, so calls chain along one line.
pub fn text(x: i16, y: i16, scale: i16, s: &str, rgb: u32) -> i16 {
    text_bytes(x, y, scale, s.as_bytes(), rgb)
}

pub fn text_bytes(mut x: i16, y: i16, scale: i16, s: &[u8], rgb: u32) -> i16 {
    for &b in s {
        glyph(x, y, scale, b, rgb);
        x += 8 * scale;
    }
    x
}

/// `v` as eight hex digits. Returns the x just past them.
pub fn hex32(mut x: i16, y: i16, scale: i16, v: u32, rgb: u32) -> i16 {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for i in 0..8 {
        glyph(x, y, scale, HEX[((v >> (28 - 4 * i)) & 0xF) as usize], rgb);
        x += 8 * scale;
    }
    x
}

/// One glyph as horizontal runs of set pixels, one rectangle per run:
/// a fifth of the GP0 traffic of a rect per pixel, on a port every word
/// of which is paced against GPUSTAT.
fn glyph(x: i16, y: i16, scale: i16, code: u8, rgb: u32) {
    let rows = &FONT[(code as usize & 0x7F) * 8..][..8];
    for (row, bits) in rows.iter().enumerate() {
        let mut col: u32 = 0;
        while col < 8 {
            if bits & (1 << col) == 0 {
                col += 1;
                continue;
            }
            let start = col;
            while col < 8 && bits & (1 << col) != 0 {
                col += 1;
            }
            // Bit 0 is the leftmost pixel (the font8x8 convention).
            rect(
                x + start as i16 * scale,
                y + row as i16 * scale,
                (col - start) as i16 * scale,
                scale,
                rgb,
            );
        }
    }
}
