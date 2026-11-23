//! Shows one photo at a time and reacts to the two buttons.

use crate::common::{Mode, Photo, MONTHS};
use crate::fb::Fb;
use crate::indexer::{FAVORITES, IMAGES};
use crate::{Event, Res};
use jpeg_decoder::{Decoder, PixelFormat};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// How long each photo stays up. Longer = less work = less power.
const SLIDE: Duration = Duration::from_secs(30);
/// Ignore button presses closer together than this (switch bounce).
const DEBOUNCE: Duration = Duration::from_millis(250);

struct Presenter<'a> {
    fb: Fb,
    root: &'a Path,
    photos: Vec<Photo>,
    /// Index into `photos` of the slide on screen (or the last one shown).
    pos: usize,
    /// False when the current mode has nothing to show.
    showing: bool,
    mode: Mode,
    /// The current photo, scaled to the screen, without any text on it.
    canvas: Vec<u8>,
}

pub fn run(fb: Fb, events: Receiver<Event>, root: &Path) -> Res<()> {
    let canvas = vec![0; fb.width * fb.height * 3];
    let mut p = Presenter {
        fb,
        root,
        photos: Vec::new(),
        pos: 0,
        showing: false,
        mode: Mode::All,
        canvas,
    };
    p.draw(Some("Loading photos..."))?;

    let mut indexed = false;
    let mut last_press = Instant::now();
    loop {
        // Sleeps here until the slide is over or something happens.
        match events.recv_timeout(SLIDE) {
            Ok(Event::Index(photos)) => {
                p.photos = photos;
                indexed = true;
                p.advance(false)?;
            }
            Ok(_) if !indexed || last_press.elapsed() < DEBOUNCE => {}
            Ok(Event::ToggleFavorite) => {
                last_press = Instant::now();
                p.toggle_favorite()?;
            }
            Ok(Event::NextMode) => {
                last_press = Instant::now();
                p.mode = p.mode.next();
                p.advance(true)?;
            }
            Err(RecvTimeoutError::Timeout) if indexed => p.advance(false)?,
            Err(_) => {}
        }
    }
}

impl Presenter<'_> {
    /// Move to the next photo that belongs to the current mode.
    fn advance(&mut self, announce_mode: bool) -> Res<()> {
        loop {
            let n = self.photos.len();
            let next = (1..=n)
                .map(|step| (self.pos + step) % n)
                .find(|&i| self.mode.includes(&self.photos[i]));
            let Some(i) = next else { break };
            self.pos = i;
            match render(&self.photos[i], self.fb.width, self.fb.height) {
                Ok(canvas) => {
                    self.canvas = canvas;
                    self.showing = true;
                    return self.draw(announce_mode.then_some(self.mode.name()));
                }
                // Unreadable file (or the stick was pulled out): forget it.
                Err(_) => {
                    self.photos.swap_remove(i);
                }
            }
        }

        self.canvas.fill(0);
        self.showing = false;
        if self.photos.is_empty() {
            // Nothing usable on the stick. Say so, then quit: systemd
            // restarts us, which rescans, so a new stick gets picked up.
            self.draw(Some("No photos found - check the USB stick"))?;
            sleep(Duration::from_secs(10));
            return Err("no photos".into());
        }
        self.draw(Some(&format!("{}: no photos", self.mode.name())))
    }

    /// Move the current photo between Images/ and Favorites/ on the stick.
    fn toggle_favorite(&mut self) -> Res<()> {
        if !self.showing {
            return Ok(());
        }
        let photo = &mut self.photos[self.pos];
        let folder = self
            .root
            .join(if photo.favorite { IMAGES } else { FAVORITES });
        let Some(name) = photo.path.file_name() else {
            return Ok(());
        };
        let dest = folder.join(name);
        // Never overwrite a different photo that happens to share the name.
        if dest.exists() {
            return Ok(());
        }
        let _ = fs::create_dir_all(&folder);
        if fs::rename(&photo.path, &dest).is_ok() {
            photo.path = dest;
            photo.favorite = !photo.favorite;
            // Flush to the stick now so pulling the plug can't lose the move.
            let _ = Command::new("sync").status();
        }
        self.draw(None)
    }

    /// Photo + date caption (bottom left) + optional message (top left).
    fn draw(&mut self, message: Option<&str>) -> Res<()> {
        let (w, h) = (self.fb.width, self.fb.height);
        let scale = (h / 200).max(2);
        let margin = scale * 4;
        let mut frame = self.canvas.clone();

        if self.showing {
            let photo = &self.photos[self.pos];
            let mut caption = match photo.date {
                Some((y, m, d)) => format!("{} {}, {}", MONTHS[m as usize - 1], d, y),
                None => String::new(),
            };
            if photo.favorite {
                caption += if caption.is_empty() { "*" } else { " *" };
            }
            let y = h.saturating_sub(margin + scale * 12);
            text(&mut frame, w, h, &caption, margin, y, scale);
        }
        if let Some(message) = message {
            text(&mut frame, w, h, message, margin, margin, scale);
        }
        self.fb.show(&frame)
    }
}

/// Decode a JPEG and fit it to a w*h RGB canvas: upright, centred, black bars.
fn render(photo: &Photo, w: usize, h: usize) -> Res<Vec<u8>> {
    let mut decoder = Decoder::new(BufReader::new(File::open(&photo.path)?));
    decoder.read_info()?;
    let info = decoder.info().ok_or("no image info")?;
    let o = photo.orientation;
    let sideways = (5..=8).contains(&o);

    // Size of the photo on screen once turned upright and fitted.
    let (iw, ih) = match sideways {
        true => (info.height as f32, info.width as f32),
        false => (info.width as f32, info.height as f32),
    };
    let fit = f32::min(w as f32 / iw, h as f32 / ih);
    let ow = ((iw * fit) as usize).clamp(1, w);
    let oh = ((ih * fit) as usize).clamp(1, h);

    // Let the decoder shrink the photo as it decodes (by 2, 4 or 8). This is
    // the big saving: a 12-megapixel photo never gets fully decoded.
    let (want_w, want_h) = if sideways { (oh, ow) } else { (ow, oh) };
    let (sw, sh) = decoder.scale(want_w as u16, want_h as u16)?;
    let (sw, sh) = (sw as usize, sh as usize);
    let pixels = decoder.decode()?;
    let channels = match info.pixel_format {
        PixelFormat::RGB24 => 3,
        PixelFormat::L8 => 1,
        _ => return Err("unsupported JPEG colour format".into()),
    };

    // (x, y) in the upright view -> pixel in the decoded data.
    let (vw, vh) = if sideways { (sh, sw) } else { (sw, sh) };
    let at = |x: usize, y: usize, c: usize| -> f32 {
        let (sx, sy) = match o {
            2 => (sw - 1 - x, y),
            3 => (sw - 1 - x, sh - 1 - y),
            4 => (x, sh - 1 - y),
            5 => (y, x),
            6 => (y, sh - 1 - x),
            7 => (sw - 1 - y, sh - 1 - x),
            8 => (sw - 1 - y, x),
            _ => (x, y),
        };
        pixels[(sy * sw + sx) * channels + c.min(channels - 1)] as f32
    };

    // Bilinear resample into the centre of a black canvas.
    let mut canvas = vec![0u8; w * h * 3];
    let (left, top) = ((w - ow) / 2, (h - oh) / 2);
    for y in 0..oh {
        let fy = ((y as f32 + 0.5) * vh as f32 / oh as f32 - 0.5).clamp(0.0, (vh - 1) as f32);
        let (y0, ty) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(vh - 1);
        for x in 0..ow {
            let fx = ((x as f32 + 0.5) * vw as f32 / ow as f32 - 0.5).clamp(0.0, (vw - 1) as f32);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(vw - 1);
            for c in 0..3 {
                let above = at(x0, y0, c) * (1.0 - tx) + at(x1, y0, c) * tx;
                let below = at(x0, y1, c) * (1.0 - tx) + at(x1, y1, c) * tx;
                let value = above * (1.0 - ty) + below * ty;
                canvas[((top + y) * w + left + x) * 3 + c] = (value + 0.5) as u8;
            }
        }
    }
    Ok(canvas)
}

/// White text on a darkened box, so it is readable over any photo.
fn text(frame: &mut [u8], w: usize, h: usize, s: &str, x: usize, y: usize, scale: usize) {
    if s.is_empty() {
        return;
    }
    let pad = scale * 2;
    let (box_w, box_h) = (s.len() * 8 * scale + pad * 2, 8 * scale + pad * 2);
    for py in y..(y + box_h).min(h) {
        for px in x..(x + box_w).min(w) {
            for c in 0..3 {
                frame[(py * w + px) * 3 + c] /= 3;
            }
        }
    }
    for (i, ch) in s.bytes().enumerate() {
        let glyph = font8x8::legacy::BASIC_LEGACY[(ch & 127) as usize];
        for (row, bits) in glyph.iter().enumerate() {
            for col in (0..8).filter(|col| bits >> col & 1 == 1) {
                let (gx, gy) = (x + pad + (i * 8 + col) * scale, y + pad + row * scale);
                for py in gy..(gy + scale).min(h) {
                    for px in gx..(gx + scale).min(w) {
                        frame[(py * w + px) * 3..][..3].fill(255);
                    }
                }
            }
        }
    }
}
