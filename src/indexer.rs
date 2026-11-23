//! Scans the USB stick once, adding each photo to the shared FSMap.
//! Only the headers of each file are read, so this is quick.

use exif::{In, Tag, Value};
use jpeg_decoder::{Decoder, PixelFormat};
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::Path;

use crate::common::{FSMap, Photo};

pub const IMAGES: &str = "photos";
pub const FAVORITES: &str = "favorites";

/// Add every readable JPEG in Images/ and Favorites/ to `map`, in a fresh
/// random order. Photos go in one at a time, so the presenter can start
/// showing them before the scan is finished.
pub fn scan(root: &Path, map: &FSMap) {
    // Listing the folders is quick; reading each file is the slow part. So
    // list and shuffle first, then read and add the files in that order.
    let mut files = Vec::new();
    for (dir, favorite) in [(IMAGES, false), (FAVORITES, true)] {
        let cur_dir = root.join(dir);
        println!("[Scanner] Scanning dir {:?}", cur_dir);
        for entry in fs::read_dir(cur_dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            // Skip hidden files (macOS writes "._photo.jpg" junk) and non-JPEGs.
            if name.starts_with('.') || !(name.ends_with(".jpg") || name.ends_with(".jpeg")) {
                println!("[Scanner] Skipping file {:?}", name);
                continue;
            }
            files.push((entry.path(), favorite));
        }
    }
    shuffle(&mut files);
    map.reserve(files.len());

    for (path, favorite) in files {
        if !is_readable(&path) {
            println!("[Scanner] Skipping unreadable photo {:?}", path);
            continue;
        }
        println!("[Scanner] Reading file {:?}", path);
        let (date, orientation) = read_exif(&path);
        map.add_photo(&Photo {
            path,
            date,
            orientation,
            favorite,
        });
    }
}

/// Whether the presenter can decode this JPEG: its header parses and it is
/// in a colour format we can show (not e.g. CMYK).
fn is_readable(path: &Path) -> bool {
    let Ok(file) = File::open(path) else { return false };
    let mut decoder = Decoder::new(BufReader::new(file));
    decoder.read_info().is_ok()
        && matches!(
            decoder.info().map(|info| info.pixel_format),
            Some(PixelFormat::RGB24 | PixelFormat::L8)
        )
}

pub fn read_exif(path: &Path) -> (Option<(u16, u8, u8)>, u32) {
    let exif = File::open(path).ok().and_then(|f| {
        exif::Reader::new()
            .read_from_container(&mut BufReader::new(f))
            .ok()
    });
    let Some(exif) = exif else { return (None, 1) };

    let date = exif
        .get_field(Tag::DateTimeOriginal, In::PRIMARY)
        .and_then(|field| match &field.value {
            Value::Ascii(text) => exif::DateTime::from_ascii(text.first()?).ok(),
            _ => None,
        })
        .filter(|d| (1..=12).contains(&d.month))
        .map(|d| (d.year, d.month, d.day));
    let orientation = exif
        .get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|field| field.value.get_uint(0))
        .unwrap_or(1);
    (date, orientation)
}

fn shuffle<T>(items: &mut [T]) {
    let mut seed = [1u8; 8];
    if let Ok(mut f) = File::open("/dev/urandom") {
        let _ = f.read_exact(&mut seed);
    }
    let mut state = u64::from_le_bytes(seed) | 1;
    for i in (1..items.len()).rev() {
        // xorshift64
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        items.swap(i, (state % (i as u64 + 1)) as usize);
    }
}
