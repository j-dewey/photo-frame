//! Settings from photos.conf.
//!
//! The file is read from the path given as the program's first argument, or
//! /etc/photos.conf. Every setting is optional; a missing file means all
//! defaults. The format is one `key = value` per line, with `#` comments:
//!
//! ```text
//! photos_root = /mnt/photos      # folder holding photos/ and favorites/
//! fb_file = /dev/shm/fb.raw      # where frames go while /dev/fb0 is missing
//! fb_size = 1920x1080            # size of the frames written to fb_file
//! ```

use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_PATH: &str = "/etc/photos.conf";

pub struct Config {
    /// Where the USB stick is mounted.
    pub photos_root: PathBuf,
    /// While there is no screen, write frames here instead (raw 32-bit BGRA).
    /// None: draw nothing until a screen appears.
    pub fb_file: Option<PathBuf>,
    /// Size of the frames written to `fb_file`, and of the drawing while
    /// headless.
    pub fb_size: (usize, usize),
}

impl Default for Config {
    fn default() -> Self {
        Config {
            photos_root: PathBuf::from("/mnt/photos"),
            fb_file: None,
            fb_size: (1920, 1080),
        }
    }
}

impl Config {
    /// Read the config file at `path`. A missing file gives the defaults;
    /// a bad line is reported and skipped, so a typo never stops the frame.
    pub fn load(path: &Path) -> Config {
        match fs::read_to_string(path) {
            Ok(text) => {
                println!("[Config] Reading {}", path.display());
                Config::parse(&text)
            }
            Err(e) => {
                println!("[Config] Can't read {} ({e}); using defaults", path.display());
                Config::default()
            }
        }
    }

    fn parse(text: &str) -> Config {
        let mut config = Config::default();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Err(e) = config.set(line) {
                println!("[Config] Line {}: {e}; ignoring it", n + 1);
            }
        }
        config
    }

    fn set(&mut self, line: &str) -> Result<(), String> {
        let (key, value) = line
            .split_once('=')
            .ok_or("expected `key = value`")?;
        let (key, value) = (key.trim(), value.trim());
        if value.is_empty() {
            return Err(format!("no value for `{key}`"));
        }
        match key {
            "photos_root" => self.photos_root = PathBuf::from(value),
            "fb_file" => self.fb_file = Some(PathBuf::from(value)),
            "fb_size" => {
                let size = value
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)))
                    .filter(|&(w, h)| w > 0 && h > 0)
                    .ok_or_else(|| format!("fb_size must be WIDTHxHEIGHT, not `{value}`"))?;
                self.fb_size = size;
            }
            _ => return Err(format!("unknown setting `{key}`")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        let c = Config::parse("# nothing here\n\n");
        assert_eq!(c.photos_root, PathBuf::from("/mnt/photos"));
        assert_eq!(c.fb_file, None);
        assert_eq!(c.fb_size, (1920, 1080));
    }

    #[test]
    fn reads_every_setting() {
        let c = Config::parse(
            "photos_root = /media/stick   # comment\n\
             fb_file=/dev/shm/fb.raw\n\
             fb_size = 800x480\n",
        );
        assert_eq!(c.photos_root, PathBuf::from("/media/stick"));
        assert_eq!(c.fb_file, Some(PathBuf::from("/dev/shm/fb.raw")));
        assert_eq!(c.fb_size, (800, 480));
    }

    #[test]
    fn bad_lines_are_skipped() {
        let c = Config::parse(
            "fb_size = big\n\
             colour = blue\n\
             just some words\n\
             fb_file =\n\
             photos_root = /ok\n",
        );
        assert_eq!(c.fb_size, (1920, 1080));
        assert_eq!(c.fb_file, None);
        assert_eq!(c.photos_root, PathBuf::from("/ok"));
    }

    #[test]
    fn missing_file_gives_defaults() {
        let c = Config::load(Path::new("/no/such/photos.conf"));
        assert_eq!(c.photos_root, PathBuf::from("/mnt/photos"));
    }
}
