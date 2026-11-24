//! Shows one photo at a time and reacts to the two buttons.

use crate::common::{Button, Buttons, FSMap, Mode, Photo, MONTHS};
use crate::fb::Fb;
use crate::indexer::read_exif;
use crate::Res;
use jpeg_decoder::{Decoder, PixelFormat};
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long each photo stays up. Longer = less work = less power.
const SLIDE: Duration = Duration::from_secs(30);
/// Ignore button presses closer together than this (switch bounce).
const DEBOUNCE: Duration = Duration::from_millis(250);

struct Presenter {
    fb: Fb,
    /// Shared with the indexer, which may still be adding photos.
    map: Arc<FSMap>,
    /// Index into `map` of the slide on screen (or the last one shown).
    pos: usize,
    /// The photo on screen, or None when the current mode has nothing to show.
    current: Option<Photo>,
    mode: Mode,
    /// The current photo, scaled to the screen, without any text on it.
    canvas: Vec<u8>,
}

pub fn run(fb: Fb, map: Arc<FSMap>, buttons: Arc<Buttons>) -> Res<()> {
    let canvas = vec![0; fb.width * fb.height * 3];
    let mut p = Presenter {
        fb,
        map,
        pos: 0,
        current: None,
        mode: Mode::All,
        canvas,
    };
    p.draw(Some("Loading photos..."))?;

    // Start as soon as the indexer has found one photo (or found none).
    p.map.wait_for_photos();
    buttons.clear(); // presses during loading are ignored
    p.advance(false)?;

    let mut last_press = Instant::now();
    loop {
        // Sleeps here until the slide is over or a button is pressed.
        match buttons.wait(SLIDE) {
            None => p.advance(false)?,
            Some(_) if last_press.elapsed() < DEBOUNCE => {}
            Some(Button::Favorite) => {
                last_press = Instant::now();
                p.toggle_favorite()?;
            }
            Some(Button::Mode) => {
                last_press = Instant::now();
                p.mode = p.mode.next();
                p.advance(true)?;
            }
        }
    }
}

impl Presenter {
    /// Move to the next photo that belongs to the current mode, skipping
    /// (and logging) any that can't be read.
    fn advance(&mut self, announce_mode: bool) -> Res<()> {
        let mut next = self.map.get_next_index(self.pos, self.mode);
        for _ in 0..self.map.count(self.mode) {
            let Some(i) = next else { break };
            self.pos = i;
            let photo = self.load(i);
            match render(&photo, self.fb.width, self.fb.height) {
                Ok(canvas) => {
                    self.canvas = canvas;
                    self.current = Some(photo);
                    return self.draw(announce_mode.then_some(self.mode.name()));
                }
                Err(e) => eprintln!("[Presenter] Skipping {:?}: {e}", photo.path),
            }
            next = self.map.get_next_index(i, self.mode);
        }

        // Nothing in this mode could be shown. Keep running: the next slide
        // or a mode change tries again.
        self.canvas.fill(0);
        self.current = None;
        let message = if !self.map.is_indexed() {
            format!("{}: loading photos...", self.mode.name())
        } else if self.map.is_empty() {
            "No photos found - check the USB stick".to_string()
        } else {
            format!("{}: no photos", self.mode.name())
        };
        self.draw(Some(&message))
    }

    /// Everything needed to show photo `i`.
    fn load(&self, i: usize) -> Photo {
        let path = self.map.get_photo(i).unwrap_or_default();
        let (date, orientation) = read_exif(&path);
        Photo {
            path,
            group: self.map.get_group(i),
            date,
            orientation,
            favorite: self.map.is_favorite(i),
        }
    }

    /// Move the current photo between Images/ and Favorites/ on the stick.
    fn toggle_favorite(&mut self) -> Res<()> {
        if self.current.is_none() {
            return Ok(());
        }
        let favorite = self.map.toggle_favorite(self.pos);
        if let (Some(photo), Some(path)) = (&mut self.current, self.map.get_photo(self.pos)) {
            photo.favorite = favorite;
            photo.path = path;
        }
        self.draw(None)
    }

    /// Re-render the current photo (or a blank screen) at the screen's size.
    fn resize_canvas(&mut self) {
        let (w, h) = (self.fb.width, self.fb.height);
        self.canvas = match &self.current {
            Some(photo) => render(photo, w, h).unwrap_or_else(|e| {
                eprintln!("[Presenter] Can't redraw {:?}: {e}", photo.path);
                vec![0; w * h * 3]
            }),
            None => vec![0; w * h * 3],
        };
    }

    /// Photo + caption (bottom left: group name over the date) + optional
    /// message (top left).
    fn draw(&mut self, message: Option<&str>) -> Res<()> {
        if self.fb.reconnect() {
            // A screen appeared at a different size: redo the photo for it.
            self.resize_canvas();
        }
        let (w, h) = (self.fb.width, self.fb.height);
        let scale = (h / 200).max(2);
        let margin = scale * 4;
        let mut frame = self.canvas.clone();

        if let Some(photo) = &self.current {
            let mut caption = match photo.date {
                Some((y, m, d)) => format!("{} {}, {}", MONTHS[m as usize - 1], d, y),
                None => String::new(),
            };
            if photo.favorite {
                caption += if caption.is_empty() { "*" } else { " *" };
            }
            // Bottom line first, then the group name above it.
            let line_h = scale * 12;
            let mut y = h.saturating_sub(margin + line_h);
            for line in [Some(caption.as_str()), photo.group.as_deref()].into_iter().flatten() {
                if !line.is_empty() {
                    text(&mut frame, w, h, line, margin, y, scale);
                    y = y.saturating_sub(line_h + scale * 2);
                }
            }
        }
        if let Some(message) = message {
            text(&mut frame, w, h, message, margin, margin, scale);
        }
        self.fb.show(&frame)
    }
}

/// Photos whose shape is close to the screen's are zoomed to fill it,
/// losing at most this share of the picture off two opposite edges. Others
/// are shown whole, over a blurred copy of themselves instead of black bars.
const MAX_CROP: f32 = 0.15;
/// The blurred background is built this many times smaller than the screen,
/// which makes blurring it nearly free.
const BG_SHRINK: usize = 16;
/// Brightness of the blurred background (1.0 = as bright as the photo).
const BG_DIM: f32 = 0.55;

/// Decode a JPEG and fit it to a w*h RGB canvas, upright and centred.
fn render(photo: &Photo, w: usize, h: usize) -> Res<Vec<u8>> {
    let mut decoder = Decoder::new(BufReader::new(File::open(&photo.path)?));
    decoder.read_info()?;
    let info = decoder.info().ok_or("no image info")?;
    let o = photo.orientation;
    let sideways = (5..=8).contains(&o);

    // Size of the photo once turned upright.
    let (iw, ih) = match sideways {
        true => (info.height as f32, info.width as f32),
        false => (info.width as f32, info.height as f32),
    };
    // Shown whole ("fit"), or zoomed to cover the screen ("fill").
    let fit = f32::min(w as f32 / iw, h as f32 / ih);
    let cover = f32::max(w as f32 / iw, h as f32 / ih);
    let fill = fit / cover >= 1.0 - MAX_CROP;
    let s = if fill { cover } else { fit };
    let (ow, oh) = ((iw * s).max(1.0), (ih * s).max(1.0));

    // Let the decoder shrink the photo as it decodes (by 2, 4 or 8). This is
    // the big saving: a 12-megapixel photo never gets fully decoded.
    let (want_w, want_h) = if sideways { (oh, ow) } else { (ow, oh) };
    let (sw, sh) = decoder.scale(want_w.ceil().min(65535.0) as u16, want_h.ceil().min(65535.0) as u16)?;
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

    let mut canvas = vec![0u8; w * h * 3];
    if !fill {
        // Background: a tiny copy that covers the screen, blurred, dimmed,
        // then stretched up to full size.
        let (bw, bh) = ((w / BG_SHRINK).max(1), (h / BG_SHRINK).max(1));
        let bs = f32::max(bw as f32 / vw as f32, bh as f32 / vh as f32);
        let mut small = vec![0u8; bw * bh * 3];
        place(&mut small, bw, bh, &at, vw, vh, vw as f32 * bs, vh as f32 * bs);
        for _ in 0..3 {
            blur(&mut small, bw, bh, 2);
        }
        small.iter_mut().for_each(|v| *v = (*v as f32 * BG_DIM) as u8);
        let small_at = |x: usize, y: usize, c: usize| small[(y * bw + x) * 3 + c] as f32;
        place(&mut canvas, w, h, small_at, bw, bh, w as f32, h as f32);
    }
    place(&mut canvas, w, h, &at, vw, vh, ow, oh);
    Ok(canvas)
}

/// Bilinear-resample a vw*vh image (read through `src`) to ow*oh, centred on
/// a w*h RGB canvas. Parts that fall off the canvas are cropped.
#[allow(clippy::too_many_arguments)]
fn place(
    dst: &mut [u8],
    w: usize,
    h: usize,
    src: impl Fn(usize, usize, usize) -> f32,
    vw: usize,
    vh: usize,
    ow: f32,
    oh: f32,
) {
    let (left, top) = ((w as f32 - ow) / 2.0, (h as f32 - oh) / 2.0);
    let (kx, ky) = (vw as f32 / ow, vh as f32 / oh);
    let span = |start: f32, len: f32, max: usize| {
        (start.max(0.0).round() as usize)..((start + len).round().clamp(0.0, max as f32) as usize)
    };
    let xs = span(left, ow, w);
    for y in span(top, oh, h) {
        let fy = ((y as f32 + 0.5 - top) * ky - 0.5).clamp(0.0, (vh - 1) as f32);
        let (y0, ty) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(vh - 1);
        for x in xs.clone() {
            let fx = ((x as f32 + 0.5 - left) * kx - 0.5).clamp(0.0, (vw - 1) as f32);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(vw - 1);
            for c in 0..3 {
                let above = src(x0, y0, c) * (1.0 - tx) + src(x1, y0, c) * tx;
                let below = src(x0, y1, c) * (1.0 - tx) + src(x1, y1, c) * tx;
                let value = above * (1.0 - ty) + below * ty;
                dst[(y * w + x) * 3 + c] = (value + 0.5) as u8;
            }
        }
    }
}

/// One box-blur pass of radius r over a w*h RGB image (edges repeat).
fn blur(img: &mut [u8], w: usize, h: usize, r: usize) {
    let n = (2 * r + 1) as u32;
    let mut tmp = vec![0u8; img.len()];
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let sum: u32 = (0..n as usize)
                    .map(|i| img[(y * w + (x + i).saturating_sub(r).min(w - 1)) * 3 + c] as u32)
                    .sum();
                tmp[(y * w + x) * 3 + c] = (sum / n) as u8;
            }
        }
    }
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let sum: u32 = (0..n as usize)
                    .map(|i| tmp[((y + i).saturating_sub(r).min(h - 1) * w + x) * 3 + c] as u32)
                    .sum();
                img[(y * w + x) * 3 + c] = (sum / n) as u8;
            }
        }
    }
}

/// Join a letter and a separate accent mark after it (macOS writes "é" in
/// file names as "e" + "´") into the single letter the font has.
fn compose(s: &str) -> impl Iterator<Item = char> + '_ {
    // For each mark: pairs of (plain letter, accented letter).
    const TABLE: [(char, &str); 7] = [
        ('\u{300}', "AÀEÈIÌOÒUÙaàeèiìoòuù"),
        ('\u{301}', "AÁEÉIÍOÓUÚYÝaáeéiíoóuúyý"),
        ('\u{302}', "AÂEÊIÎOÔUÛaâeêiîoôuû"),
        ('\u{303}', "AÃNÑOÕaãnñoõ"),
        ('\u{308}', "AÄEËIÏOÖUÜaäeëiïoöuüyÿ"),
        ('\u{30A}', "AÅaå"),
        ('\u{327}', "CÇcç"),
    ];
    let mut chars = s.chars().peekable();
    std::iter::from_fn(move || {
        let ch = chars.next()?;
        let Some(&mark) = chars.peek() else {
            return Some(ch); // last character
        };
        let Some((_, pairs)) = TABLE.iter().find(|(m, _)| *m == mark) else {
            return Some(ch);
        };
        let pairs: Vec<char> = pairs.chars().collect();
        match pairs.chunks(2).find(|p| p[0] == ch) {
            Some(p) => {
                chars.next();
                Some(p[1])
            }
            None => Some(ch),
        }
    })
}

/// The 8x8 bitmap for `ch`. Covers English and Western European letters
/// (é, ñ, ü...). Accent marks the font can't join on are dropped; anything
/// else shows as '?'.
fn glyph(ch: char) -> Option<[u8; 8]> {
    use font8x8::legacy::{BASIC_LEGACY, LATIN_LEGACY};
    match ch as u32 {
        0x0300..=0x036F => None,
        c @ 0x20..=0x7E => Some(BASIC_LEGACY[c as usize]),
        c @ 0xA0..=0xFF => Some(LATIN_LEGACY[c as usize - 0xA0]),
        _ => Some(BASIC_LEGACY[b'?' as usize]),
    }
}

/// White text on a darkened box, so it is readable over any photo.
fn text(frame: &mut [u8], w: usize, h: usize, s: &str, x: usize, y: usize, scale: usize) {
    if s.is_empty() {
        return;
    }
    let glyphs: Vec<[u8; 8]> = compose(s).filter_map(glyph).collect();
    let pad = scale * 2;
    let (box_w, box_h) = (glyphs.len() * 8 * scale + pad * 2, 8 * scale + pad * 2);
    for py in y..(y + box_h).min(h) {
        for px in x..(x + box_w).min(w) {
            for c in 0..3 {
                frame[(py * w + px) * 3 + c] /= 3;
            }
        }
    }
    for (i, glyph) in glyphs.iter().enumerate() {
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

#[cfg(test)]
mod tests {
    use super::compose;

    #[test]
    fn joins_separate_accents() {
        let c = |s: &str| compose(s).collect::<String>();
        assert_eq!(c("Cafe\u{301} Trip"), "Café Trip");
        assert_eq!(c("Cafe\u{301}"), "Café");
        assert_eq!(c("Kamila"), "Kamila");
        assert_eq!(c("x\u{301}"), "x\u{301}"); // no such letter: left alone
    }
}
