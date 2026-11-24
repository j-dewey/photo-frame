//! Scans the USB stick once, adding each photo to the shared FSMap.
//! Only the headers of each file are read, so this is quick.

use exif::{In, Tag, Value};
use jpeg_decoder::{Decoder, PixelFormat};
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::Path;

use crate::common::{FSMap, Photo};

/// Folder on the stick holding the photos. Each folder inside it is a group.
pub const IMAGES: &str = "photos";
/// List of favorite photos on the stick, one per line, relative to IMAGES.
pub const FAVORITES_FILE: &str = "favorites.txt";
/// Where older versions kept favorites (by moving them here). Photos found
/// here are moved back into IMAGES and added to FAVORITES_FILE.
const OLD_FAVORITES: &str = "favorites";

/// Add every readable JPEG in IMAGES, and in each group folder inside it, to
/// `map`, in a fresh random order. Photos go in one at a time, so the
/// presenter can start showing them before the scan is finished.
pub fn scan(root: &Path, map: &FSMap) {
    adopt_old_favorites(root, map);

    // Listing the folders is quick; reading each file is the slow part. So
    // list and shuffle first, then read and add the files in that order.
    let images = root.join(IMAGES);
    println!("[Scanner] Scanning dir {:?}", images);
    let mut files = Vec::new();
    for entry in visible(&images) {
        let path = entry.path();
        if !path.is_dir() {
            if is_jpeg(&path) {
                files.push((path, None));
            }
            continue;
        }
        // A group: photos directly inside it count, deeper folders don't.
        let group = entry.file_name().to_string_lossy().into_owned();
        println!("[Scanner] Scanning group {:?}", group);
        for inner in visible(&path) {
            let path = inner.path();
            if path.is_dir() {
                println!("[Scanner] Skipping {:?}: groups can't hold folders", path);
            } else if is_jpeg(&path) {
                files.push((path, Some(group.clone())));
            }
        }
    }
    shuffle(&mut files);
    map.reserve(files.len());

    let mut count = 0;
    for (path, group) in files {
        if !is_readable(&path) {
            println!("[Scanner] Skipping unreadable photo {:?}", path);
            continue;
        }
        println!("[Scanner] Reading file {:?}", path);
        let (date, orientation) = read_exif(&path);
        let favorite = map.is_listed_favorite(&path);
        map.add_photo(&Photo {
            path,
            group,
            date,
            orientation,
            favorite,
        });
        count += 1;
    }

    println!("[Scanner] Found {} photos", count);
}

/// Entries of `dir`, skipping hidden ones (macOS writes "._photo.jpg" junk
/// and ".Trashes" folders).
fn visible(dir: &Path) -> impl Iterator<Item = fs::DirEntry> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
}

fn is_jpeg(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
    let ok = name.ends_with(".jpg") || name.ends_with(".jpeg");
    if !ok {
        println!("[Scanner] Skipping file {:?}", path);
    }
    ok
}

/// Move photos from the old favorites/ folder into IMAGES and list them as
/// favorites, so a stick set up by an older version keeps its favorites.
fn adopt_old_favorites(root: &Path, map: &FSMap) {
    let (old, images) = (root.join(OLD_FAVORITES), root.join(IMAGES));
    let mut moved = Vec::new();
    for entry in visible(&old) {
        let (from, to) = (entry.path(), images.join(entry.file_name()));
        if !is_jpeg(&from) {
            continue;
        }
        if to.exists() {
            println!("[Scanner] Can't move old favorite {:?}: {:?} exists", from, to);
            continue;
        }
        let _ = fs::create_dir_all(&images);
        match fs::rename(&from, &to) {
            Ok(()) => moved.push(to),
            Err(e) => println!("[Scanner] Can't move old favorite {:?}: {e}", from),
        }
    }
    if !moved.is_empty() {
        println!("[Scanner] Moved {} old favorites into {:?}", moved.len(), images);
        map.add_listed_favorites(&moved);
    }
    // Only goes if empty, so nothing is ever lost.
    let _ = fs::remove_dir(&old);
}

/// Whether the presenter can decode this JPEG: its header parses and it is
/// in a colour format we can show (not e.g. CMYK).
fn is_readable(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
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
