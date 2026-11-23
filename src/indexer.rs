//! Scans the USB stick once and builds the list of photos.
//! Only the small EXIF header of each file is read, so this is quick.

use exif::{In, Tag, Value};
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::Path;

use crate::common::Photo;

pub const IMAGES: &str = "photos";
pub const FAVORITES: &str = "favorites";

/// Every JPEG in Images/ and Favorites/, in a fresh random order.
pub fn scan(root: &Path) -> Vec<Photo> {
    let mut photos = Vec::new();
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
            println!("[Scanner] Reading file {:?}", name);
            let path = entry.path();
            let (date, orientation) = read_exif(&path);
            photos.push(Photo {
                path,
                date,
                orientation,
                favorite,
            });
        }
    }
    shuffle(&mut photos);
    photos
}

fn read_exif(path: &Path) -> (Option<(u16, u8, u8)>, u32) {
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

fn shuffle(photos: &mut [Photo]) {
    let mut seed = [1u8; 8];
    if let Ok(mut f) = File::open("/dev/urandom") {
        let _ = f.read_exact(&mut seed);
    }
    let mut state = u64::from_le_bytes(seed) | 1;
    for i in (1..photos.len()).rev() {
        // xorshift64
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        photos.swap(i, (state % (i as u64 + 1)) as usize);
    }
}
