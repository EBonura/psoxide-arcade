//! PSoXide Arcade drawing primitives.
//!
//! The front end uses a flat cabinet shell, CRT game cards and cached text,
//! all kept deliberately lean for the PS1 packet envelope.

use psx_gpu::framebuf::FrameBuffer;
use psx_gpu::material::{BlendMode, TextureMaterial};
use psx_gpu::{self as gpu};
use psx_vram::{Clut, Color555, TexDepth, Tpage, VramRect};

fn scale_rgb(c: (u8, u8, u8), t: u8) -> (u8, u8, u8) {
    (
        ((c.0 as u32 * t as u32) >> 8) as u8,
        ((c.1 as u32 * t as u32) >> 8) as u8,
        ((c.2 as u32 * t as u32) >> 8) as u8,
    )
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

/// Three 4bpp game thumbnails packed side by side in one texture page.
/// Separate CLUTs retain each game's palette while one upload keeps the whole
/// selector resident for the life of the menu.
pub struct ArcadeCards {
    tpage: Tpage,
    cluts: [Clut; 3],
    card_w: i16,
    card_h: i16,
}

impl ArcadeCards {
    pub fn upload(
        pixels: &[u8],
        palettes: &[u8],
        card_w: i16,
        card_h: i16,
        tpage: Tpage,
        cluts: [Clut; 3],
    ) -> Self {
        let strip_w = card_w * 3;
        psx_vram::upload_bytes(
            VramRect::new(
                tpage.x(),
                tpage.y(),
                (strip_w as u16).div_ceil(4),
                card_h as u16,
            ),
            pixels,
        );
        for (slot, clut) in cluts.iter().enumerate() {
            let offset = slot * 16 * 2;
            let colors: [Color555; 16] = core::array::from_fn(|index| {
                let at = offset + index * 2;
                Color555::raw(u16::from_le_bytes([palettes[at], palettes[at + 1]]))
            });
            psx_vram::upload_clut(*clut, &colors);
        }
        ArcadeCards {
            tpage,
            cluts,
            card_w,
            card_h,
        }
    }

    pub fn draw(&self, slot: usize, x: i16, y: i16, tint: u8) {
        let slot = slot.min(2);
        let u0 = (slot as i16 * self.card_w) as u8;
        let u1 = (slot as i16 * self.card_w + self.card_w - 1) as u8;
        let v1 = (self.card_h - 1) as u8;
        gpu::draw_quad_textured(
            [
                (x, y),
                (x + self.card_w, y),
                (x, y + self.card_h),
                (x + self.card_w, y + self.card_h),
            ],
            [(u0, 0), (u1, 0), (u0, v1), (u1, v1)],
            self.cluts[slot].uv_clut_word(),
            self.tpage.uv_tpage_word(0),
            (tint, tint, tint),
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

// Capcom Arcade Stadium-inspired cabinet palette. The cool shell and dark
// blue bezel make the red game-select screen read as a separate CRT image.
const CABINET_LILAC: (u8, u8, u8) = (112, 101, 211);
const CABINET_LIGHT: (u8, u8, u8) = (164, 151, 238);
const CABINET_BLUE: (u8, u8, u8) = (24, 18, 112);
const SELECT_RED: (u8, u8, u8) = (190, 43, 25);
const SELECT_BLUE: (u8, u8, u8) = (16, 30, 116);
const SELECT_WHITE: (u8, u8, u8) = (240, 242, 235);
const SELECT_YELLOW: (u8, u8, u8) = (250, 210, 54);

/// The collection as one late-era arcade cabinet. The upper red rectangle is
/// the game-select CRT; the lower blue recess is the control deck carrying
/// music status and Credits.
pub fn selection_cabinet(pulse: u8) {
    let light = scale_rgb(CABINET_LIGHT, 200u8.saturating_add(pulse / 8));

    // Cabinet body and monitor bevel.
    gpu::draw_rect_flat(
        0,
        23,
        320,
        217,
        CABINET_LILAC.0,
        CABINET_LILAC.1,
        CABINET_LILAC.2,
    );
    gpu::draw_rect_flat(0, 23, 320, 3, light.0, light.1, light.2);
    gpu::draw_rect_flat(
        14,
        27,
        292,
        151,
        CABINET_BLUE.0,
        CABINET_BLUE.1,
        CABINET_BLUE.2,
    );
    gpu::draw_rect_flat(20, 31, 280, 143, 2, 2, 10);
    gpu::draw_rect_flat(28, 34, 264, 136, SELECT_RED.0, SELECT_RED.1, SELECT_RED.2);

    // Cabinet control deck, widening toward the player like the reference.
    gpu::draw_tri_gouraud(
        [(14, 178), (306, 178), (0, 240)],
        [CABINET_LIGHT, CABINET_LIGHT, CABINET_BLUE],
    );
    gpu::draw_tri_gouraud(
        [(306, 178), (0, 240), (320, 240)],
        [CABINET_LIGHT, CABINET_BLUE, CABINET_BLUE],
    );
    gpu::draw_rect_flat(
        22,
        190,
        276,
        44,
        CABINET_BLUE.0,
        CABINET_BLUE.1,
        CABINET_BLUE.2,
    );
    gpu::draw_rect_flat(24, 192, 272, 40, 3, 4, 24);
    gpu::draw_rect_flat(24, 192, 272, 1, light.0, light.1, light.2);
}

/// One of the three game cards on the CRT. Selection is the white frame from
/// the reference; unselected cards stay cobalt, so their screenshots remain
/// visible without competing with the current game.
pub fn selection_card(x: i16, y: i16, w: i16, h: i16, active: bool, pulse: u8) {
    let edge = if active {
        scale_rgb(SELECT_WHITE, 230u8.saturating_add(pulse / 10))
    } else {
        SELECT_BLUE
    };
    gpu::draw_rect_flat(x, y, w as u16, h as u16, edge.0, edge.1, edge.2);
    gpu::draw_rect_flat(x + 2, y + 2, (w - 4) as u16, (h - 4) as u16, 5, 8, 44);
    gpu::draw_rect_flat(
        x + 3,
        y + 3,
        (w - 6) as u16,
        12,
        SELECT_BLUE.0,
        SELECT_BLUE.1,
        SELECT_BLUE.2,
    );
    if active {
        gpu::draw_rect_flat(
            x + 3,
            y + h - 8,
            (w - 6) as u16,
            5,
            SELECT_YELLOW.0,
            SELECT_YELLOW.1,
            SELECT_YELLOW.2,
        );
    }
}

pub fn credits_key(x: i16, y: i16, w: i16, h: i16, active: bool, pulse: u8) {
    let edge = if active {
        scale_rgb(SELECT_WHITE, 225u8.saturating_add(pulse / 10))
    } else {
        CABINET_LILAC
    };
    gpu::draw_rect_flat(x, y, w as u16, h as u16, edge.0, edge.1, edge.2);
    gpu::draw_rect_flat(x + 2, y + 2, (w - 4) as u16, (h - 4) as u16, 3, 4, 24);
    if active {
        gpu::draw_rect_flat(
            x + 3,
            y + h - 4,
            (w - 6) as u16,
            2,
            SELECT_YELLOW.0,
            SELECT_YELLOW.1,
            SELECT_YELLOW.2,
        );
    }
}

/// Jukebox strip below the game file. The caller writes title and spectrum on
/// top; the rails pulse gently with the authored beat grid.
pub fn arcade_jukebox(x: i16, y: i16, w: i16, h: i16, pulse: u8) {
    let edge = scale_rgb(CABINET_LIGHT, 178u8.saturating_add(pulse / 5));
    gpu::draw_rect_flat(x, y, w as u16, h as u16, edge.0, edge.1, edge.2);
    gpu::draw_rect_flat(x + 2, y + 2, (w - 4) as u16, (h - 4) as u16, 2, 3, 22);
    gpu::draw_rect_flat(
        x + w - 38,
        y + 3,
        1,
        (h - 6) as u16,
        SELECT_BLUE.0,
        SELECT_BLUE.1,
        SELECT_BLUE.2,
    );
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
            SELECT_WHITE.0,
            SELECT_WHITE.1,
            SELECT_WHITE.2,
        );
        gpu::draw_rect_flat(
            0,
            240 - half,
            320,
            1,
            SELECT_RED.0,
            SELECT_RED.1,
            SELECT_RED.2,
        );
    }
}

/// A dark panel to lay text over, with a thin border.
///
/// Black at [`BlendMode::Average`] is `(background + 0) / 2`, so it halves
/// whatever it covers rather than hiding it: the ball and the starfield stay
/// visible underneath, just far enough back for white text to sit on them.
pub fn text_panel(x: i16, y: i16, w: i16, h: i16) {
    const BORDER: (u8, u8, u8) = SELECT_WHITE;
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
impl TextCache {
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
