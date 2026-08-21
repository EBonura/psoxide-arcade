//! Drawing the carousel: everything that talks to the GPU.
//!
//! [`carousel`] works out where things go; this turns those positions into
//! gouraud triangles. Ellipses are triangle fans shaded top to bottom, which
//! at this size reads as a glossy lozenge without a single texture.

use carousel::{Bead, Placed, TURN};
use psx_gpu::framebuf::FrameBuffer;
use psx_gpu::material::{BlendMode, TextureMaterial};
use psx_vram::{Clut, Color555, TexDepth, Tpage, VramRect};
use psx_gpu::{self as gpu};
use psx_math::{cos_q12, sin_q12};

/// Most segments an ellipse is drawn with. These are triangle fans, so this is
/// the polygon count: at twelve the pills read as coarse dodecagons, and at
/// 320x240 with no antialiasing every facet shows.
const MAX_SEGMENTS: usize = 9;
/// Fewest. Below this a bead stops looking like a circle at all.
const MIN_SEGMENTS: usize = 6;

/// Segments worth spending on an ellipse of this size. The ball is seventy-odd
/// beads and most of them are a handful of pixels across, where twelve
/// segments buys nothing but triangles; at that count the menu was missing
/// 60 Hz, which stretched every time-driven effect with it.
fn segments_for(rx: i16, ry: i16) -> usize {
    let size = rx.max(ry) as usize;
    (size / 2).clamp(MIN_SEGMENTS, MAX_SEGMENTS)
}

const GLOSS_TOP: (u8, u8, u8) = (255, 66, 44);
const GLOSS_BOTTOM: (u8, u8, u8) = (58, 0, 2);
const GLOSS_EDGE: (u8, u8, u8) = (178, 10, 12);
const SPECULAR: (u8, u8, u8) = (255, 196, 170);

fn lerp(a: u8, b: u8, t: u8) -> u8 {
    let a = a as i32;
    let b = b as i32;
    (a + (((b - a) * t as i32) >> 8)) as u8
}

fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: u8) -> (u8, u8, u8) {
    (lerp(a.0, b.0, t), lerp(a.1, b.1, t), lerp(a.2, b.2, t))
}

fn scale_rgb(c: (u8, u8, u8), t: u8) -> (u8, u8, u8) {
    (
        ((c.0 as u32 * t as u32) >> 8) as u8,
        ((c.1 as u32 * t as u32) >> 8) as u8,
        ((c.2 as u32 * t as u32) >> 8) as u8,
    )
}

fn ellipse(cx: i16, cy: i16, rx: i16, ry: i16, top: (u8, u8, u8), bottom: (u8, u8, u8)) {
    if rx <= 0 || ry <= 0 {
        return;
    }
    let segments = segments_for(rx, ry);
    let centre = mix(top, bottom, 128);
    let vertex = |seg: usize| -> ((i16, i16), (u8, u8, u8)) {
        let a = ((seg as i32 * TURN) / segments as i32) as u16;
        let sx = cx as i32 + ((rx as i32 * sin_q12(a)) >> 12);
        let sy = cy as i32 - ((ry as i32 * cos_q12(a)) >> 12);
        // cos is +1 at the top of the ellipse, -1 at the bottom.
        let t = ((4096 - cos_q12(a)) * 255 / 8192) as u8;
        ((sx as i16, sy as i16), mix(top, bottom, t))
    };

    let (mut prev_p, mut prev_c) = vertex(0);
    for seg in 1..=segments {
        let (p, c) = vertex(seg);
        gpu::draw_tri_gouraud([(cx, cy), prev_p, p], [centre, prev_c, c]);
        prev_p = p;
        prev_c = c;
    }
}

/// One carousel pill: the lozenge, a rim, and a specular blob up and left.
///
/// `pulse` lifts the whole thing on the beat.
pub fn pill(item: &Placed, pulse: u8) {
    let lit = 90 + ((item.front as u32 * 165) >> 8) as u32;
    let dim = (lit + (pulse as u32 * 40 / 255)).min(255) as u8;
    ellipse(
        item.x,
        item.y,
        item.rx,
        item.ry,
        scale_rgb(GLOSS_TOP, dim),
        scale_rgb(GLOSS_BOTTOM, dim),
    );
    ellipse(
        item.x - (item.rx * 5) / 12,
        item.y - (item.ry * 5) / 12,
        item.rx / 5,
        item.ry / 4,
        scale_rgb(SPECULAR, dim),
        scale_rgb(mix(SPECULAR, GLOSS_TOP, 200), dim),
    );
}

/// The carousel mirrored in the floor it sits on: same pill, flipped about
/// `floor_y`, squashed and dimmed. Reflections of the far pills fall off the
/// bottom of the screen and clip away, which is the cheap half of the trick.
pub fn pill_reflection(item: &Placed, y: i16) {
    let ry = (item.ry * 2) / 5;
    if y - ry > 239 || ry <= 0 {
        return;
    }
    // Dim, and darker at the top where it meets the real pill, so the two do
    // not read as one object.
    let lit = 40 + ((item.front as u32 * 55) >> 8) as u8;
    ellipse(
        item.x,
        y,
        item.rx,
        ry,
        scale_rgb(GLOSS_BOTTOM, lit),
        scale_rgb(GLOSS_TOP, lit),
    );
}

/// One of the small spheres in the cluster.
pub fn bead(bead: &Bead) {
    if bead.r <= 0 {
        return;
    }
    ellipse(
        bead.x,
        bead.y,
        bead.r,
        bead.r,
        scale_rgb(GLOSS_TOP, bead.lit),
        scale_rgb(GLOSS_BOTTOM, bead.lit),
    );
    // Only the near beads are big enough for a highlight to land on.
    if bead.r >= 7 {
        ellipse(
            bead.x - bead.r / 3,
            bead.y - bead.r / 3,
            bead.r / 3,
            bead.r / 3,
            scale_rgb(SPECULAR, bead.lit),
            scale_rgb(GLOSS_TOP, bead.lit),
        );
    }
}

const METER_WIDTH: u16 = 3;
const METER_PITCH: i16 = 4;
const METER_TALLEST: i16 = 16;

fn meter_bar(x: i16, base_y: i16, level: u8) {
    let h = 1 + (level as i16 * (METER_TALLEST - 1)) / 255;
    // Tall bars run hot, short ones stay in the dark blue.
    // Tall bars run hot toward orange, short ones stay in the deep red.
    let heat = level / 2;
    gpu::draw_rect_flat(
        x,
        base_y - h,
        METER_WIDTH,
        h as u16,
        180u8.saturating_add(level / 4),
        20u8.saturating_add(heat),
        16u8.saturating_add(heat / 3),
    );
}

/// The level meter, one bar per band of the track's pre-analysed spectrum,
/// low frequencies on the left.
pub fn level_meter(x: i16, base_y: i16, levels: &[u8]) {
    for (band, level) in levels.iter().enumerate() {
        meter_bar(x + band as i16 * METER_PITCH, base_y, *level);
    }
}

/// Fallback for a track the disc carries no analysis for: bars lagging each
/// other off the beat, which keeps time without listening.
pub fn level_meter_beat(x: i16, base_y: i16, pulse: u8) {
    for bar in 0..6i16 {
        let level = pulse.saturating_sub((bar as u16 * 34) as u8);
        meter_bar(x + bar * METER_PITCH, base_y, level);
    }
}


/// Flags are drawn at this size in the top-right corner. Two to one, which is
/// the Union flag's own ratio; at three to two it read as squat.
pub const FLAG_W: i16 = 24;
pub const FLAG_H: i16 = 12;

/// The PSoXide mark, as the one texture on the screen.
///
/// 4bpp with a sixteen-colour palette, and palette entry zero left at
/// `0x0000`, which the PlayStation treats as transparent: the starfield shows
/// through the letterforms rather than the logo sitting in a black box.
pub struct Banner {
    tpage: Tpage,
    clut: Clut,
    w: i16,
    h: i16,
}

impl Banner {
    /// Push the texture and its palette into VRAM. `tpage` must be 4bpp and
    /// clear of the framebuffer; the whole thing has to fit one page, since
    /// UVs are bytes.
    pub fn upload(pixels: &[u8], palette: &[u8], w: i16, h: i16, tpage: Tpage, clut: Clut) -> Self {
        // Four texels a halfword, so a row is a quarter as wide in VRAM.
        let halfwords_per_row = (w as u16).div_ceil(4);
        psx_vram::upload_bytes(
            psx_vram::VramRect::new(tpage.x(), tpage.y(), halfwords_per_row, h as u16),
            pixels,
        );
        let entries: [Color555; 16] = core::array::from_fn(|i| {
            Color555::raw(u16::from_le_bytes([palette[i * 2], palette[i * 2 + 1]]))
        });
        psx_vram::upload_clut(clut, &entries);
        Banner { tpage, clut, w, h }
    }

    /// Draw it with its top-left at `x, y`, one texel to one pixel.
    pub fn draw(&self, x: i16, y: i16) {
        let (w, h) = (self.w, self.h);
        let (u1, v1) = ((w - 1) as u8, (h - 1) as u8);
        gpu::draw_quad_textured(
            [(x, y), (x + w, y), (x, y + h), (x + w, y + h)],
            [(0, 0), (u1, 0), (0, v1), (u1, v1)],
            self.clut.uv_clut_word(),
            self.tpage.uv_tpage_word(0),
            // Neutral tint: 128 is "as the texture is" on this hardware.
            (128, 128, 128),
        );
    }
}

/// A horizontal strip of small marks sharing one page and one palette.
///
/// The credits card writes `EBonura/PSoXide`, not the whole URL, because the
/// box is twenty-odd characters wide and every real link overflows it. These
/// carry the domain the text drops.
pub struct Icons {
    tpage: Tpage,
    clut: Clut,
    /// Row of the page the strip starts at: it shares the banner's page and
    /// sits below the mark, so this is not zero.
    v: u8,
    /// Cell pitch and the mark inside it. The cell is padded to a multiple
    /// of four texels; at 4bpp anything else starts a later mark mid-nibble.
    cell: i16,
    size: i16,
}

impl Icons {
    /// Push the strip into `tpage` at page row `v`, and its palette to `clut`.
    pub fn upload(
        pixels: &[u8],
        palette: &[u8],
        w: i16,
        h: i16,
        tpage: Tpage,
        v: u8,
        clut: Clut,
        cell: i16,
    ) -> Self {
        // Four texels a halfword, as with the banner.
        let halfwords_per_row = (w as u16).div_ceil(4);
        psx_vram::upload_bytes(
            psx_vram::VramRect::new(tpage.x(), tpage.y() + v as u16, halfwords_per_row, h as u16),
            pixels,
        );
        let entries: [Color555; 16] = core::array::from_fn(|i| {
            Color555::raw(u16::from_le_bytes([palette[i * 2], palette[i * 2 + 1]]))
        });
        psx_vram::upload_clut(clut, &entries);
        Icons {
            tpage,
            clut,
            v,
            cell,
            size: h,
        }
    }

    /// Draw mark `slot` with its top-left at `x, y`, tinted like the text
    /// beside it. The texture is a grey ramp, so the tint is the colour.
    /// A sprite rather than a textured polygon, for the reason spelled out on
    /// [`TextCache::draw`]: the polygon path carries its page in a vertex and
    /// draws nothing into an off-screen rect, which is exactly where these go.
    pub fn draw(&self, slot: i16, x: i16, y: i16, tint: (u8, u8, u8)) {
        let s = self.size as u16;
        gpu::draw_sprite_material(
            x,
            y,
            s,
            s,
            ((slot * self.cell) as u8, self.v),
            TextureMaterial::opaque(self.clut.uv_clut_word(), self.tpage.uv_tpage_word(0), tint),
        );
    }
}

/// One step of a fade to black: halve whatever is in the buffer.
pub fn fade_step() {
    for tri in [
        [(0, 0), (320, 0), (0, 240)],
        [(320, 0), (0, 240), (320, 240)],
    ] {
        gpu::draw_tri_flat_blended(tri, 0, 0, 0, BlendMode::Average);
    }
}

/// The solid black band the mark and the title sit in, with the same border
/// the text panel uses so the two read as one system.
pub fn header_strip(h: i16) {
    const BORDER: (u8, u8, u8) = (150, 30, 34);
    gpu::draw_rect_flat(0, 0, 320, h as u16, 0, 0, 0);
    gpu::draw_rect_flat(0, h - 1, 320, 1, BORDER.0, BORDER.1, BORDER.2);
}

/// A dark panel to lay text over, with a thin border.
///
/// Black at [`BlendMode::Average`] is `(background + 0) / 2`, so it halves
/// whatever it covers rather than hiding it: the ball and the starfield stay
/// visible underneath, just far enough back for white text to sit on them.
pub fn text_panel(x: i16, y: i16, w: i16, h: i16) {
    const BORDER: (u8, u8, u8) = (150, 30, 34);
    // Two triangles, since the SDK blends triangles and not rectangles.
    for tri in [
        [(x, y), (x + w, y), (x, y + h)],
        [(x + w, y), (x, y + h), (x + w, y + h)],
    ] {
        gpu::draw_tri_flat_blended(tri, 0, 0, 0, BlendMode::Average);
    }
    // Border solid rather than blended, so the edge stays crisp against
    // whatever is behind it.
    gpu::draw_rect_flat(x, y, w as u16, 1, BORDER.0, BORDER.1, BORDER.2);
    gpu::draw_rect_flat(x, y + h - 1, w as u16, 1, BORDER.0, BORDER.1, BORDER.2);
    gpu::draw_rect_flat(x, y, 1, h as u16, BORDER.0, BORDER.1, BORDER.2);
    gpu::draw_rect_flat(x + w - 1, y, 1, h as u16, BORDER.0, BORDER.1, BORDER.2);
}

/// The Union flag.
///
/// Drawn a row at a time rather than as two big parallelograms. At 24x12 the
/// saltire steps exactly two across for one down, corner to corner, and
/// stepping it by hand gets that; handing the GPU a parallelogram lets its
/// own edge rules pick the steps, which came out ragged and uneven.
///
/// Proportions follow the real flag where they fit: the cross of St George is
/// a fifth of the height in red with a pixel of white either side. The arms of
/// the saltire are a pixel wider than geometry wants, because at twelve pixels
/// tall a true-width diagonal disappears.
///
/// The counterchange is real: St Patrick's red sits below the white diagonal
/// on the hoist and above it on the fly, which is the asymmetry the flag is
/// recognised by and the first thing an eyeballed version loses.
pub fn flag_uk(x: i16, y: i16) {
    const BLUE: (u8, u8, u8) = (8, 24, 92);
    const WHITE: (u8, u8, u8) = (238, 238, 242);
    const RED: (u8, u8, u8) = (184, 16, 36);
    let (w, h) = (FLAG_W, FLAG_H);

    gpu::draw_rect_flat(x, y, w as u16, h as u16, BLUE.0, BLUE.1, BLUE.2);

    // A horizontal run of one row, clipped to the flag.
    let run = |from: i16, len: i16, row: i16, c: (u8, u8, u8)| {
        let start = from.max(0);
        let end = (from + len).min(w);
        if end > start {
            gpu::draw_rect_flat(x + start, y + row, (end - start) as u16, 1, c.0, c.1, c.2);
        }
    };

    for row in 0..h {
        // Two across for one down, both ways.
        let down = row * 2;
        let up = w - 2 - down;
        run(down - 1, 4, row, WHITE);
        run(up - 1, 4, row, WHITE);

        // St Patrick, offset within the white. Hoist half low, fly half high,
        // which is what counterchanged means here.
        let hoist = row < h / 2;
        let down_red = if hoist { down + 1 } else { down - 1 };
        let up_red = if hoist { up - 1 } else { up + 1 };
        run(down_red, 2, row, RED);
        run(up_red, 2, row, RED);
    }

    // Cross of St George over the top, white-bordered.
    gpu::draw_rect_flat(x, y + 4, w as u16, 4, WHITE.0, WHITE.1, WHITE.2);
    gpu::draw_rect_flat(x + 10, y, 4, h as u16, WHITE.0, WHITE.1, WHITE.2);
    gpu::draw_rect_flat(x, y + 5, w as u16, 2, RED.0, RED.1, RED.2);
    gpu::draw_rect_flat(x + 11, y, 2, h as u16, RED.0, RED.1, RED.2);
}

/// The Italian tricolour.
pub fn flag_it(x: i16, y: i16) {
    let third = (FLAG_W / 3) as u16;
    let h = FLAG_H as u16;
    gpu::draw_rect_flat(x, y, third, h, 0, 140, 69);
    gpu::draw_rect_flat(x + third as i16, y, third, h, 240, 240, 240);
    gpu::draw_rect_flat(x + 2 * third as i16, y, third, h, 205, 33, 42);
}

/// A block of text rendered once into spare VRAM and then blitted.
///
/// Drawing a description a glyph at a time costs hundreds of textured quads
/// a frame, which measured at roughly a whole vblank: as much as the entire
/// carousel. The text only changes when the selection or the language does,
/// so it is rendered into an off-screen rect on those frames and drawn as
/// one sprite on every other one.
pub struct TextCache {
    /// What is currently rendered, so a frame that would draw the same thing
    /// again can skip it.
    key: u32,
    /// Row of the shared page this instance owns, and its extent. A second
    /// cache costs nothing but rows: at 15bpp a texel is a halfword, so the
    /// page at `CACHE_X` runs 256 rows deep and the description block only
    /// uses the top 92 of them.
    y: u16,
    w: i16,
    h: i16,
}

/// Spare VRAM, clear of both framebuffers and of every font page. The
/// column is a page wide at most, so the blit no longer straddles a page
/// boundary the way the old full-width block did.
const CACHE_X: u16 = 512;
const CACHE_Y: u16 = 0;
pub const CACHE_W: i16 = 115;
pub const CACHE_H: i16 = 92;

impl TextCache {
    /// A cache holding nothing. Any key re-renders it.
    pub const fn new() -> Self {
        Self::at(CACHE_Y, CACHE_W, CACHE_H)
    }

    /// A second cache further down the same page. `y` is a texture V, so it
    /// has to stay inside a byte.
    pub const fn at(y: u16, w: i16, h: i16) -> Self {
        TextCache {
            key: u32::MAX,
            y,
            w,
            h,
        }
    }

    /// Whether `key` is already rendered.
    pub fn holds(&self, key: u32) -> bool {
        self.key == key
    }

    /// Point the GPU at the off-screen rect and clear it to black. Black
    /// because the block is drawn back with [`BlendMode::Add`], where black
    /// contributes nothing and the panel behind shows through.
    pub fn begin(&mut self, key: u32) {
        self.key = key;
        gpu::fill_rect(CACHE_X, self.y, self.w as u16, self.h as u16, 0, 0, 0);
        gpu::set_draw_area(
            CACHE_X,
            self.y,
            CACHE_X + self.w as u16 - 1,
            self.y + self.h as u16 - 1,
        );
        gpu::set_draw_offset(CACHE_X as i16, self.y as i16);
    }

    /// Point it back at the buffer being drawn this frame.
    pub fn end(&self, fb: &FrameBuffer) {
        let y = fb.buffer_y(fb.drawing);
        gpu::set_draw_area(0, y, fb.width - 1, y + fb.height - 1);
        gpu::set_draw_offset(0, y as i16);
    }

    /// Blit the block with its top-left at `x, y`.
    ///
    /// A textured rectangle, not a textured polygon. A rectangle takes its
    /// page from the current draw mode, which is how the font draws every
    /// glyph; the polygon path carries the page in a vertex instead and drew
    /// nothing here, including when pointed at the framebuffer itself.
    pub fn draw(&self, x: i16, y: i16) {
        let tpage = Tpage::new(CACHE_X, CACHE_Y, TexDepth::Bit15);
        gpu::draw_sprite_material(
            x,
            y,
            self.w as u16,
            self.h as u16,
            (0, self.y as u8),
            // Neutral tint: 128 is "as the texture is". A texel of
            // 0x0000 is transparent on this hardware, which is what
            // lets the panel show through around the letterforms.
            TextureMaterial::opaque(0, tpage.uv_tpage_word(0), (128, 128, 128)),
        );
    }
}

// ======================================================================
// The screenshot beside the description
// ======================================================================

/// Where the selected game's screenshot sits in VRAM: one 8bpp 120x90
/// frame at (512,256), 60x90 halfwords -- clear of both framebuffers,
/// every font page, the text cache above it and its CLUT rows below it.
const SHOT_TPAGE: Tpage = Tpage::new(512, 256, TexDepth::Bit8);
/// TWO CLUT rows, ping-ponged per upload. The GPU's 240-entry CLUT cache
/// line (8bpp colors 16-255) reloads only when a primitive's clut WORD
/// changes; rewriting the data under a constant clut address left every
/// shot after the first wearing the first shot's palette on real hardware
/// (console footage 2026-08-07, reproduced in PSoXide once the per-line
/// cache landed). Alternating rows makes the word change with the data.
const SHOT_CLUTS: [Clut; 2] = [Clut::new(512, 508), Clut::new(512, 509)];
/// Which of the two rows holds the CURRENT image's palette. MIPS-I has no
/// atomics and this hardware is single threaded; a static mut behind raw
/// pointer accessors is the whole synchronisation story.
static mut SHOT_CLUT_BANK: usize = 0;

fn shot_clut_bank() -> usize {
    unsafe { core::ptr::read(&raw const SHOT_CLUT_BANK) }
}

fn set_shot_clut_bank(bank: usize) {
    unsafe { core::ptr::write(&raw mut SHOT_CLUT_BANK, bank) }
}
const SHOT_RECT: VramRect =
    VramRect::new(512, 256, (disc_toc::SHOT_W / 2) as u16, disc_toc::SHOT_H as u16);

/// Send one cooked shot (CLUT then pixels, `disc_toc::SHOT_BYTES` of it)
/// into the screenshot's VRAM slot. Only call while the shot is faded to
/// black: there is a single slot, and swapping it mid-view would tear.
pub fn upload_shot(shot: &[u8]) {
    let clut = &shot[..disc_toc::SHOT_CLUT_BYTES];
    let pixels = &shot[disc_toc::SHOT_CLUT_BYTES..disc_toc::SHOT_BYTES];
    let bank = shot_clut_bank() ^ 1;
    let target = SHOT_CLUTS[bank];
    psx_vram::upload_bytes(VramRect::new(target.x(), target.y(), 256, 1), clut);
    psx_vram::upload_bytes(SHOT_RECT, pixels);
    set_shot_clut_bank(bank);
}

/// The screenshot itself, with its top-left at `x, y`, faded up through
/// `level` (128 is full brightness). A sprite rather than a quad: a sprite
/// steps one texel per pixel from its start, which keeps the frame
/// pixel-exact instead of trusting interpolated UVs. Its box supplies the
/// border, so the image draws bare.
pub fn draw_shot(x: i16, y: i16, level: u8) {
    let clut = SHOT_CLUTS[shot_clut_bank()];
    gpu::draw_sprite_material(
        x,
        y,
        disc_toc::SHOT_W as u16,
        disc_toc::SHOT_H as u16,
        (0, 0),
        TextureMaterial::opaque(
            clut.uv_clut_word(),
            SHOT_TPAGE.uv_tpage_word(0),
            (level, level, level),
        ),
    );
}
