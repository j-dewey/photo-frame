//! Digital photo frame.
//!
//! Three kinds of thread, all of which sleep unless there is work to do:
//!   - indexer:   scans the USB stick once at start-up, then exits
//!   - input:     one per input device, blocked until a button is pressed
//!   - presenter: the main thread, wakes once per slide or on an event

mod common;
mod fb;
mod indexer;
mod presenter;

use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread;

pub type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// Where the USB stick is mounted, unless a path is given as the first argument.
const DEFAULT_ROOT: &str = "/mnt/photos";

// Linux key codes. The buttons are set up in /boot/firmware/config.txt to act
// as these keys, so a USB keyboard works too (handy for testing).
const KEY_F: u16 = 33; // toggle favorite
const KEY_M: u16 = 50; // next display mode

pub enum Event {
    Index(Vec<common::Photo>),
    ToggleFavorite,
    NextMode,
}

fn main() -> Res<()> {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or(DEFAULT_ROOT.into()));
    let fb = fb::Fb::open()?;
    let (tx, rx) = mpsc::channel();

    watch_buttons(&tx);

    let (index_tx, index_root) = (tx.clone(), root.clone());
    thread::spawn(move || {
        let _ = index_tx.send(Event::Index(indexer::scan(&index_root)));
    });

    // `tx` stays alive here so the presenter's channel never disconnects.
    presenter::run(fb, rx, &root)
}

/// Start one blocked reader thread per /dev/input/event* device.
fn watch_buttons(tx: &Sender<Event>) {
    for entry in std::fs::read_dir("/dev/input")
        .into_iter()
        .flatten()
        .flatten()
    {
        if !entry.file_name().to_string_lossy().starts_with("event") {
            continue;
        }
        let tx = tx.clone();
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
                let _ = match code {
                    KEY_F => tx.send(Event::ToggleFavorite),
                    KEY_M => tx.send(Event::NextMode),
                    _ => Ok(()),
                };
            }
        });
    }
}
