//! Development hook: `DAW_SCREENSHOT=<file.bmp>` saves the window after a
//! delay (`DAW_SCREENSHOT_DELAY` seconds, default 8) and quits.

use std::io::Write;
use std::path::Path;

use iced::window::Screenshot;

/// Write a 32-bit bottom-up BMP.
pub fn write_bmp(path: &Path, shot: &Screenshot) -> std::io::Result<()> {
    let (width, height) = (shot.size.width, shot.size.height);
    let pixels = width * height * 4;
    let mut out = Vec::with_capacity(54 + pixels as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + pixels).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    for row in (0..height).rev() {
        let start = (row * width * 4) as usize;
        for pixel in shot.rgba[start..start + (width * 4) as usize].as_chunks::<4>().0 {
            out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
        }
    }
    std::fs::File::create(path)?.write_all(&out)
}
