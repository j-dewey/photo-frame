use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Condvar, Mutex, MutexGuard},
    time::Duration,
};

use crate::indexer;

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
    /// USB root that holds the IMAGES and FAVORITES folders.
    root: PathBuf,
    photos: Vec<PathBuf>,
    // These map back into self.photos and are kept sorted, so "next" in any
    // mode follows the same (shuffled) order as All Photos.
    spring: Vec<usize>,
    summer: Vec<usize>,
    autumn: Vec<usize>,
    winter: Vec<usize>,
    favorites: Vec<usize>,
    /// Set once the indexer has gone through the whole stick.
    indexed: bool,
}

impl FSMapInner {
    fn push_photo(&mut self, photo: &Photo) -> usize {
        self.photos.push(photo.path.clone());
        let new_index = self.photos.len() - 1;

        // Update Season Buffers
        match Mode::get_month(photo) {
            Mode::Spring => self.spring.push(new_index),
            Mode::Summer => self.summer.push(new_index),
            Mode::Autumn => self.autumn.push(new_index),
            Mode::Winter => self.winter.push(new_index),
            _ => { /* No date attached to image */ }
        }
        if photo.favorite {
            self.favorites.push(new_index);
        }

        new_index
    }

    /// The indices into `photos` that belong to `mode`, or None for All.
    fn list(&self, mode: Mode) -> Option<&[usize]> {
        match mode {
            Mode::All => None,
            Mode::Spring => Some(&self.spring),
            Mode::Summer => Some(&self.summer),
            Mode::Autumn => Some(&self.autumn),
            Mode::Winter => Some(&self.winter),
            Mode::Favorites => Some(&self.favorites),
        }
    }

    fn next_index(&self, cur: usize, mode: Mode) -> Option<usize> {
        match self.list(mode) {
            None if self.photos.is_empty() => None,
            None => Some((cur + 1) % self.photos.len()),
            Some([]) => None,
            Some(list) => {
                // First entry after `cur`, wrapping around to the start.
                let after = list.partition_point(|&i| i <= cur);
                Some(*list.get(after).unwrap_or(&list[0]))
            }
        }
    }
}

/// A map of the file system into the appropriate categories.
///
/// Shared (in an `Arc`) by the indexer, which fills it in while it scans the
/// stick, and the presenter, which shows photos from it. Photos are only ever
/// appended, so an index into the map stays valid for the life of the program.
pub struct FSMap {
    inner: Mutex<FSMapInner>,
    /// Signalled whenever a photo is added or indexing finishes.
    changed: Condvar,
}

impl FSMap {
    pub fn new(root: &Path) -> Self {
        Self {
            inner: Mutex::new(FSMapInner {
                root: root.to_path_buf(),
                photos: Vec::new(),
                spring: Vec::new(),
                summer: Vec::new(),
                autumn: Vec::new(),
                winter: Vec::new(),
                favorites: Vec::new(),
                indexed: false,
            }),
            changed: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, FSMapInner> {
        // A panic while holding the lock can't leave the lists half-updated
        // in a harmful way, so just carry on with the data.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Make room for `len` more photos, once the indexer knows how many.
    pub fn reserve(&self, len: usize) {
        let mut guard = self.lock();
        let fs = &mut *guard;
        fs.photos.reserve(len);
        for list in [
            &mut fs.spring,
            &mut fs.summer,
            &mut fs.autumn,
            &mut fs.winter,
        ] {
            list.reserve(len / 4);
        }
        fs.favorites.reserve(len / 10);
    }

    /// Add a photo and wake anyone waiting in `wait_for_photos`.
    pub fn add_photo(&self, photo: &Photo) -> usize {
        let index = self.lock().push_photo(photo);
        self.changed.notify_all();
        index
    }

    /// Called by the indexer when it has gone through the whole stick.
    pub fn finish_indexing(&self) {
        self.lock().indexed = true;
        self.changed.notify_all();
    }

    pub fn is_indexed(&self) -> bool {
        self.lock().indexed
    }

    /// Block until there is at least one photo or indexing has finished.
    pub fn wait_for_photos(&self) {
        let fs = self.lock();
        let _fs = self
            .changed
            .wait_while(fs, |fs| fs.photos.is_empty() && !fs.indexed)
            .unwrap_or_else(|e| e.into_inner());
    }

    pub fn len(&self) -> usize {
        self.lock().photos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Path of photo `img`, if there is one.
    pub fn get_photo(&self, img: usize) -> Option<PathBuf> {
        self.lock().photos.get(img).cloned()
    }

    pub fn is_favorite(&self, img: usize) -> bool {
        self.lock().favorites.binary_search(&img).is_ok()
    }

    /// Number of photos that `mode` would show (so far).
    pub fn count(&self, mode: Mode) -> usize {
        let fs = self.lock();
        fs.list(mode).map_or(fs.photos.len(), <[usize]>::len)
    }

    /// Index of the photo after `cur` in `mode`, wrapping around, or None if
    /// the mode has no photos. `cur` doesn't have to be in the mode itself
    /// (e.g. right after the mode changes).
    pub fn get_next_index(&self, cur: usize, mode: Mode) -> Option<usize> {
        self.lock().next_index(cur, mode)
    }

    /// Path of the photo after `cur` in `mode`, wrapping around. Returns an
    /// empty PathBuf if the mode has no photos; use `get_next_index` to tell
    /// that apart and to learn the new index.
    #[allow(dead_code)]
    pub fn get_next(&self, cur: usize, mode: Mode) -> PathBuf {
        let fs = self.lock();
        fs.next_index(cur, mode)
            .map(|i| fs.photos[i].clone())
            .unwrap_or_default()
    }

    /// Move photo `img` between IMAGES and FAVORITES on the stick and update
    /// the favorites list. Returns whether the photo is a favorite afterwards,
    /// so if the move fails (bad index, name clash, stick pulled) it returns
    /// the unchanged state.
    pub fn toggle_favorite(&self, img: usize) -> bool {
        // Don't hold the lock while touching the stick, so the indexer can
        // keep adding photos meanwhile.
        let (path, was_favorite, root) = {
            let fs = self.lock();
            let Some(path) = fs.photos.get(img) else {
                return false;
            };
            let was_favorite = fs.favorites.binary_search(&img).is_ok();
            (path.clone(), was_favorite, fs.root.clone())
        };
        let Some(name) = path.file_name() else {
            return was_favorite;
        };
        let folder = root.join(if was_favorite {
            indexer::IMAGES
        } else {
            indexer::FAVORITES
        });
        let dest = folder.join(name);
        // Never overwrite a different photo that happens to share the name.
        if dest.exists() {
            return was_favorite;
        }
        let _ = fs::create_dir_all(&folder);
        if fs::rename(&path, &dest).is_err() {
            return was_favorite;
        }
        // Flush to the stick now so pulling the plug can't lose the move.
        let _ = Command::new("sync").status();

        let mut fs = self.lock();
        fs.photos[img] = dest;
        match fs.favorites.binary_search(&img) {
            Ok(at) => {
                fs.favorites.remove(at);
            }
            Err(at) => fs.favorites.insert(at, img),
        }
        !was_favorite
    }
}

#[derive(Clone, Copy)]
pub enum Button {
    Favorite,
    Mode,
}

/// The latest button press, shared by the button threads and the presenter.
#[derive(Default)]
pub struct Buttons {
    pressed: Mutex<Option<Button>>,
    changed: Condvar,
}

impl Buttons {
    fn lock(&self) -> MutexGuard<'_, Option<Button>> {
        self.pressed.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Record a press and wake the presenter. A press that hasn't been
    /// handled yet is replaced, since only the latest one matters.
    pub fn press(&self, button: Button) {
        *self.lock() = Some(button);
        self.changed.notify_all();
    }

    /// Sleep until a button is pressed or `timeout` passes. Returns the
    /// press, or None on timeout.
    pub fn wait(&self, timeout: Duration) -> Option<Button> {
        let pressed = self.lock();
        let (mut pressed, _) = self
            .changed
            .wait_timeout_while(pressed, timeout, |p| p.is_none())
            .unwrap_or_else(|e| e.into_inner());
        pressed.take()
    }

    /// Forget any press that hasn't been handled.
    pub fn clear(&self) {
        *self.lock() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, thread};

    fn photo(name: &str, month: Option<u8>, favorite: bool) -> Photo {
        Photo {
            path: PathBuf::from(name),
            date: month.map(|m| (2024, m, 1)),
            orientation: 1,
            favorite,
        }
    }

    fn map_of(root: &Path, photos: &[Photo]) -> FSMap {
        let map = FSMap::new(root);
        for p in photos {
            map.add_photo(p);
        }
        map.finish_indexing();
        map
    }

    fn sample(root: &Path) -> FSMap {
        map_of(
            root,
            &[
                photo("a.jpg", Some(4), false),  // 0 spring
                photo("b.jpg", Some(7), true),   // 1 summer, fav
                photo("c.jpg", Some(5), false),  // 2 spring
                photo("d.jpg", None, true),      // 3 undated, fav
                photo("e.jpg", Some(12), false), // 4 winter
            ],
        )
    }

    #[test]
    fn next_wraps_within_each_mode() {
        let map = sample(Path::new("/nowhere"));
        assert_eq!(map.get_next(0, Mode::All), PathBuf::from("b.jpg"));
        assert_eq!(map.get_next(4, Mode::All), PathBuf::from("a.jpg"));
        assert_eq!(map.get_next(0, Mode::Spring), PathBuf::from("c.jpg"));
        assert_eq!(map.get_next(2, Mode::Spring), PathBuf::from("a.jpg"));
        // `cur` outside the mode jumps to the next photo that is in it.
        assert_eq!(map.get_next(1, Mode::Spring), PathBuf::from("c.jpg"));
        assert_eq!(map.get_next(1, Mode::Favorites), PathBuf::from("d.jpg"));
        assert_eq!(map.get_next(3, Mode::Favorites), PathBuf::from("b.jpg"));
        assert_eq!(map.get_next(0, Mode::Winter), PathBuf::from("e.jpg"));
        assert_eq!(map.get_next(0, Mode::Autumn), PathBuf::new());
        assert_eq!(map.get_next_index(0, Mode::Autumn), None);
        assert_eq!(map.count(Mode::Spring), 2);
    }

    #[test]
    fn toggle_moves_file_and_updates_favorites() {
        let root = std::env::temp_dir().join(format!("fsmap-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(indexer::IMAGES)).unwrap();
        let file = root.join(indexer::IMAGES).join("x.jpg");
        fs::write(&file, b"not really a jpeg").unwrap();

        let mut p = photo("x.jpg", Some(6), false);
        p.path = file.clone();
        let map = map_of(&root, &[p]);

        assert!(map.toggle_favorite(0));
        let fav = root.join(indexer::FAVORITES).join("x.jpg");
        assert!(fav.exists() && !file.exists());
        assert_eq!(map.get_photo(0), Some(fav.clone()));
        assert_eq!(map.get_next(0, Mode::Favorites), fav);

        assert!(!map.toggle_favorite(0));
        assert!(file.exists() && !fav.exists());
        assert_eq!(map.get_next_index(0, Mode::Favorites), None);

        // Bad index: nothing happens.
        assert!(!map.toggle_favorite(7));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn presenter_wakes_on_first_photo_while_indexing() {
        let map = Arc::new(FSMap::new(Path::new("/nowhere")));
        let indexer = Arc::clone(&map);
        let t = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            indexer.add_photo(&photo("a.jpg", None, false));
            thread::sleep(Duration::from_millis(50));
            indexer.add_photo(&photo("b.jpg", None, false));
            indexer.finish_indexing();
        });
        map.wait_for_photos();
        assert!(!map.is_empty());
        t.join().unwrap();
        assert!(map.is_indexed());
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn wait_returns_once_indexing_finds_nothing() {
        let map = map_of(Path::new("/nowhere"), &[]);
        map.wait_for_photos();
        assert!(map.is_empty());
    }

    #[test]
    fn buttons_wake_waiter_or_time_out() {
        let buttons = Arc::new(Buttons::default());
        assert!(buttons.wait(Duration::from_millis(20)).is_none());
        let presser = Arc::clone(&buttons);
        let t = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            presser.press(Button::Mode);
        });
        assert!(matches!(
            buttons.wait(Duration::from_secs(5)),
            Some(Button::Mode)
        ));
        t.join().unwrap();
        buttons.press(Button::Favorite);
        buttons.clear();
        assert!(buttons.wait(Duration::from_millis(10)).is_none());
    }
}
