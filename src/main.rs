//! Digital photo frame.
//!
//! Three kinds of thread, all of which sleep unless there is work to do:
//!   - indexer:   scans the USB stick once at start-up, then exits
//!   - input:     one per input device, blocked until a button is pressed
//!   - presenter: the main thread, wakes once per slide or on a button press
//!
//! They share two things, each a Mutex + Condvar:
//!   - FSMap:   the indexer adds photos as it finds them; the presenter shows
//!              them, starting as soon as the first one is in
//!   - Buttons: the input threads record a press; the presenter acts on it

mod common;
mod config;
mod fb;
mod indexer;
mod presenter;

use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use common::{Button, Buttons, FSMap};

pub type Res<T> = Result<T, Box<dyn std::error::Error>>;

// Linux key codes. The buttons are set up in /boot/firmware/config.txt to act
// as these keys, so a USB keyboard works too (handy for testing).
const KEY_F: u16 = 33; // toggle favorite
const KEY_M: u16 = 50; // next display mode

fn main() -> Res<()> {
    println!("Initing slideshow...");
    // photos.conf: the first argument, or /etc/photos.conf.
    let config_path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(config::DEFAULT_PATH));
    let config = config::Config::load(&config_path);
    let root = config.photos_root.clone();
    println!("Loading PathBuf at {}", root.display());
    println!("Loading Frame Buffer");
    let fb = fb::Fb::open(&config);
    let map = Arc::new(FSMap::new(&root));
    let buttons = Arc::new(Buttons::default());

    println!("Starting threads");
    watch_buttons(&buttons);

    let index_map = Arc::clone(&map);
    thread::spawn(move || {
        indexer::scan(&root, &index_map);
        index_map.finish_indexing();
    });

    presenter::run(fb, map, buttons)
}

/// Start one blocked reader thread per /dev/input/event* device.
fn watch_buttons(buttons: &Arc<Buttons>) {
    for entry in std::fs::read_dir("/dev/input")
        .into_iter()
        .flatten()
        .flatten()
    {
        if !entry.file_name().to_string_lossy().starts_with("event") {
            continue;
        }
        let buttons = Arc::clone(buttons);
        thread::spawn(move || {
            let Ok(mut dev) = File::open(entry.path()) else {
                return;
            };
            // struct input_event: a timeval (two machine words), then
            // u16 type, u16 code, i32 value.
            let len = 2 * std::mem::size_of::<usize>() + 8;
            let mut buf = [0u8; 24];
            while dev.read_exact(&mut buf[..len]).is_ok() {
                let e = &buf[len - 8..len];
                let kind = u16::from_le_bytes([e[0], e[1]]);
                let code = u16::from_le_bytes([e[2], e[3]]);
                let value = i32::from_le_bytes([e[4], e[5], e[6], e[7]]);
                if kind != 1 || value != 1 {
                    continue; // only key-down events
                }
                match code {
                    KEY_F => buttons.press(Button::Favorite),
                    KEY_M => buttons.press(Button::Mode),
                    _ => {}
                }
            }
        });
    }
}
