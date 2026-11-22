// SPDX-License-Identifier: GPL-3.0-or-later
//! An 8-bit RGBA image: the compositor's CPU readback format and the unit of golden comparison.

/// Interleaved 8-bit RGBA with straight alpha, tightly packed rows (`width * 4` bytes each).
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Image({}x{})", self.width, self.height)
    }
}

impl Image {
    /// A `width × height` image filled with `rgba`.
    #[must_use]
    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Image {
        let n = width as usize * height as usize;
        Image {
            width,
            height,
            data: rgba.repeat(n),
        }
    }

    /// The pixel at `(x, y)`.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    /// Sets the pixel at `(x, y)`.
    pub fn set_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.data[i..i + 4].copy_from_slice(&rgba);
    }

    /// Whether `width`, `height` and the buffer length agree.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.data.len() == self.width as usize * self.height as usize * 4
    }
}
