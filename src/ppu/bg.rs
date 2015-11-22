//! Background layer rendering

use super::{Ppu, Rgb};

/// Collected background settings
struct BgSettings {
    /// Mosaic pixel size (1-16). 1 = Normal pixels.
    /// FIXME: I think there's a difference between disabled and enabled with 1x1 mosaic size in
    /// some modes (highres presumably)
    #[allow(dead_code)] // FIXME NYI
    mosaic: u8,
    /// Tilemap word address in VRAM
    /// "Starting at the tilemap address, the first $800 bytes are for tilemap A. Then come the
    /// $800 bytes for B, then C then D."
    tilemap_word_addr: u16,
    /// When `true`, this BGs tilemaps are mirrored sideways
    tilemap_mirror_h: bool,
    /// When `true`, this BGs tilemaps are mirrored downwards
    tilemap_mirror_v: bool,
    /// Either 8 or 16.
    tile_size: u8,
    /// Character Data start address in VRAM
    chr_addr: u16,
    hscroll: u16,
    vscroll: u16,
}

/// Unpacked tilemap entry for internal (rendering) use
struct TilemapEntry {
    #[allow(dead_code)] // FIXME
    vflip: bool,
    #[allow(dead_code)]
    hflip: bool,
    /// Priority bit (0-1)
    priority: u8,
    /// Tile palette (0-7)
    palette: u8,
    /// Index into the character/tile data, where the actual tile is stored
    tile_number: u16,
}

impl Ppu {
    /// Determines whether the given BG layer (1-4) is enabled
    fn bg_enabled(&self, bg: u8) -> bool { self.tm & (1 << (bg - 1)) != 0 }

    /// Reads the tilemap entry at the given VRAM word address.
    ///     vhopppcc cccccccc (high, low)
    ///     v/h        = Vertical/Horizontal flip this tile.
    ///     o          = Tile priority.
    ///     ppp        = Tile palette base.
    ///     cccccccccc = Tile number.
    fn tilemap_entry(&self, word_address: u16) -> TilemapEntry {
        let byte_address = word_address << 1;
        let lo = self.vram[byte_address];
        let hi = self.vram[byte_address + 1];

        TilemapEntry {
            vflip: hi & 0x80 != 0,
            hflip: hi & 0x40 != 0,
            priority: (hi & 0x20) >> 5,
            palette: (hi & 0x1c) >> 2,
            tile_number: ((hi as u16 & 0x03) << 8) | lo as u16,
        }
    }

    /// Collects properties of a background layer
    fn bg_settings(&self, bg: u8) -> BgSettings {
        // The BGxSC register for our background layer
        let bgsc = match bg {
            1 => self.bg1sc,
            2 => self.bg2sc,
            3 => self.bg3sc,
            4 => self.bg4sc,
            _ => unreachable!(),
        };
        // Chr (Tileset, not Tilemap) start (word?) address >> 12
        let chr = match bg {
            1 => self.bg12nba & 0x0f,
            2 => (self.bg12nba & 0xf0) >> 4,
            3 => self.bg34nba & 0x0f,
            4 => (self.bg34nba & 0xf0) >> 4,
            _ => unreachable!(),
        };
        let (hofs, vofs) = match bg {
            1 => (self.bg1hofs, self.bg1vofs),
            2 => (self.bg2hofs, self.bg2vofs),
            3 => (self.bg3hofs, self.bg3vofs),
            4 => (self.bg4hofs, self.bg4vofs),
            _ => unreachable!(),
        };

        BgSettings {
            mosaic: if self.mosaic & (1 << (bg-1)) == 0 {
                1
            } else {
                ((self.mosaic & 0xf0) >> 4) + 1
            },
            // FIXME: This looks more like the byte address to me!
            tilemap_word_addr: ((bgsc as u16 & 0xfc) >> 2) << 10,
            tilemap_mirror_h: bgsc & 0b01 == 0, // inverted bit value
            tilemap_mirror_v: bgsc & 0b10 == 0, // inverted bit value
            tile_size: match self.bg_mode() {
                // "If the BG character size for BG1/BG2/BG3/BG4 bit is set, then the BG is made of
                // 16x16 tiles. Otherwise, 8x8 tiles are used. However, note that Modes 5 and 6
                // always use 16-pixel wide tiles, and Mode 7 always uses 8x8 tiles."
                5 | 6 => 16,
                7 => 8,
                _ => {
                    // BGMODE: `4321----` (`-` = not relevant here)
                    if self.bgmode & (1 << (bg + 3)) == 0 {
                        8
                    } else {
                        16
                    }
                }
            },
            chr_addr: (chr as u16) << 12,
            hscroll: hofs,
            vscroll: vofs,
        }
    }

    /// Returns the number of colors in the given BG layer in the current BG mode (4, 16, 128 or
    /// 256). `X` denotes a BG for offset-per-tile data.
    ///
    ///     Mode    # Colors for BG
    ///              1   2   3   4
    ///     ======---=---=---=---=
    ///     0        4   4   4   4
    ///     1       16  16   4   -
    ///     2       16  16   X   -
    ///     3      256  16   -   -
    ///     4      256   4   X   -
    ///     5       16   4   -   -
    ///     6       16   -   X   -
    ///     7      256   -   -   -
    ///     7EXTBG 256 128   -   -
    fn color_count_for_bg(&self, bg: u8) -> u16 {
        match self.bg_mode() {
            0 => 4,
            1 => match bg {
                1 | 2 => 16,
                3 => 4,
                _ => unreachable!(),
            },
            2 => 16,
            3 => match bg {
                1 => 256,
                2 => 16,
                _ => unreachable!(),
            },
            4 => match bg {
                1 => 256,
                2 => 4,
                _ => unreachable!(),
            },
            5 => match bg {
                1 => 16,
                2 => 4,
                _ => unreachable!(),
            },
            6 => 16,
            7 => panic!("NYI: color_count_for_bg for mode 7"),   // (make sure to handle EXTBG)
            _ => unreachable!(),
        }
    }

    /// Calculates the palette base index for a tile in the given background layer. `tile_palette`
    /// is the palette number stored in the tilemap entry (the 3 `p` bits).
    fn palette_base_for_bg_tile(&self, bg: u8, palette_num: u8) -> u8 {
        debug_assert!(bg >= 1 && bg <= 4);
        match self.bg_mode() {
            0 => palette_num * 4 + (bg - 1) * 32,
            1 | 5 => palette_num * self.color_count_for_bg(bg) as u8,   // doesn't have 256 colors
            2 => palette_num * 16,
            3 => match bg {
                1 => 0,
                2 => palette_num * 16,
                _ => unreachable!(),    // no BG3/4
            },
            4 => match bg {
                1 => 0,
                2 => palette_num * 4,
                _ => unreachable!(),    // no BG3/4
            },
            6 => palette_num * 16,      // BG1 has 16 colors
            7 => panic!("NYI: palette_base_for_bg_tile for mode 7"),
            _ => unreachable!(),
        }
    }


    /// Lookup the color of the given background layer (1-4) at the current pixel, using the given
    /// priority (0-1) only. This will also scroll backgrounds accordingly and apply color math.
    ///
    /// Returns `None` if the pixel is transparent, `Some(Rgb)` otherwise.
    pub fn lookup_bg_color(&self, bg_num: u8, prio: u8) -> Option<Rgb> {
        debug_assert!(bg_num >= 1 && bg_num <= 4);
        debug_assert!(prio == 0 || prio == 1);
        if !self.bg_enabled(bg_num) { return None }

        // Apply BG scrolling and get the tile coordinates
        // FIXME Apply mosaic filter
        // FIXME Fix this: "Note that many games will set their vertical scroll values to -1 rather
        // than 0. This is because the SNES loads OBJ data for each scanline during the previous
        // scanline. The very first line, though, wouldn’t have any OBJ data loaded! So the SNES
        // doesn’t actually output scanline 0, although it does everything to render it. These
        // games want the first line of their tilemap to be the first line output, so they set
        // their VOFS registers in this manner. Note that an interlace screen needs -2 rather than
        // -1 to properly correct for the missing line 0 (and an emulator would need to add 2
        // instead of 1 to account for this)."
        let x = self.x;
        let y = self.scanline;
        let bg = self.bg_settings(bg_num);
        let tile_size = bg.tile_size;
        let (xscroll, yscroll) = (bg.hscroll, bg.vscroll);
        let tile_x = x.wrapping_add(xscroll) / tile_size as u16;
        let tile_y = y.wrapping_add(yscroll) / tile_size as u16;
        let off_x = (x.wrapping_add(xscroll) % tile_size as u16) as u8;
        let off_y = (y.wrapping_add(yscroll) % tile_size as u16) as u8;
        let (sx, sy) = (!bg.tilemap_mirror_h, !bg.tilemap_mirror_v);

        // Calculate the VRAM word address, where the tilemap entry for our tile is stored
        // FIXME Check if this really is correct
        let tilemap_entry_word_address =
            bg.tilemap_word_addr |
            ((tile_y & 0x1f) << 5) |
            (tile_x & 0x1f) |
            if sy {(tile_y & 0x20) << if sx {6} else {5}} else {0} |
            if sx {(tile_x & 0x20) << 5} else {0};
        let tilemap_entry = self.tilemap_entry(tilemap_entry_word_address);
        if tilemap_entry.priority != prio { return None }

        let color_count = self.color_count_for_bg(bg_num);
        debug_assert!(color_count.is_power_of_two());  // should be power of two
        if color_count == 256 {
            debug_assert!(self.cgwsel & 0x01 == 0, "NYI: direct color mode");
        }

        // Calculate the number of bitplanes needed to store a color in this BG
        let bitplane_count = (color_count - 1).count_ones() as u16;

        // FIXME: Formula taken from the wiki, is this correct? In particular: `chr_addr<<1`?
        let bitplane_start_addr =
            (bg.chr_addr << 1) +
            (tilemap_entry.tile_number * 8 * bitplane_count);   // 8 bytes per bitplane

        let palette_base = self.palette_base_for_bg_tile(bg_num, tilemap_entry.palette);
        let palette_index = self.read_chr_entry(bitplane_count as u8,
                                                bitplane_start_addr,
                                                tile_size,
                                                (off_x, off_y));

        match palette_index {
            0 => None,
            _ => Some(self.lookup_color(palette_base + palette_index)),
        }
    }
}
