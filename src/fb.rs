//! The Linux framebuffer: the screen as a plain file of pixels.
//! No GPU, no window system, nothing running between slides.

use crate::Res;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};

const DEVICE: &str = "/dev/fb0";
const SYSFS: &str = "/sys/class/graphics/fb0";

pub struct Fb {
    file: File,
    pub width: usize,
    pub height: usize,
    bytes_per_pixel: usize,
    stride: usize,
    buf: Vec<u8>,
}

impl Fb {
    pub fn open() -> Res<Fb> {
        let sys = |name: &str| fs::read_to_string(format!("{SYSFS}/{name}"));
        let size = sys("virtual_size")?;
        let (w, h) = size.trim().split_once(',').ok_or("bad virtual_size")?;
        let (width, height): (usize, usize) = (w.parse()?, h.parse()?);
        let bytes_per_pixel = sys("bits_per_pixel")?.trim().parse::<usize>()? / 8;
        let stride: usize = sys("stride")?.trim().parse()?;
        if bytes_per_pixel != 2 && bytes_per_pixel != 4 {
            return Err("framebuffer must be 16 or 32 bits per pixel".into());
        }
        Ok(Fb {
            file: OpenOptions::new().write(true).open(DEVICE)?,
            width,
            height,
            bytes_per_pixel,
            stride,
            buf: vec![0; stride * height],
        })
    }

    /// Put a width*height RGB image (3 bytes per pixel) on the screen.
    pub fn show(&mut self, rgb: &[u8]) -> Res<()> {
        for y in 0..self.height {
            let row = &mut self.buf[y * self.stride..];
            for x in 0..self.width {
                let p = &rgb[(y * self.width + x) * 3..];
                let (r, g, b) = (p[0], p[1], p[2]);
                if self.bytes_per_pixel == 4 {
                    row[x * 4..x * 4 + 4].copy_from_slice(&[b, g, r, 255]);
                } else {
                    let v = (r as u16 >> 3) << 11 | (g as u16 >> 2) << 5 | b as u16 >> 3;
                    row[x * 2..x * 2 + 2].copy_from_slice(&v.to_le_bytes());
                }
            }
        }
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&self.buf)?;
        Ok(())
    }
}
