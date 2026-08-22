//! PSoXide Arcade drawing primitives.
//!
//! The front end is mostly flat geometry: cabinet, CRT, neon floor, bank keys,
//! and cached text. Small ellipses give its joystick and buttons a physical
//! highlight without spending texture memory.

use psx_gpu::framebuf::FrameBuffer;
use psx_gpu::material::{BlendMode, TextureMaterial};
use psx_gpu::{self as gpu};
use psx_math::{cos_q12, sin_q12};
use psx_vram::{Clut, Color555, TexDepth, Tpage, VramRect};

/// Most segments an ellipse is drawn with. These are triangle fans, so this is
/// the polygon count: at twelve the pills read as coarse dodecagons, and at
/// 320x240 with no antialiasing every facet shows.
const MAX_SEGMENTS: usize = 9;
/// Fewest. Below this a cabinet button stops looking circular.
const MIN_SEGMENTS: usize = 6;
const TURN: i32 = 1 << 16;

/// Segments worth spending on an ellipse of this size.
fn segments_for(rx: i16, ry: i16) -> usize {
    let size = rx.max(ry) as usize;
    (size / 2).clamp(MIN_SEGMENTS, MAX_SEGMENTS)
}

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

// ======================================================================
// PSoXide Arcade cabinet front end
// ======================================================================

const ARCADE_CYAN: (u8, u8, u8) = (18, 218, 236);
const ARCADE_MAGENTA: (u8, u8, u8) = (238, 38, 146);
const ARCADE_VIOLET: (u8, u8, u8) = (70, 20, 104);
const ARCADE_INK: (u8, u8, u8) = (5, 2, 18);

/// A quiet CRT wall over a perspective neon floor. It is intentionally flat
/// and architectural: no stars, sphere, ring, or other parent-disc motifs.
pub fn arcade_backdrop(frame: u32, pulse: u8, attract: bool) {
    let pulse_lift = pulse / 10;
    let idle_lift = if attract { 14 } else { 0 };

    // CRT scanlines in the cabinet bay.
    for y in (35..204).step_by(4) {
        gpu::draw_rect_flat(0, y, 320, 1, 4, 2, 13);
    }

    // A row of chase bulbs makes the whole screen read as a marquee. In
    // attract mode the chase brightens, but the game information stays put.
    for bulb in 0..20i16 {
        let hot = ((bulb as u32 + frame / 4) & 3) == 0;
        let lift = if hot { 60 + idle_lift } else { 12 };
        let c = if bulb & 1 == 0 {
            ARCADE_CYAN
        } else {
            ARCADE_MAGENTA
        };
        let c = scale_rgb(c, (96u8).saturating_add(lift).saturating_add(pulse_lift));
        gpu::draw_rect_flat(3 + bulb * 16, 35, 3, 2, c.0, c.1, c.2);
    }

    // Perspective floor. Five horizontals are enough at 320x240; the tighter
    // spacing near the horizon does the depth work without a texture.
    for (y, level) in [(202, 32u8), (208, 42), (216, 58), (226, 76), (238, 100)] {
        let c = scale_rgb(ARCADE_MAGENTA, level.saturating_add(pulse_lift));
        gpu::draw_rect_flat(0, y, 320, 1, c.0, c.1, c.2);
    }
    for x in (0..=320i16).step_by(32) {
        let c = scale_rgb(ARCADE_CYAN, 60u8.saturating_add(pulse_lift));
        gpu::draw_tri_flat([(160, 199), (x, 240), (x + 1, 240)], c.0, c.1, c.2);
    }
}

/// The selected game lives in this cabinet. Its screen aperture is fixed at
/// `(x + 10, y + 38)` so the 120x90 cooked screenshot lands pixel-perfect.
pub fn arcade_cabinet(x: i16, y: i16, w: i16, h: i16, pulse: u8, browse: u8) {
    let energy = 112u8.saturating_add(pulse / 5).saturating_add(browse / 4);
    let cyan = scale_rgb(ARCADE_CYAN, energy);
    let pink = scale_rgb(ARCADE_MAGENTA, energy);

    // Outer glow and cabinet body.
    gpu::draw_rect_flat(x, y + 3, w as u16, (h - 6) as u16, cyan.0, cyan.1, cyan.2);
    gpu::draw_rect_flat(
        x + 2,
        y + 1,
        (w - 4) as u16,
        (h - 2) as u16,
        pink.0,
        pink.1,
        pink.2,
    );
    gpu::draw_rect_flat(x + 4, y + 3, (w - 8) as u16, (h - 6) as u16, 10, 3, 23);

    // Marquee and its hot lower rail.
    gpu::draw_rect_flat(x + 7, y + 5, (w - 14) as u16, 28, 38, 5, 52);
    gpu::draw_rect_flat(x + 7, y + 5, (w - 14) as u16, 2, cyan.0, cyan.1, cyan.2);
    gpu::draw_rect_flat(x + 7, y + 31, (w - 14) as u16, 2, pink.0, pink.1, pink.2);

    // CRT bezel and black glass. The screenshot draws over the glass.
    gpu::draw_rect_flat(x + 8, y + 35, 124, 95, 10, 92, 108);
    gpu::draw_rect_flat(x + 10, y + 37, 120, 92, 0, 0, 0);

    // Control deck projects toward the player.
    gpu::draw_tri_gouraud(
        [(x + 8, y + 131), (x + w - 8, y + 131), (x + 15, y + 146)],
        [(56, 8, 72), (56, 8, 72), (18, 3, 28)],
    );
    gpu::draw_tri_gouraud(
        [
            (x + w - 8, y + 131),
            (x + 15, y + 146),
            (x + w - 15, y + 146),
        ],
        [(56, 8, 72), (18, 3, 28), (18, 3, 28)],
    );
    gpu::draw_rect_flat(x + 8, y + 131, (w - 16) as u16, 1, pink.0, pink.1, pink.2);

    // Joystick, three action buttons, and coin door.
    gpu::draw_rect_flat(x + 31, y + 134, 2, 7, 180, 190, 210);
    ellipse(x + 32, y + 133, 4, 3, ARCADE_CYAN, (4, 70, 88));
    for (button, color) in [ARCADE_MAGENTA, ARCADE_CYAN, (244, 184, 36)]
        .iter()
        .enumerate()
    {
        ellipse(
            x + 91 + button as i16 * 13,
            y + 138,
            4,
            3,
            *color,
            scale_rgb(*color, 70),
        );
    }
    gpu::draw_rect_flat(x + 14, y + 147, (w - 28) as u16, (h - 152) as u16, 7, 2, 15);
    for slot in [x + 45, x + w - 49] {
        gpu::draw_rect_flat(slot, y + 151, 6, 7, 38, 42, 55);
        gpu::draw_rect_flat(slot + 1, y + 152, 4, 2, pink.0, pink.1, pink.2);
    }
}

/// Right-hand game file. A cyan top rail and magenta bottom rail separate it
/// from the red parent-disc panels even before the text cache is blitted.
pub fn arcade_info_panel(x: i16, y: i16, w: i16, h: i16) {
    gpu::draw_rect_flat(
        x,
        y,
        w as u16,
        h as u16,
        ARCADE_CYAN.0,
        ARCADE_CYAN.1,
        ARCADE_CYAN.2,
    );
    gpu::draw_rect_flat(
        x + 2,
        y + 2,
        (w - 4) as u16,
        (h - 4) as u16,
        ARCADE_INK.0,
        ARCADE_INK.1,
        ARCADE_INK.2,
    );
    gpu::draw_rect_flat(x + 2, y + 2, (w - 4) as u16, 13, 22, 5, 34);
    gpu::draw_rect_flat(
        x + 2,
        y + h - 3,
        (w - 4) as u16,
        1,
        ARCADE_MAGENTA.0,
        ARCADE_MAGENTA.1,
        ARCADE_MAGENTA.2,
    );
}

/// Jukebox strip below the game file. The caller writes title and spectrum on
/// top; the rails pulse gently with the authored beat grid.
pub fn arcade_jukebox(x: i16, y: i16, w: i16, h: i16, pulse: u8) {
    let cyan = scale_rgb(ARCADE_CYAN, 150u8.saturating_add(pulse / 3));
    gpu::draw_rect_flat(x, y, w as u16, h as u16, cyan.0, cyan.1, cyan.2);
    gpu::draw_rect_flat(x + 2, y + 2, (w - 4) as u16, (h - 4) as u16, 7, 2, 16);
    gpu::draw_rect_flat(x + w - 38, y + 3, 1, (h - 6) as u16, 60, 12, 75);
}

/// One fixed game-bank key. Active keys lift two pixels and swap from violet
/// to cyan/pink rails, like an illuminated cabinet button.
pub fn arcade_bank_key(x: i16, y: i16, w: i16, h: i16, active: bool, pulse: u8, browse: u8) {
    let lift = if active { 2 } else { 0 };
    let y = y - lift;
    let edge = if active {
        scale_rgb(
            ARCADE_CYAN,
            170u8.saturating_add(pulse / 4).saturating_add(browse / 4),
        )
    } else {
        ARCADE_VIOLET
    };
    gpu::draw_rect_flat(x, y, w as u16, h as u16, edge.0, edge.1, edge.2);
    gpu::draw_rect_flat(x + 2, y + 2, (w - 4) as u16, (h - 4) as u16, 12, 3, 23);
    if active {
        gpu::draw_rect_flat(
            x + 3,
            y + 3,
            (w - 6) as u16,
            2,
            ARCADE_MAGENTA.0,
            ARCADE_MAGENTA.1,
            ARCADE_MAGENTA.2,
        );
    }
}

/// Arcade launch transition: the CRT's top and bottom shutters close over the
/// selected game, meeting on a cyan scan line before the loader takes over.
pub fn arcade_launch_shutter(frame: i32, total: i32) {
    let half = (frame * 120 / total.max(1)).clamp(0, 120) as i16;
    gpu::draw_rect_flat(0, 0, 320, half as u16, 0, 0, 0);
    gpu::draw_rect_flat(0, 240 - half, 320, half as u16, 0, 0, 0);
    if half > 0 && half < 120 {
        gpu::draw_rect_flat(
            0,
            half - 1,
            320,
            1,
            ARCADE_CYAN.0,
            ARCADE_CYAN.1,
            ARCADE_CYAN.2,
        );
        gpu::draw_rect_flat(
            0,
            240 - half,
            320,
            1,
            ARCADE_MAGENTA.0,
            ARCADE_MAGENTA.1,
            ARCADE_MAGENTA.2,
        );
    }
}

/// The solid black band the mark and the title sit in, with the same border
/// the text panel uses so the two read as one system.
pub fn header_strip(h: i16) {
    gpu::draw_rect_flat(0, 0, 320, h as u16, 0, 0, 0);
    gpu::draw_rect_flat(
        0,
        h - 2,
        320,
        1,
        ARCADE_CYAN.0,
        ARCADE_CYAN.1,
        ARCADE_CYAN.2,
    );
    gpu::draw_rect_flat(
        0,
        h - 1,
        320,
        1,
        ARCADE_MAGENTA.0,
        ARCADE_MAGENTA.1,
        ARCADE_MAGENTA.2,
    );
}

/// A dark panel to lay text over, with a thin border.
///
/// Black at [`BlendMode::Average`] is `(background + 0) / 2`, so it halves
/// whatever it covers rather than hiding it: the ball and the starfield stay
/// visible underneath, just far enough back for white text to sit on them.
pub fn text_panel(x: i16, y: i16, w: i16, h: i16) {
    const BORDER: (u8, u8, u8) = ARCADE_CYAN;
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
const SHOT_RECT: VramRect = VramRect::new(
    512,
    256,
    (disc_toc::SHOT_W / 2) as u16,
    disc_toc::SHOT_H as u16,
);

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
