//! Common PPU rendering code (used by sprite and BG rendering)
//!
//! # Terminology
//!
//! * **Tile**/**Character**: A tile consists of 8x8 or 16x16 pixels worth of color indices and is
//!   what makes up both sprites and backgrounds. Sprites are always made up of 8x8 tiles, while
//!   backgrounds can use 16x16 pixel tiles by setting the appropriate bit in `BGMODE ($2105)`.
//! * **Background**: A background is a big, connected layer of tiles that can (only) be moved as a
//!   unit. Depending on the background mode, between 1 and 4 BG layers can be used, each with
//!   between 4 and 256 colors.
//! * **Sprite**: A sprite is an independently movable object up to 64x64 pixels in size. Sprites
//!   always use 8x8 tiles and 16 colors, while each sprite can use a different palette. Sprites can
//!   not be rotated (rotating objects in games are either prerendered or make use of BG mode 7,
//!   which allows full matrix transformations on the layer).
//! * **Name table**/**Character table**/**Character map**: These tables store tile data: Maps of
//!   color indices to use for the tile's pixels. Sprites and all background modes except mode 7
//!   store the color indices in "bitplanes", which makes their decoding tricky: A bitplane stores 1
//!   bit for every pixel of the tile. Two bitplanes are interleaved: Bitplane 0 is stored in the
//!   low byte, while bitplane 1 is stored in the high byte.
//! * **Tile map**: Backgrounds consist of 1, 2 or 4 tile maps, each storing data for 32x32 tiles.
//!   For each tile on the background, the tile map stores the tile's priority and H/V flip bits,
//!   the palette, and the tile number. If 16x16 tiles are enabled, each entry in the tile map
//!   stores data for a 16x16 tile consisting of 4 8x8 tiles: `TILE`, `TILE+1`, `TILE+16` and
//!   `TILE+17`, where `TILE` is the stored tile number.

use super::{Ppu, Rgb, SnesRgb};

/// An enum of all layers a pixel can come from
enum Layer {
    Bg1,
    Bg2,
    Bg3,
    Bg4,
    Obj {
        /// Sprites with a palette 0-3 are opaque and cannot participate in color math
        opaque: bool,
    },
    Backdrop,
}

enum WindowOp { And, Or, Xor, XNor }
use self::WindowOp::{ And, Or, Xor, XNor };

/// Masking data per layer
struct Mask {
    w1_en: bool,
    w2_en: bool,
    w1_inv: bool,
    w2_inv: bool,
    wop: WindowOp
}

impl Mask {
    fn new(mask_reg: u8, mask_offset: usize, op_reg: u8, op_offset: usize) -> Self {
        let mask_bits = (mask_reg >> mask_offset) & 0b1111;
        let op_bits = (op_reg >> op_offset) & 0b11;
        
        Mask {
            w1_en: mask_bits & 0b0010 != 0,
            w2_en: mask_bits & 0b1000 != 0,
            w1_inv: mask_bits & 0b0001 != 0,
            w2_inv: mask_bits & 0b0100 != 0,
            wop: match op_bits {
                0b00 => Or,
                0b01 => And,
                0b10 => Xor,
                _ => XNor
            }
        }
    }
    
    /// Determine masking result based on windows.
    fn check(&self, in_w1: bool, in_w2: bool) -> bool {
        let (w1, w2) = (in_w1 ^ self.w1_inv, in_w2 ^ self.w2_inv);
        
        match (self.w1_en, self.w2_en) {
            (false, false) => false,
            (true, false) => w1,
            (false, true) => w2,
            (true, true) => match self.wop {
                And => w1 && w2,
                Or => w1 || w2,
                Xor => w1 ^ w2,
                XNor => !(w1 ^ w2)
            }
        }
    }
}

/// Rendering
impl Ppu {
    /// Get the configured sprite size in pixels. If `size_toggle` is `false`, gets the size of
    /// small sprites, otherwise gets the size of large sprites (OAM size bit set).
    pub fn obj_size(&self, size_toggle: bool) -> (u8, u8) {
        match self.obsel >> 5 & 0b111 {
            0b000 => if !size_toggle {(8,8)} else {(16,16)},
            0b001 => if !size_toggle {(8,8)} else {(32,32)},
            0b010 => if !size_toggle {(8,8)} else {(64,64)},
            0b011 => if !size_toggle {(16,16)} else {(32,32)},
            0b100 => if !size_toggle {(16,16)} else {(64,64)},
            0b101 => if !size_toggle {(32,32)} else {(64,64)},

            0b110 => if !size_toggle {(16,32)} else {(32,64)},
            0b111 => if !size_toggle {(16,32)} else {(32,32)},
            _ => unreachable!(),
        }
    }

    /// Returns the active BG mode (0-7).
    pub fn bg_mode(&self) -> u8 { self.bgmode & 0b111 }

    /// Returns the backdrop color used as a default color (with color math applied, if enabled).
    fn backdrop_color(&self) -> SnesRgb {
        self.cgram.get_color(0)
    }

    /// Renders a "raw" pixel (not doing color math), and returns the color and the layer it came
    /// from.
    ///
    /// If `sub` is true, fetches the pixel from the subscreen. Otherwise, the main screen is used.
    fn get_raw_pixel(&mut self, subscreen: bool) -> (SnesRgb, Layer) {
        macro_rules! e {
            ( $e:expr ) => ( $e );
        }

        macro_rules! bglayer {
            ( 1 ) => { Layer::Bg1 };
            ( 2 ) => { Layer::Bg2 };
            ( 3 ) => { Layer::Bg3 };
            ( 4 ) => { Layer::Bg4 };
        }

        // Enable/disable masking for each mask (except color)
        // Color math & color window settings are read from CGWSEL below
        let enable_mask_reg = if subscreen { self.tsw } else { self.tmw };
        let enable_bg_1_mask = (enable_mask_reg & 0b00001) != 0;
        let enable_bg_2_mask = (enable_mask_reg & 0b00010) != 0;
        let enable_bg_3_mask = (enable_mask_reg & 0b00100) != 0;
        let enable_bg_4_mask = (enable_mask_reg & 0b01000) != 0;
        let enable_sprite_mask = (enable_mask_reg & 0b10000) != 0;

        // Set up all six masks from mask configurations
        let mask_bg_1 = Mask::new(self.w12sel, 0, self.wbglog, 0);
        let mask_bg_2 = Mask::new(self.w12sel, 4, self.wbglog, 2);
        let mask_bg_3 = Mask::new(self.w34sel, 0, self.wbglog, 4);
        let mask_bg_4 = Mask::new(self.w34sel, 4, self.wbglog, 6);
        let mask_sprites = Mask::new(self.wobjsel, 0, self.wobjlog, 0);
        let mask_color = Mask::new(self.wobjsel, 4, self.wobjlog, 2);

        // Check current pixel to get W1 and W2
        let in_w1 = self.x >= (self.wh0 as u16) && self.x < (self.wh1 as u16);
        let in_w2 = self.x >= (self.wh2 as u16) && self.x < (self.wh3 as u16);

        macro_rules! mask_layer {
            ( 1 ) => { enable_bg_1_mask && mask_bg_1.check(in_w1, in_w2) };
            ( 2 ) => { enable_bg_2_mask && mask_bg_2.check(in_w1, in_w2) };
            ( 3 ) => { enable_bg_3_mask && mask_bg_3.check(in_w1, in_w2) };
            ( 4 ) => { enable_bg_4_mask && mask_bg_4.check(in_w1, in_w2) };
            ( sprites ) => { enable_sprite_mask && mask_sprites.check(in_w1, in_w2) };
        }

        // Enable/disable color clipping using mask settings, masks and cgwsel.
        let clip_color = {
            match (self.cgwsel >> 6, mask_color.check(in_w1, in_w2)) {
                (0b11, _) => true,     // Always clip
                (0b01, false) => true, // Clip outside window
                (0b10, true) => true,  // Clip inside window
                _ => false
            }
        };

        // This macro gets the current pixel from a tile with given priority in the given layer.
        // If the pixel is non-transparent and non-masked, it will return its RGB value (after
        // color clipping). If it is transparent or masked, it will do nothing (ie. the code
        // following this macro is executed).

        // FIXME: It's not clear whether colors should clip to black or color 0. I've seen both indicated.
        // Clip to color 0 for now.
        let clip_to_color = self.backdrop_color();
        
        macro_rules! try_layer {
            ( Sprites with priority $prio:tt ) => {
                if let Some((rgb, opaque)) = self.maybe_draw_sprite_pixel(e!($prio), subscreen) {
                    if !mask_layer!(sprites) {
                        let rgb_post_clip = if !clip_color { rgb } else { clip_to_color };
                        
                        return (rgb_post_clip, Layer::Obj { opaque: opaque });
                    }
                }
            };
            ( BG $bg:tt tiles with priority $prio:tt ) => {
                if let Some(rgb) = self.lookup_bg_color(e!($bg), e!($prio), subscreen) {
                    if !mask_layer!($bg) {
                        let rgb_post_clip = if !clip_color { rgb } else { clip_to_color };
                        
                        return (rgb_post_clip, bglayer!($bg));
                    }
                }
            };
        }

        match self.bg_mode() {
            0 => {
                // I love macros <3
                try_layer!(Sprites with priority 3);
                try_layer!(BG 1 tiles with priority 1);
                try_layer!(BG 2 tiles with priority 1);
                try_layer!(Sprites with priority 2);
                try_layer!(BG 1 tiles with priority 0);
                try_layer!(BG 2 tiles with priority 0);
                try_layer!(Sprites with priority 1);
                try_layer!(BG 3 tiles with priority 1);
                try_layer!(BG 4 tiles with priority 1);
                try_layer!(Sprites with priority 0);
                try_layer!(BG 3 tiles with priority 0);
                try_layer!(BG 4 tiles with priority 0);
            }
            1 => {
                if self.bgmode & 0x08 != 0 { try_layer!(BG 3 tiles with priority 1); }
                try_layer!(Sprites with priority 3);
                try_layer!(BG 1 tiles with priority 1);
                try_layer!(BG 2 tiles with priority 1);
                try_layer!(Sprites with priority 2);
                try_layer!(BG 1 tiles with priority 0);
                try_layer!(BG 2 tiles with priority 0);
                try_layer!(Sprites with priority 1);
                if self.bgmode & 0x08 == 0 { try_layer!(BG 3 tiles with priority 1); }
                try_layer!(Sprites with priority 0);
                try_layer!(BG 3 tiles with priority 0);
            }
            2 ... 5 => {
                // FIXME Do the background priorities differ here?
                try_layer!(Sprites with priority 3);
                try_layer!(BG 1 tiles with priority 1);
                try_layer!(Sprites with priority 2);
                try_layer!(BG 2 tiles with priority 1);
                try_layer!(Sprites with priority 1);
                try_layer!(BG 1 tiles with priority 0);
                try_layer!(Sprites with priority 0);
                try_layer!(BG 2 tiles with priority 0);
            }
            6 => {
                try_layer!(Sprites with priority 3);
                try_layer!(BG 1 tiles with priority 1);
                try_layer!(Sprites with priority 2);
                try_layer!(Sprites with priority 1);
                try_layer!(BG 1 tiles with priority 0);
                try_layer!(Sprites with priority 0);
            }
            7 => {
                let extbg = self.setini & 0x40 != 0;
                try_layer!(Sprites with priority 3);
                try_layer!(Sprites with priority 2);
                if extbg { try_layer!(BG 2 tiles with priority 1); }
                try_layer!(Sprites with priority 1);
                try_layer!(BG 1 tiles with priority 0);    // priority is ignored
                try_layer!(Sprites with priority 0);
                if extbg { try_layer!(BG 2 tiles with priority 0); }
            }
            _ => unreachable!(),
        }

        (self.backdrop_color(), Layer::Backdrop)
    }

    fn color_math_enabled(&self, layer: Layer) -> bool {
        let bit = match layer {
            Layer::Bg1 => 0,
            Layer::Bg2 => 1,
            Layer::Bg3 => 2,
            Layer::Bg4 => 3,
            Layer::Obj { opaque: false } => 4,
            Layer::Obj { opaque: true } => return false,    // No color math for you!
            Layer::Backdrop => 5,
        };

        if (self.cgadsub & (1 << bit)) == 0 {
            return false;
        }

        let in_w1 = self.x >= (self.wh0 as u16) && self.x < (self.wh1 as u16);
        let in_w2 = self.x >= (self.wh2 as u16) && self.x < (self.wh3 as u16);

        let mask = Mask::new(self.wobjsel, 4, self.wobjlog, 2);
        
        // Apply color mask & settings in cgwsel
        match ((self.cgwsel >> 4) & 0b11, mask.check(in_w1, in_w2)) {
            (0b11, _) => false,     // Always
            (0b01, false) => false, // Outside window
            (0b10, true) => false,  // Inside window
            _ => true
        }
    }

    /// Main rendering entry point. Renders the current pixel and returns its color. Assumes that
    /// the current pixel is on the screen.
    pub fn render_pixel(&mut self) -> Rgb {
        assert!(self.x < super::SCREEN_WIDTH as u16);
        assert!(self.scanline < super::SCREEN_HEIGHT as u16);

        if self.forced_blank() {
            return Rgb {r: 0, g: 0, b: 0};
        }

        if self.x == 0 && self.scanline == 0 {
            // Sprite overflow flags are reset "at the end of VBlank"
            // FIXME Is this correct or is the time wrong?
            self.range_over = false;
            self.time_over = false;

            trace!("New frame. BG mode {}, layers enabled: {:05b}, sprites are {:?} or {:?}",
                   self.bg_mode(),
                   self.tm & 0x1f,
                   self.obj_size(false),
                   self.obj_size(true));
        }

        if self.x == 0 {
            // Entered new scanline.
            self.collect_sprite_data_for_scanline();
        }

        let (main_pix_color, main_pix_layer) = self.get_raw_pixel(false);
        let post_math_color = if self.color_math_enabled(main_pix_layer) {
            let math_color = if self.cgwsel & 0x02 == 0 {
                // Fixed color. Note that the fixed color is also used as the subscreen's backdrop
                // color.
                SnesRgb::new(self.coldata_r, self.coldata_g, self.coldata_b)
            } else {
                // Subscreen
                let (sub_color, sub_layer) = self.get_raw_pixel(true);
                match sub_layer {
                    Layer::Backdrop => {
                        // Use COLDATA color as backdrop (FIXME a bit hacky, but is it too bad?)
                        SnesRgb::new(self.coldata_r, self.coldata_g, self.coldata_b)
                    }
                    _ => sub_color,
                }
            };

            // FIXME: Disable half-math when color is clipped.
            if self.cgadsub & 0x80 == 0 {
                // Add
                main_pix_color.saturating_add(&math_color)
            } else {
                // Subtract
                main_pix_color.saturating_sub(&math_color)
            }
        } else {
            // No color math
            main_pix_color
        };

        let brightness = self.brightness() as u16;
        let final_color = if brightness == 0 {
            // This isn't actually correct: The image is still (barely) visible. So barely that this
            // makes basically no difference.
            SnesRgb::new(0, 0, 0)
        } else {
            SnesRgb::new(
                (post_math_color.r() as u16 * (brightness + 1) / 16) as u8,
                (post_math_color.g() as u16 * (brightness + 1) / 16) as u8,
                (post_math_color.b() as u16 * (brightness + 1) / 16) as u8,
            )
        };

        final_color.to_adjusted_rgb()
    }

    /// Reads character data for a pixel and returns the palette index stored in the bitplanes.
    ///
    /// # Parameters
    /// * `bitplane_count`: Number of bitplanes (must be even)
    /// * `start_addr`: Address of the first bitplane (or the first 2)
    /// * `tile_size`: 8 or 16
    /// * `(x, y)`: Offset inside the tile (`0-7` or `0-15`, depending on the tile size)
    /// * `(vflip, hflip)`: Flip this tile vertically (top and down are flipped) or horizontally
    ///   (left and right are flipped)
    pub fn read_chr_entry(&self,
                          bitplane_count: u8,
                          start_addr: u16,
                          tile_size: u8,
                          (x, y): (u8, u8),
                          (vflip, hflip): (bool, bool)) -> u8 {
        // 2 bitplanes are stored interleaved with each other, so there can only be an even number
        debug_assert!(bitplane_count & 1 == 0, "odd bitplane count");
        debug_assert!(x <= 7 || (x <= 15 && tile_size == 16), "invalid x value: {}", x);
        debug_assert!(y <= 7 || (y <= 15 && tile_size == 16), "invalid y value: {}", y);
        debug_assert!(tile_size == 8, "non-8x8 tiles unsupported"); // FIXME support 16x16 tiles
        let bitplane_pairs = bitplane_count >> 1;

        // Flip coordinates, if necessary
        let x = if hflip { tile_size - x - 1 } else { x };
        let y = if vflip { tile_size - y - 1 } else { y };

        let mut palette_index = 0u8;
        for i in 0..bitplane_pairs {
            let bitplane_bits = self.read_2_bitplanes(
                start_addr + i as u16 * 16, // 16 Bytes per pair of bitplanes
                (x, y));
            palette_index |= bitplane_bits << (2 * i);
        }

        palette_index
    }

    /// Reads 2 bits of the given coordinate within the bitplane's tile from 2 interleaved
    /// bitplanes.
    ///
    /// # Parameters
    /// * `bitplanes_start`: Start address of the bitplanes
    /// * `(x_off, y_off)`: Offset into the tile (`0-7`)
    fn read_2_bitplanes(&self, bitplanes_start: u16, (x_off, y_off): (u8, u8)) -> u8 {
        // Bit 0 in low bytes, bit 1 in high bytes
        let lo = self.vram[bitplanes_start + y_off as u16 * 2];
        let hi = self.vram[bitplanes_start + y_off as u16 * 2 + 1];
        // X values in a byte: 01234567
        let bit0 = (lo >> (7 - x_off)) & 1;
        let bit1 = (hi >> (7 - x_off)) & 1;

        (bit1 << 1) | bit0
    }
}
