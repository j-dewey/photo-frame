//! The Linux framebuffer: the screen as a plain file of pixels.
//! No GPU, no window system, nothing running between slides.
//!
//! The Pi only creates /dev/fb0 when a screen is connected (unless HDMI is
//! forced on, see the README). While it is missing:
//!   - if `fb_file` is set in photos.conf, frames go to that file instead,
//!     as raw 32-bit BGRA pixels at `fb_size`;
//!   - otherwise Fb runs headless: nothing is written.
//! Either way, each `reconnect` looks for /dev/fb0 again and switches to it
//! as soon as it appears. See "Testing without a screen" in the README.

use crate::config::Config;
use crate::Res;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const DEVICE: &str = "/dev/fb0";
const SYSFS: &str = "/sys/class/graphics/fb0";

/// An open /dev/fb0 (or fb_file) and its pixel layout.
struct Device {
    file: File,
    /// True for /dev/fb0, false for the fb_file stand-in.
    is_screen: bool,
    bytes_per_pixel: usize,
    stride: usize,
    /// One screen of pixels in the device's format, written in one go.
    buf: Vec<u8>,
}

pub struct Fb {
    /// None while running headless (no screen and no fb_file).
    device: Option<Device>,
    /// Where frames go while there is no screen, from photos.conf.
    fb_file: Option<PathBuf>,
    /// Size of `fb_file` frames, and of the drawing while headless.
    fb_size: (usize, usize),
    pub width: usize,
    pub height: usize,
}

impl Fb {
    /// Open the screen, or fb_file, or start headless.
    pub fn open(config: &Config) -> Fb {
        let mut fb = Fb {
            device: None,
            fb_file: config.fb_file.clone(),
            fb_size: config.fb_size,
            width: config.fb_size.0,
            height: config.fb_size.1,
        };
        if let Err(e) = fb.connect() {
            println!("[Fb] Unable to conenct to file {:?}", fb.fb_file);
            println!("[Fb] Running headless until a screen appears ({e})");
        }
        fb
    }

    /// Unless the screen is already open, look for it again (falling back to
    /// fb_file). Returns true if the size changed, meaning anything drawn so
    /// far is the wrong size.
    pub fn reconnect(&mut self) -> bool {
        self.connect().unwrap_or(false)
    }

    fn connect(&mut self) -> Res<bool> {
        if self.device.as_ref().is_some_and(|d| d.is_screen) {
            return Ok(false);
        }
        let (device, width, height) = match probe_screen() {
            Ok(found) => {
                println!("[Fb] Found screen: {}x{}", found.1, found.2);
                found
            }
            // Already writing to fb_file: keep at it.
            Err(_) if self.device.is_some() => return Ok(false),
            Err(e) => {
                let Some(path) = &self.fb_file else {
                    return Err(e);
                };
                let found = probe_file(path, self.fb_size)?;
                println!(
                    "[Fb] No screen ({e}); writing {}x{} frames to {:?}",
                    found.1, found.2, path
                );
                found
            }
        };
        self.device = Some(device);
        let resized = (width, height) != (self.width, self.height);
        (self.width, self.height) = (width, height);
        Ok(resized)
    }

    /// Put a width*height RGB image (3 bytes per pixel) on the screen. Does
    /// nothing while headless. If the screen goes away, goes headless.
    pub fn show(&mut self, rgb: &[u8]) -> Res<()> {
        let Some(dev) = &mut self.device else {
            return Ok(());
        };
        for y in 0..self.height {
            let row = &mut dev.buf[y * dev.stride..];
            for x in 0..self.width {
                let p = &rgb[(y * self.width + x) * 3..];
                let (r, g, b) = (p[0], p[1], p[2]);
                if dev.bytes_per_pixel == 4 {
                    row[x * 4..x * 4 + 4].copy_from_slice(&[b, g, r, 255]);
                } else {
                    let v = (r as u16 >> 3) << 11 | (g as u16 >> 2) << 5 | b as u16 >> 3;
                    row[x * 2..x * 2 + 2].copy_from_slice(&v.to_le_bytes());
                }
            }
        }
        let written = dev
            .file
            .seek(SeekFrom::Start(0))
            .and_then(|_| dev.file.write_all(&dev.buf));
        if let Err(e) = written {
            println!("[Fb] Lost the screen ({e}); looking for it again");
            self.device = None;
        }
        Ok(())
    }
}

/// Open /dev/fb0 and read its size and pixel format from sysfs.
fn probe_screen() -> Res<(Device, usize, usize)> {
    let sys = |name: &str| fs::read_to_string(format!("{SYSFS}/{name}"));
    let size = sys("virtual_size")?;
    let (w, h) = size.trim().split_once(',').ok_or("bad virtual_size")?;
    let (width, height): (usize, usize) = (w.parse()?, h.parse()?);
    let bytes_per_pixel = sys("bits_per_pixel")?.trim().parse::<usize>()? / 8;
    let stride: usize = sys("stride")?.trim().parse()?;
    if bytes_per_pixel != 2 && bytes_per_pixel != 4 {
        return Err("framebuffer must be 16 or 32 bits per pixel".into());
    }
    let device = Device {
        file: OpenOptions::new().write(true).open(DEVICE)?,
        is_screen: true,
        bytes_per_pixel,
        stride,
        buf: vec![0; stride * height],
    };
    Ok((device, width, height))
}

/// A plain file standing in for the screen: 32-bit BGRA, no row padding.
fn probe_file(path: &Path, (width, height): (usize, usize)) -> Res<(Device, usize, usize)> {
    let stride = width * 4;
    let device = Device {
        file: OpenOptions::new().write(true).create(true).open(path)?,
        is_screen: false,
        bytes_per_pixel: 4,
        stride,
        buf: vec![0; stride * height],
    };
    Ok((device, width, height))
}
