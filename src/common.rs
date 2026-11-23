use std::{
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

pub struct Photo {
    pub path: PathBuf,
    /// (year, month, day) the photo was taken, if the camera recorded it.
    pub date: Option<(u16, u8, u8)>,
    /// EXIF orientation, 1-8 (1 = already upright).
    pub orientation: u32,
    pub favorite: bool,
}

pub const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

#[derive(Clone, Copy)]
pub enum Mode {
    All,
    Spring,
    Summer,
    Autumn,
    Winter,
    Favorites,
}

impl Mode {
    pub fn next(self) -> Mode {
        match self {
            Mode::All => Mode::Spring,
            Mode::Spring => Mode::Summer,
            Mode::Summer => Mode::Autumn,
            Mode::Autumn => Mode::Winter,
            Mode::Winter => Mode::Favorites,
            Mode::Favorites => Mode::All,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::All => "All Photos",
            Mode::Spring => "Spring",
            Mode::Summer => "Summer",
            Mode::Autumn => "Autumn",
            Mode::Winter => "Winter",
            Mode::Favorites => "Favorites",
        }
    }

    /// Seasons go by month: Mar-May, Jun-Aug, Sep-Nov, Dec-Feb.
    /// Photos with no date only appear in All Photos and Favorites.
    pub fn includes(self, photo: &Photo) -> bool {
        let month = photo.date.map(|(_, month, _)| month);
        match self {
            Mode::All => true,
            Mode::Favorites => photo.favorite,
            Mode::Spring => matches!(month, Some(3..=5)),
            Mode::Summer => matches!(month, Some(6..=8)),
            Mode::Autumn => matches!(month, Some(9..=11)),
            Mode::Winter => matches!(month, Some(12 | 1 | 2)),
        }
    }

    /// Get the month a photo was taken
    /// If it doesn't have a month attached, return All
    pub fn get_month(photo: &Photo) -> Self {
        let month = photo.date.map(|(_, month, _)| month).unwrap_or(13);
        match month {
            3 | 4 | 5 => Mode::Spring,
            6 | 7 | 8 => Mode::Summer,
            9 | 10 | 11 => Mode::Autumn,
            12 | 1 | 2 => Mode::Winter,
            _ => Mode::All,
        }
    }
}

// A map of the file system into the appropriate categories
struct FSMapInner {
    photos: Vec<PathBuf>,
    // These map back into self.photos
    spring: Vec<usize>,
    summer: Vec<usize>,
    autumn: Vec<usize>,
    winter: Vec<usize>,
    favorites: Vec<usize>,
}

// A map of the file system into the appropriate categories
// Always behind a lock
struct FSMap {
    inner: Mutex<FSMapInner>,
}

impl FSMap {
    pub fn new(len: usize) -> Self {
        Self {
            inner: Mutex::new(FSMapInner {
                photos: Vec::with_capacity(len),
                spring: Vec::with_capacity(len / 4),
                summer: Vec::with_capacity(len / 4),
                autumn: Vec::with_capacity(len / 4),
                winter: Vec::with_capacity(len / 4),
                favorites: Vec::with_capacity(len / 10),
            }),
        }
    }

    fn push_photo(fs: &mut MutexGuard<'_, FSMapInner>, photo: &Photo) -> usize {
        fs.photos.push(photo.path.clone());
        let new_index = fs.photos.len() - 1;

        // Update Month Buffers
        match Mode::get_month(&photo) {
            Mode::Spring => fs.spring.push(new_index),
            Mode::Summer => fs.summer.push(new_index),
            Mode::Autumn => fs.autumn.push(new_index),
            Mode::Winter => fs.winter.push(new_index),
            _ => { /* No date attached to image */ }
        }

        return new_index;
    }

    pub fn add_photo(&mut self, photo: &Photo) {
        let mut fs = self
            .inner
            .lock()
            .expect("Failed to acquire lock for FSMapInner during add_photo");
        FSMap::push_photo(&mut fs, photo);
    }

    pub fn add_favorite_photo(&mut self, photo: &Photo) {
        let mut fs = self
            .inner
            .lock()
            .expect("Failed to acquire lock for FSMapInner during add_photo");

        let new_index = FSMap::push_photo(&mut fs, photo);
        fs.favorites.push(new_index);
    }
}
