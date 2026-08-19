//! Street Fighter 1 video: a pre-CPS board, and almost nothing is shared.
//!
//! SF1 has no CPS-A and no CPS-B, so there is no register file to consult, no
//! bank mapper, no layer-priority word and no gfxram — the tilemaps for the two
//! scrolling planes live in a **ROM** region, the palette is plain 4-4-4 RAM,
//! and the four graphics layouts are two distinct `gfx_layout`s rather than one
//! parameterized by tile size.
//!
//! What *is* shared is the geometry: [`crate::WIDTH`], [`crate::HEIGHT`],
//! [`crate::VISIBLE_X`] and [`crate::VISIBLE_Y`] are the same 384×224 window at
//! (64, 16) inside a raster whose origin is in blanking, so this module inherits
//! the crate documentation's coordinate rule unchanged. SF1's raster is 512×256
//! where CPS-1's is 512×262, which changes nothing about the offset.
//!
//! Hardware facts are cited to MAME `mame0261`, `src/mame/capcom/sf.cpp`
//! (BSD-3-Clause, Olivier Galibert), and to `src/emu/{drawgfx,digfx,tilemap,
//! emupal}.cpp` for the framework behaviour SF1 relies on. Read 2026-08-17.

pub mod gfx;
pub mod palette;
pub mod sprites;
pub mod tilemap;

use crate::compose::Framebuffer;
use gfx::{GfxLayout, GfxSet, CHAR_LAYOUT, SPRITE_LAYOUT};

/// `m_active` bit 2: screen flip (`sf.cpp:351`, "active when dip 8 (flip) on").
pub const ACTIVE_FLIP: u8 = 0x04;
/// Bit 3: the character (text) plane (`sf.cpp:352`).
pub const ACTIVE_TX: u8 = 0x08;
/// Bit 5: the background plane (`sf.cpp:353`).
///
/// ⚠️ Tested **twice**: `gfxctrl_w` disables the tilemap, and `screen_update`
/// separately fills the frame with pen 0 when it is clear (`sf.cpp:455-458`).
pub const ACTIVE_BG: u8 = 0x20;
/// Bit 6: the foreground ("middle") plane (`sf.cpp:354`).
pub const ACTIVE_FG: u8 = 0x40;
/// Bit 7: the sprites (`sf.cpp:462`).
pub const ACTIVE_SPRITES: u8 = 0x80;

/// A debugger's subtraction from the frame.
///
/// The same contract as [`crate::compose::LayerMask`], with SF1's four planes: a
/// mask can hide what the hardware draws and can never show what the hardware
/// hides. So every plane is gated on `hardware_bit && mask_bit`, never `||`.
///
/// Masking the background does **not** trigger [`ACTIVE_BG`]'s pen-0 fill: the
/// fill is hardware behaviour, and a view that changed it would show something the
/// board never shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerMask {
    /// Draw the background plane.
    pub bg: bool,
    /// Draw the foreground plane.
    pub fg: bool,
    /// Draw the sprites.
    pub sprites: bool,
    /// Draw the text plane.
    pub tx: bool,
}

impl LayerMask {
    /// Everything drawn: the default, and the identity.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            bg: true,
            fg: true,
            sprites: true,
            tx: true,
        }
    }
}

impl Default for LayerMask {
    /// [`LayerMask::all`] — **not** the derived all-`false`, which would render a
    /// blank screen for every caller that never heard of a mask.
    fn default() -> Self {
        Self::all()
    }
}

/// One of SF1's four drawable planes.
///
/// The hardware has four separate graphics ROM regions with four fixed colour
/// bases and two different tile layouts, and `sf.cpp` states each fact inline at
/// the point of use. Naming them here states each once: the renderer and the
/// graphics viewer then read the same statement, so a panel cannot report a colour
/// base or a tile size the renderer did not use.
///
/// The order of [`Plane::ALL`] is the drawing order — background, foreground,
/// sprites, text — which is also [`Plane::cycled`]'s order and
/// [`Plane::index`]'s numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plane {
    /// The background tilemap. `gfx1`, colour base 0.
    Bg,
    /// The foreground tilemap. `gfx2`, colour base 256.
    Fg,
    /// The object layer. `gfx3`, colour base 512.
    Sprites,
    /// The text tilemap. `gfx4`, colour base 768, and the only user of
    /// [`crate::sf1::gfx::CHAR_LAYOUT`].
    Tx,
}

impl Plane {
    /// Every plane, in drawing order.
    pub const ALL: [Plane; 4] = [Plane::Bg, Plane::Fg, Plane::Sprites, Plane::Tx];

    /// The first palette entry this plane's colour 0 uses.
    ///
    /// `sf.cpp`'s `PALETTE_INIT` splits 1,024 entries into four fixed blocks of
    /// 256; nothing in the guest can move them.
    #[must_use]
    pub const fn colour_base(self) -> u16 {
        match self {
            Self::Bg => 0,
            Self::Fg => 256,
            Self::Sprites => 512,
            Self::Tx => 768,
        }
    }

    /// The tile layout this plane's region is decoded with.
    #[must_use]
    pub const fn layout(self) -> &'static GfxLayout {
        match self {
            Self::Tx => &CHAR_LAYOUT,
            _ => &SPRITE_LAYOUT,
        }
    }

    /// The `gfxctrl` bit that enables this plane on the hardware.
    #[must_use]
    pub const fn active_bit(self) -> u8 {
        match self {
            Self::Bg => ACTIVE_BG,
            Self::Fg => ACTIVE_FG,
            Self::Sprites => ACTIVE_SPRITES,
            Self::Tx => ACTIVE_TX,
        }
    }

    /// Whether the viewer's mask permits this plane.
    ///
    /// A mask only ever subtracts: [`Sf1Video::render`] draws a plane when the
    /// hardware bit is set **and** this returns true, never when either alone is.
    #[must_use]
    pub const fn permitted(self, mask: &LayerMask) -> bool {
        match self {
            Self::Bg => mask.bg,
            Self::Fg => mask.fg,
            Self::Sprites => mask.sprites,
            Self::Tx => mask.tx,
        }
    }

    /// A two-character label, for a panel with 4 pixels per character.
    ///
    /// "OB" rather than "SP" so it lines up with CPS-1's object row, which the
    /// same overlay draws two panels away.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bg => "BG",
            Self::Fg => "FG",
            Self::Sprites => "OB",
            Self::Tx => "TX",
        }
    }

    /// The next plane, wrapping. [`Plane::ALL`]'s order.
    #[must_use]
    pub const fn cycled(self) -> Self {
        match self {
            Self::Bg => Self::Fg,
            Self::Fg => Self::Sprites,
            Self::Sprites => Self::Tx,
            Self::Tx => Self::Bg,
        }
    }

    /// This plane's decoder, over a region the caller supplies.
    ///
    /// Takes the region rather than reading it from a `Sf1Video` so that
    /// [`Sf1Video::render`] can call it while `self.fb.pens` is mutably borrowed —
    /// see the ⚠️ on `render`.
    #[must_use]
    pub const fn set(self, rom: &[u8]) -> GfxSet<'_> {
        GfxSet {
            rom,
            layout: self.layout(),
            colour_base: self.colour_base(),
        }
    }

    /// `0..=3`, in [`Plane::ALL`]'s order, for indexing a four-element table.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Bg => 0,
            Self::Fg => 1,
            Self::Sprites => 2,
            Self::Tx => 3,
        }
    }
}

/// SF1's video: four graphics regions, the tilerom, and a frame.
#[derive(Debug, Clone)]
pub struct Sf1Video {
    /// The most recently rendered frame. Only [`Framebuffer::pens`] is used —
    /// SF1 has no priority plane, so `prio` stays at its initial value.
    pub fb: Framebuffer,
    /// A debugger's subtraction. [`LayerMask::all`] by default.
    pub enable: LayerMask,
    /// gfx1: the background plane's tiles, colour base 0 (`sf.cpp:725`).
    bg_gfx: Vec<u8>,
    /// gfx2: the foreground plane's, colour base 256 (`sf.cpp:726`).
    fg_gfx: Vec<u8>,
    /// gfx3: the sprites', colour base 512 (`sf.cpp:727`).
    obj_gfx: Vec<u8>,
    /// gfx4: the text plane's, colour base 768 (`sf.cpp:728`).
    tx_gfx: Vec<u8>,
    /// The tilerom, holding both scrolling planes' tile maps.
    tilerom: Vec<u8>,
    /// The last render's palette, already converted.
    ///
    /// Converted at render time rather than per pixel: the 68000 writes entries
    /// directly and MAME's `palette_device::write16` recalculates immediately, so
    /// a frame's colours are the RAM as it stood when the frame was drawn.
    colours: Box<[[u8; 3]; palette::ENTRIES]>,
}

impl Sf1Video {
    /// A video subsystem for SF1, with its five ROM regions.
    ///
    /// Empty regions are allowed and draw nothing — the frontend builds a machine
    /// before a ROM set is loaded on some paths.
    #[must_use]
    pub fn new(
        gfx1: Vec<u8>,
        gfx2: Vec<u8>,
        gfx3: Vec<u8>,
        gfx4: Vec<u8>,
        tilerom: Vec<u8>,
    ) -> Self {
        Self {
            fb: Framebuffer::new(),
            enable: LayerMask::all(),
            bg_gfx: gfx1,
            fg_gfx: gfx2,
            obj_gfx: gfx3,
            tx_gfx: gfx4,
            tilerom,
            colours: Box::new([[0u8; 3]; palette::ENTRIES]),
        }
    }

    /// A pen's colour, from the last rendered frame's palette.
    ///
    /// Out-of-range pens are black rather than a panic: a pen is
    /// `colour_base + granularity * (colour % 16) + pen`, and `colour` comes from
    /// guest-written videoram or a guest-written object attribute.
    #[must_use]
    pub fn rgb(&self, pen: u16) -> [u8; 3] {
        self.colours.get(pen as usize).copied().unwrap_or([0, 0, 0])
    }

    /// This plane's graphics region, as the constructor was given it.
    ///
    /// Published for the graphics viewer: the four regions are private fields with
    /// four different lengths, and a panel that browsed the wrong one would draw
    /// tiles the renderer never fetches.
    #[must_use]
    pub fn region(&self, plane: Plane) -> &[u8] {
        match plane {
            Plane::Bg => &self.bg_gfx,
            Plane::Fg => &self.fg_gfx,
            Plane::Sprites => &self.obj_gfx,
            Plane::Tx => &self.tx_gfx,
        }
    }

    /// This plane's decoder — region, layout and colour base together.
    ///
    /// The one call a panel needs: it cannot assemble a correct [`GfxSet`] from
    /// [`Sf1Video::region`] alone without repeating the layout and colour-base
    /// choices, which is exactly the duplication [`Plane`] exists to prevent.
    #[must_use]
    pub fn gfx(&self, plane: Plane) -> GfxSet<'_> {
        plane.set(self.region(plane))
    }

    /// The tilemap ROM.
    ///
    /// Published because SF1's background and foreground maps live in ROM rather
    /// than in guest RAM, so a tilemap panel reads this and not `videoram`.
    #[must_use]
    pub fn tilerom(&self) -> &[u8] {
        &self.tilerom
    }

    /// Render one frame — `screen_update`, `sf.cpp:453-467`.
    ///
    /// ```c
    /// if (m_active & 0x20) bg->draw(...); else bitmap.fill(0, cliprect);
    /// fg->draw(...);
    /// if (m_active & 0x80) draw_sprites(...);
    /// tx->draw(...);
    /// ```
    ///
    /// The `fg` and `tx` draws are unconditional here because their enables live in
    /// the tilemap objects (`sf.cpp:352-354`); this function folds those in, which
    /// is why all four planes read their [`ACTIVE_TX`]-family bit in one place.
    ///
    /// There is no priority machinery to consult: SF1 has no per-tile priority bit
    /// and no layer-order register, so the order above is fixed.
    pub fn render(
        &mut self,
        videoram: &[u16],
        objectram: &[u16],
        palette_ram: &[u16],
        active: u8,
        bgscroll: u16,
        fgscroll: u16,
    ) {
        for (entry, slot) in self.colours.iter_mut().enumerate() {
            let raw = palette_ram.get(entry).copied().unwrap_or(0);
            *slot = palette::entry_to_rgb(raw);
        }

        // Pen 0 unconditionally, not `Framebuffer::clear()` — that fills with
        // CPS-1's `BACKGROUND_PEN` (0xBFF), which is past SF1's 1,024 entries.
        // `sf.cpp:458` fills with 0, and doing it on every path (rather than only
        // when bit 5 is clear, as MAME does) is what makes a background-disabled
        // frame show pen 0 instead of the previous frame.
        self.fb.pens.fill(0);

        let flip = active & ACTIVE_FLIP != 0;

        if active & ACTIVE_BG != 0 && self.enable.bg {
            let gfx = GfxSet {
                rom: &self.bg_gfx,
                layout: &SPRITE_LAYOUT,
                colour_base: 0,
            };
            let rom = &self.tilerom;
            tilemap::draw(
                &mut self.fb.pens,
                &gfx,
                &tilemap::BG,
                |i| tilemap::bg_tile_info(rom, i),
                u32::from(bgscroll),
                flip,
                // The background never calls `set_transparent_pen`, so pen 15 draws.
                None,
            );
        }

        if active & ACTIVE_FG != 0 && self.enable.fg {
            let gfx = GfxSet {
                rom: &self.fg_gfx,
                layout: &SPRITE_LAYOUT,
                colour_base: 256,
            };
            let rom = &self.tilerom;
            tilemap::draw(
                &mut self.fb.pens,
                &gfx,
                &tilemap::FG,
                |i| tilemap::fg_tile_info(rom, i),
                u32::from(fgscroll),
                flip,
                // `set_transparent_pen(15)`, `sf.cpp:764`.
                Some(15),
            );
        }

        if active & ACTIVE_SPRITES != 0 && self.enable.sprites {
            let gfx = GfxSet {
                rom: &self.obj_gfx,
                layout: &SPRITE_LAYOUT,
                colour_base: 512,
            };
            sprites::draw(&mut self.fb.pens, &gfx, objectram, flip);
        }

        if active & ACTIVE_TX != 0 && self.enable.tx {
            let gfx = GfxSet {
                rom: &self.tx_gfx,
                layout: &CHAR_LAYOUT,
                colour_base: 768,
            };
            tilemap::draw(
                &mut self.fb.pens,
                &gfx,
                &tilemap::TX,
                |i| tilemap::tx_tile_info(videoram, i),
                // The text plane has no scroll register at all.
                0,
                flip,
                // `set_transparent_pen(3)`, `sf.cpp:765`. Three, because the char
                // layout has two planes and therefore four pens.
                Some(3),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HEIGHT, VISIBLE_X, VISIBLE_Y, WIDTH};

    /// `m_active`'s bits, from `gfxctrl_w`'s own comment block (`sf.cpp:338-348`).
    ///
    /// ```text
    /// b0 = reset, or maybe "set anyway"
    /// b1 = pulsed when control6.b6==0 until it's 1
    /// b2 = active when dip 8 (flip) on
    /// b3 = active character plane
    /// b4 = unused
    /// b5 = active background plane
    /// b6 = active middle plane
    /// b7 = active sprites
    /// ```
    #[test]
    fn the_active_bits_are_the_drivers() {
        assert_eq!(ACTIVE_FLIP, 0x04);
        assert_eq!(ACTIVE_TX, 0x08);
        assert_eq!(ACTIVE_BG, 0x20);
        assert_eq!(ACTIVE_FG, 0x40);
        assert_eq!(ACTIVE_SPRITES, 0x80);
    }

    /// Bit 5 is tested **twice**, and the second test is the one that clears.
    ///
    /// `screen_update` (`sf.cpp:455-458`) is
    /// `if (m_active & 0x20) bg->draw(); else bitmap.fill(0)`, and `gfxctrl_w`
    /// separately calls `bg->enable(data & 0x20)`. So a disabled background both
    /// skips its draw and fills the frame with pen **0** — not the previous frame,
    /// and not CPS-1's 0xBFF background pen, which is out of SF1's 1,024-entry
    /// range entirely.
    ///
    /// Redundant in MAME (the enable would have made `draw` a no-op anyway) and
    /// load-bearing here: without the fill, disabling the background leaves the
    /// last frame on screen.
    #[test]
    fn a_disabled_background_fills_the_frame_with_pen_zero() {
        let mut v = video_with_solid_bg();
        let (vram, oram, pal) = (blank_vram(), blank_oram(), flat_palette());
        // Background on: the solid tile covers the frame.
        v.render(&vram, &oram, &pal, ACTIVE_BG, 0, 0);
        assert!(
            v.fb.pens.iter().all(|&p| p == 1),
            "bg drew pen 1 everywhere"
        );
        // Background off: pen 0 everywhere, and the previous frame is gone.
        v.render(&vram, &oram, &pal, 0, 0, 0);
        assert!(v.fb.pens.iter().all(|&p| p == 0), "filled with pen 0");
    }

    /// A fresh `Sf1Video`'s buffer is cleared by the first render, not inherited.
    ///
    /// [`crate::compose::Framebuffer::new`] fills with CPS-1's `BACKGROUND_PEN`
    /// (0xBFF), which is past SF1's 1,024 entries. Every render path must write
    /// every pixel.
    #[test]
    fn the_constructors_cps1_fill_never_survives_a_render() {
        let v = Sf1Video::new(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        assert!(
            v.fb.pens.iter().any(|&p| p >= palette::ENTRIES as u16),
            "the inherited fill really is out of SF1's range"
        );
        let mut v = v;
        v.render(&blank_vram(), &blank_oram(), &flat_palette(), 0, 0, 0);
        assert!(
            v.fb.pens.iter().all(|&p| (p as usize) < palette::ENTRIES),
            "every pixel is in range after a render"
        );
    }

    /// The four planes composite back to front: bg, fg, sprites, tx.
    ///
    /// `sf.cpp:453-467`. Each of the four writes a distinct pen over the whole
    /// frame here, so the visible pen names the last plane drawn.
    #[test]
    fn the_planes_composite_in_mames_order() {
        let mut v = video_with_four_solid_planes();
        let (vram, oram, pal) = (solid_vram(), solid_oram(), flat_palette());
        let all = ACTIVE_BG | ACTIVE_FG | ACTIVE_SPRITES | ACTIVE_TX;
        // Each layer's solid pen, from `video_with_four_solid_planes`.
        v.render(&vram, &oram, &pal, ACTIVE_BG, 0, 0);
        assert_eq!(v.fb.pens[0], BG_PEN, "bg alone");
        v.render(&vram, &oram, &pal, ACTIVE_BG | ACTIVE_FG, 0, 0);
        assert_eq!(v.fb.pens[0], FG_PEN, "fg over bg");
        v.render(
            &vram,
            &oram,
            &pal,
            ACTIVE_BG | ACTIVE_FG | ACTIVE_SPRITES,
            0,
            0,
        );
        assert_eq!(v.fb.pens[0], OBJ_PEN, "sprites over fg");
        v.render(&vram, &oram, &pal, all, 0, 0);
        assert_eq!(v.fb.pens[0], TX_PEN, "tx over everything");
    }

    /// Each plane's enable bit is independent, and the sprites' is 0x80.
    #[test]
    fn each_enable_bit_gates_only_its_own_plane() {
        let mut v = video_with_four_solid_planes();
        let (vram, oram, pal) = (solid_vram(), solid_oram(), flat_palette());
        // Everything but the sprites: tx still wins, so look under it by turning
        // tx off too.
        v.render(&vram, &oram, &pal, ACTIVE_BG | ACTIVE_FG, 0, 0);
        assert_eq!(v.fb.pens[0], FG_PEN);
        v.render(&vram, &oram, &pal, ACTIVE_BG | ACTIVE_SPRITES, 0, 0);
        assert_eq!(v.fb.pens[0], OBJ_PEN, "fg off, sprites on");
        v.render(&vram, &oram, &pal, ACTIVE_BG | ACTIVE_TX, 0, 0);
        assert_eq!(v.fb.pens[0], TX_PEN, "tx on its own over bg");
        // Sprites with the background off: pen 0 fill, then sprites over it.
        v.render(&vram, &oram, &pal, ACTIVE_SPRITES, 0, 0);
        assert_eq!(v.fb.pens[0], OBJ_PEN, "no bg, sprites still draw");
    }

    /// The debugger's mask subtracts and never adds.
    ///
    /// The same `&&`-not-`||` rule [`crate::compose::LayerMask`] documents: a mask
    /// can hide what the hardware shows, and can never show what the hardware
    /// hides. Asserted in both directions.
    #[test]
    fn the_layer_mask_can_only_subtract() {
        let mut v = video_with_four_solid_planes();
        let (vram, oram, pal) = (solid_vram(), solid_oram(), flat_palette());
        let all_active = ACTIVE_BG | ACTIVE_FG | ACTIVE_SPRITES | ACTIVE_TX;
        assert_eq!(v.enable, LayerMask::all(), "the default hides nothing");
        v.enable = LayerMask {
            tx: false,
            ..LayerMask::all()
        };
        v.render(&vram, &oram, &pal, all_active, 0, 0);
        assert_eq!(v.fb.pens[0], OBJ_PEN, "the mask hid the text plane");
        // And the other way: the mask permitting a plane the hardware disabled
        // must not bring it back.
        v.enable = LayerMask::all();
        v.render(&vram, &oram, &pal, ACTIVE_BG, 0, 0);
        assert_eq!(v.fb.pens[0], BG_PEN, "fg/obj/tx stay off");
    }

    /// Masking the background does **not** substitute the pen-0 fill.
    ///
    /// The fill belongs to bit 5, which is hardware. A mask that also triggered it
    /// would make the debugger's view of a background-disabled frame differ from
    /// the hardware's, which is the one thing the view must never do.
    #[test]
    fn masking_the_background_leaves_the_hardware_fill_alone() {
        let mut v = video_with_four_solid_planes();
        let (vram, oram, pal) = (solid_vram(), solid_oram(), flat_palette());
        v.enable = LayerMask {
            bg: false,
            ..LayerMask::all()
        };
        // Hardware bg on, mask off: no bg pixels, but also no special fill — the
        // frame still starts at pen 0 and the other planes draw over it.
        v.render(&vram, &oram, &pal, ACTIVE_BG | ACTIVE_FG, 0, 0);
        assert_eq!(v.fb.pens[0], FG_PEN);
        v.render(&vram, &oram, &pal, ACTIVE_BG, 0, 0);
        assert_eq!(v.fb.pens[0], 0, "nothing drawn, so the pen-0 clear shows");
    }

    /// Screen flip comes from bit 2 and reaches the tilemaps and the sprites.
    ///
    /// `flip_screen_set(data & 0x04)` (`sf.cpp:351`), whose `state` is normalised
    /// to 0xff so any set bit is the same as all of them (`driver.cpp:319-321`).
    /// The frame comes out reversed — asserted here on the text plane, whose map is
    /// exactly the raster size.
    #[test]
    fn bit_two_flips_the_screen() {
        let mut v = video_with_one_text_pixel();
        let (oram, pal) = (blank_oram(), flat_palette());
        let vram = one_text_tile_at_origin();
        v.render(&vram, &oram, &pal, ACTIVE_TX, 0, 0);
        assert_eq!(v.fb.pens[0], TX_PEN, "unflipped at screen (0,0)");
        let unflipped = v.fb.pens.clone();
        v.render(&vram, &oram, &pal, ACTIVE_TX | ACTIVE_FLIP, 0, 0);
        let mut reversed = unflipped.to_vec();
        reversed.reverse();
        assert_eq!(
            v.fb.pens.as_ref(),
            reversed.as_slice(),
            "the frame reversed"
        );
        assert_eq!(v.fb.pens[WIDTH * HEIGHT - 1], TX_PEN);
    }

    /// Neither plane's scroll affects the other, and the text plane has none.
    #[test]
    fn the_two_scrolls_are_independent_and_the_text_plane_has_none() {
        let mut v = video_with_four_solid_planes();
        let (oram, pal) = (blank_oram(), flat_palette());
        let vram = solid_vram();
        // The solid planes are uniform, so scrolling cannot show. Assert instead
        // that the values reach the right tilemap by checking the frame is
        // unchanged for the text plane and changed for a non-uniform bg — which
        // `a_background_scroll_moves_the_background` covers. Here: the text plane
        // ignores both scroll registers entirely.
        v.render(&vram, &oram, &pal, ACTIVE_TX, 0, 0);
        let base = v.fb.pens.clone();
        v.render(&vram, &oram, &pal, ACTIVE_TX, 0x1234, 0x5678);
        assert_eq!(v.fb.pens, base, "the text plane has no scroll input");
    }

    /// The background scroll moves the background and not the foreground.
    #[test]
    fn a_background_scroll_moves_the_background_only() {
        let mut v = video_with_striped_planes();
        let (oram, pal) = (blank_oram(), flat_palette());
        let vram = blank_vram();
        v.render(&vram, &oram, &pal, ACTIVE_BG, 0, 0);
        let bg0 = v.fb.pens.clone();
        v.render(&vram, &oram, &pal, ACTIVE_BG, 1, 0);
        assert_ne!(v.fb.pens, bg0, "bgscroll moved the background");
        v.render(&vram, &oram, &pal, ACTIVE_BG, 0, 0x0FFF);
        assert_eq!(v.fb.pens, bg0, "fgscroll did not");
        // And symmetrically.
        v.render(&vram, &oram, &pal, ACTIVE_FG, 0, 0);
        let fg0 = v.fb.pens.clone();
        v.render(&vram, &oram, &pal, ACTIVE_FG, 0, 1);
        assert_ne!(v.fb.pens, fg0, "fgscroll moved the foreground");
        v.render(&vram, &oram, &pal, ACTIVE_FG, 0x0FFF, 0);
        assert_eq!(v.fb.pens, fg0, "bgscroll did not");
    }

    /// `rgb` looks a pen up in the palette RAM handed to the last render.
    ///
    /// SF1 has no palette copy step — the 68000 writes entries and MAME's
    /// `palette_device::write16` recalculates immediately — so the video keeps the
    /// converted colours from the render rather than re-reading RAM per pixel.
    #[test]
    fn rgb_converts_the_palette_of_the_last_rendered_frame() {
        let mut v = video_with_four_solid_planes();
        let mut pal = vec![0u16; palette::ENTRIES];
        pal[1] = 0x0135;
        pal[2] = 0x0FFF;
        v.render(&solid_vram(), &blank_oram(), &pal, 0, 0, 0);
        assert_eq!(
            v.rgb(1),
            [17, 51, 85],
            "the same values as palette's own test"
        );
        assert_eq!(v.rgb(2), [0xFF, 0xFF, 0xFF]);
        assert_eq!(v.rgb(0), [0, 0, 0]);
        // Out of range is black, not a panic: the pen comes from a guest-written
        // tile colour and a guest-written videoram word.
        assert_eq!(v.rgb(palette::ENTRIES as u16), [0, 0, 0]);
        assert_eq!(v.rgb(u16::MAX), [0, 0, 0]);
    }

    /// A short palette RAM leaves the rest black rather than panicking.
    #[test]
    fn a_short_palette_ram_is_tolerated() {
        let mut v = video_with_four_solid_planes();
        v.render(&solid_vram(), &blank_oram(), &[0x0FFF], 0, 0, 0);
        assert_eq!(v.rgb(0), [0xFF, 0xFF, 0xFF]);
        assert_eq!(v.rgb(1), [0, 0, 0], "past the supplied RAM");
    }

    /// A machine with no graphics at all renders a blank frame.
    ///
    /// The frontend constructs a video before a ROM set is loaded in some paths, so
    /// this must not panic.
    #[test]
    fn an_empty_machine_renders_blank() {
        let mut v = Sf1Video::new(Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let all = ACTIVE_BG | ACTIVE_FG | ACTIVE_SPRITES | ACTIVE_TX | ACTIVE_FLIP;
        v.render(&blank_vram(), &blank_oram(), &flat_palette(), all, 0, 0);
        assert!(v.fb.pens.iter().all(|&p| p == 0));
    }

    // --- fixtures -----------------------------------------------------------
    //
    // Each plane gets a distinct pen so one assertion on one pixel names the
    // winning plane. The pens are palette *entries*, so they include the layer's
    // colour base: bg 0, fg 256, obj 512, tx 768 (`sf.cpp:724-729`).

    /// bg colour 0, pen 1 -> entry 1.
    const BG_PEN: u16 = 1;
    /// fg colour 0, pen 1 -> entry 257.
    const FG_PEN: u16 = 257;
    /// obj colour 0, pen 1 -> entry 513.
    const OBJ_PEN: u16 = 513;
    /// tx colour 0, pen 1 -> entry 769.
    const TX_PEN: u16 = 769;

    /// A 16×16 sprite element whose every pixel is pen 1.
    ///
    /// Plane 3 is bit offset `half`, each byte's high nibble; plane 2 is `half + 4`,
    /// the low nibble. Setting only the high nibbles gives pen bit 0 everywhere and
    /// nothing else — pen 1, which is opaque (pen 15 is the transparent one).
    fn sprite_all_pen_one() -> Vec<u8> {
        let mut rom = vec![0u8; 128];
        for byte in rom.iter_mut().skip(64) {
            *byte = 0xF0;
        }
        rom
    }

    /// An 8×8 char element whose every pixel is pen 1.
    ///
    /// Plane 1 (offset 0) is each byte's high nibble; setting those gives pen bit 0.
    /// Pen 1 is not the text plane's transparent pen, which is 3.
    fn char_all_pen_one() -> Vec<u8> {
        vec![0xF0u8; 16]
    }

    /// A tilerom whose every entry is tile 0, colour 0, no flip — for both planes.
    fn tilerom_all_zero() -> Vec<u8> {
        // 0x40000 is the region's real size, and 0x20000 + 0x10000 + 2 is the
        // highest address `fg_tile_info` reaches for index 32,767... which is
        // 0x20000 + 0x10000 + 65,535 = 0x3ffff. So the real size is also the
        // minimum. All zeroes: code 0, colour 0, no flip.
        vec![0u8; 0x4_0000]
    }

    /// A video whose background is a solid pen-1 tile and whose other planes are
    /// empty regions (so they draw nothing).
    fn video_with_solid_bg() -> Sf1Video {
        Sf1Video::new(
            sprite_all_pen_one(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            tilerom_all_zero(),
        )
    }

    /// A video where all four planes draw a solid pen 1 in their own colour base.
    fn video_with_four_solid_planes() -> Sf1Video {
        Sf1Video::new(
            sprite_all_pen_one(),
            sprite_all_pen_one(),
            sprite_all_pen_one(),
            char_all_pen_one(),
            tilerom_all_zero(),
        )
    }

    /// Like [`video_with_four_solid_planes`], but the scrolling planes' tiles vary
    /// along x so a one-pixel scroll is observable.
    fn video_with_striped_planes() -> Sf1Video {
        let mut rom = vec![0u8; 128];
        // Plane 3 set for x = 0..3 of every row: pen 1 on the left half-tile,
        // pen 0 on the right. A one-pixel scroll shifts the boundary.
        for row in 0..16 {
            rom[64 + row * 4] = 0xF0;
        }
        Sf1Video::new(rom.clone(), rom, Vec::new(), Vec::new(), tilerom_all_zero())
    }

    /// A video whose only drawable plane is the text plane.
    fn video_with_one_text_pixel() -> Sf1Video {
        let mut rom = vec![0u8; 16 * 1024];
        // Element 1, pixel (0,0), plane 1 -> pen 1.
        rom[16] = 0x80;
        Sf1Video::new(Vec::new(), Vec::new(), Vec::new(), rom, tilerom_all_zero())
    }

    /// Videoram with element 1 in the one cell that maps to screen (0,0).
    ///
    /// Raster (64,16) is text tile (8,2), and the text map scans by rows.
    fn one_text_tile_at_origin() -> Vec<u16> {
        let mut ram = vec![0u16; tilemap::TX.tiles() as usize];
        ram[tilemap::Scan::Rows.index(8, 2, tilemap::TX.cols, tilemap::TX.rows) as usize] = 1;
        ram
    }

    /// Videoram of all zeroes: text tile 0 everywhere.
    fn blank_vram() -> Vec<u16> {
        vec![0u16; tilemap::TX.tiles() as usize]
    }

    /// Videoram of all zeroes — the text plane's tile 0 is the solid element in the
    /// four-plane fixture, so this *is* the solid case there.
    fn solid_vram() -> Vec<u16> {
        blank_vram()
    }

    /// An object table with every entry off-screen.
    fn blank_oram() -> Vec<u16> {
        let mut ram = vec![0u16; sprites::ENTRIES * sprites::STRIDE];
        for entry in 0..sprites::ENTRIES {
            ram[entry * sprites::STRIDE + 2] = 0xFFFF;
            ram[entry * sprites::STRIDE + 3] = 0xFFFF;
        }
        ram
    }

    /// An object table whose entry 0 covers screen (0,0) and the rest are off-screen.
    fn solid_oram() -> Vec<u16> {
        let mut ram = blank_oram();
        ram[0] = 0; // code 0
        ram[1] = 0; // colour 0, small, no flip
        ram[2] = VISIBLE_Y as u16;
        ram[3] = VISIBLE_X as u16;
        ram
    }

    /// A palette whose entry `n` is a distinguishable non-zero — but the
    /// compositing tests read `fb.pens`, not colours, so any fill will do.
    fn flat_palette() -> Vec<u16> {
        vec![0u16; palette::ENTRIES]
    }

    #[test]
    fn every_plane_names_the_colour_base_the_renderer_uses() {
        assert_eq!(Plane::Bg.colour_base(), 0);
        assert_eq!(Plane::Fg.colour_base(), 256);
        assert_eq!(Plane::Sprites.colour_base(), 512);
        assert_eq!(Plane::Tx.colour_base(), 768);
    }

    #[test]
    fn only_the_text_plane_uses_the_char_layout() {
        assert_eq!(Plane::Tx.layout().width, 8);
        assert_eq!(Plane::Tx.layout().planes, 2);
        for p in [Plane::Bg, Plane::Fg, Plane::Sprites] {
            assert_eq!(p.layout().width, 16);
            assert_eq!(p.layout().planes, 4);
        }
    }

    #[test]
    fn the_two_granularities_differ_and_the_text_plane_is_the_small_one() {
        // Two planes need four pens, four planes need sixteen. A hardcoded 16 puts
        // every text tile's colour four times too far up the palette.
        let v = Sf1Video::new(
            vec![0; 512],
            vec![0; 512],
            vec![0; 512],
            vec![0; 128],
            Vec::new(),
        );
        assert_eq!(v.gfx(Plane::Tx).granularity(), 4);
        assert_eq!(v.gfx(Plane::Bg).granularity(), 16);
    }

    #[test]
    fn every_plane_names_its_hardware_bit() {
        assert_eq!(Plane::Bg.active_bit(), ACTIVE_BG);
        assert_eq!(Plane::Fg.active_bit(), ACTIVE_FG);
        assert_eq!(Plane::Sprites.active_bit(), ACTIVE_SPRITES);
        assert_eq!(Plane::Tx.active_bit(), ACTIVE_TX);
        assert_eq!(
            [0x20u8, 0x40, 0x80, 0x08],
            [
                Plane::Bg.active_bit(),
                Plane::Fg.active_bit(),
                Plane::Sprites.active_bit(),
                Plane::Tx.active_bit()
            ]
        );
    }

    #[test]
    fn a_planes_permission_reads_its_own_field_of_the_mask() {
        let mut mask = LayerMask::all();
        for p in Plane::ALL {
            assert!(p.permitted(&mask), "{} starts permitted", p.name());
        }
        mask.fg = false;
        assert!(!Plane::Fg.permitted(&mask));
        assert!(Plane::Bg.permitted(&mask));
        assert!(Plane::Sprites.permitted(&mask));
        assert!(Plane::Tx.permitted(&mask));
    }

    #[test]
    fn cycling_a_plane_visits_all_four_and_returns() {
        let mut p = Plane::Bg;
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(p.name());
            p = p.cycled();
        }
        assert_eq!(seen, ["BG", "FG", "OB", "TX"]);
        assert_eq!(p, Plane::Bg);
    }

    #[test]
    fn the_index_agrees_with_all_so_a_lookup_table_can_use_it() {
        for (i, p) in Plane::ALL.into_iter().enumerate() {
            assert_eq!(p.index(), i, "{} is at {i}", p.name());
        }
    }

    #[test]
    fn each_planes_region_is_the_one_the_constructor_was_given() {
        let v = Sf1Video::new(
            vec![1; 16],
            vec![2; 32],
            vec![3; 64],
            vec![4; 128],
            vec![5; 256],
        );
        assert_eq!(v.region(Plane::Bg), &[1u8; 16][..]);
        assert_eq!(v.region(Plane::Fg), &[2u8; 32][..]);
        assert_eq!(v.region(Plane::Sprites), &[3u8; 64][..]);
        assert_eq!(v.region(Plane::Tx), &[4u8; 128][..]);
        assert_eq!(v.tilerom(), &[5u8; 256][..]);
    }

    #[test]
    fn a_gfx_set_carries_the_planes_region_layout_and_colour_base() {
        let v = Sf1Video::new(
            vec![0; 512],
            vec![0; 512],
            vec![0; 512],
            vec![0; 128],
            Vec::new(),
        );
        let g = v.gfx(Plane::Tx);
        assert_eq!(g.colour_base, 768);
        assert_eq!(g.rom.len(), 128);
        // `char_increment` is in bits: 128 bytes * 8 / 128 bits = 8 chars, frac 1/1.
        assert_eq!(g.elements(), 8);
        let g = v.gfx(Plane::Bg);
        assert_eq!(g.colour_base, 0);
        // 512 bytes * 8 / 512 bits = 8, halved by the sprite layout's frac 1/2.
        assert_eq!(g.elements(), 4);
    }

    #[test]
    fn the_element_count_is_exactly_the_last_code_that_decodes() {
        // The tile browser's in-ROM test is `code < elements()`, with no bank
        // mapper to complicate it. This is what makes that test correct.
        let v = Sf1Video::new(
            vec![0xFF; 1024],
            Vec::new(),
            Vec::new(),
            vec![0xFF; 256],
            Vec::new(),
        );
        for plane in [Plane::Bg, Plane::Tx] {
            let g = v.gfx(plane);
            let last = g.elements() - 1;
            let (w, h) = (g.layout.width, g.layout.height);
            for y in 0..h {
                for x in 0..w {
                    assert!(
                        g.pen(last, x, y).is_some(),
                        "{} code {last} ({x},{y})",
                        plane.name()
                    );
                }
            }
            assert!(
                g.pen(g.elements(), 0, 0).is_none(),
                "{} one past the end",
                plane.name()
            );
        }
    }
}
