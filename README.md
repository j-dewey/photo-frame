# Slideshow

## Build on Mac ARM
`DOCKER_DEFAULT_PLATFORM=linux/amd64 cross build --release --target arm-unknown-linux-gnueabihf`

This causes it to build it for Linux rather than Mac

## Overview

A photo frame program for a Raspberry Pi. It shows the JPEGs on a USB stick
one at a time, with the date each was taken in the bottom-left corner.

- One small binary, three small dependencies, no desktop, no GPU, no network.
- It draws straight to the Linux framebuffer and then sleeps. Between slides it uses no CPU at all; it wakes once every 30 seconds or when a button is pressed.
- **Mode button** cycles All Photos -> Spring -> Summer -> Autumn -> Winter ->
  Favorites. The mode name shows in the top-left corner when it changes.
- **Favorite button** moves the photo on screen between `Images/` and
  `Favorites/` on the stick. Favorites have a `*` after the date.

It has been tested against a simulated framebuffer and button device, not yet
on a real Pi, so expect to spend a little time on first boot.

## How it works

| File               | Job                                                            |
| ------------------ | -------------------------------------------------------------- |
| `src/main.rs`      | Starts everything; records button presses                      |
| `src/common.rs`    | `FSMap` (photos by season and favorite) and `Buttons`, shared between threads |
| `src/indexer.rs`   | Runs once at start: adds each photo to the `FSMap` as it reads it |
| `src/presenter.rs` | Runs forever: decodes, scales, captions and shows each photo   |
| `src/fb.rs`        | Writes pixels to `/dev/fb0`                                    |

Things to know:

- **JPEG only.** `.jpg` / `.jpeg`. iPhone HEIC photos must be exported as JPEG
  first. Other files are ignored.
- Photos play in a random order that changes every boot.
- Photos close to the screen's shape are zoomed to fill it (losing at most
  15% off two edges). Others, such as tall phone photos, are shown whole over
  a blurred, darkened copy of themselves instead of black bars. Tune with
  `MAX_CROP`, `BG_DIM` and `BG_SHRINK` in `src/presenter.rs`.
- The slideshow starts as soon as the first photo has been read, while the
  rest of the stick is still being scanned.
- The stick is read once at start-up. Photos that can't be read are logged
  and skipped; the slideshow carries on with the rest.
- Seasons go by month (Mar-May, Jun-Aug, Sep-Nov, Dec-Feb). Photos with no
  "date taken" appear only in All Photos and Favorites, with no caption.
- Only the top level of `Images/` and `Favorites/` is read, not subfolders.
- The slide time is `SLIDE` at the top of `src/presenter.rs`.

## Testing without a screen

Settings live in `photos.conf`: `/etc/photos.conf` on the Pi, or any file
passed as the program's first argument. The example `photos.conf` in this
folder explains each one:

| Setting       | Default        | What it does                                              |
| ------------- | -------------- | --------------------------------------------------------- |
| `photos_root` | `/mnt/photos`  | Folder holding `photos/` and `favorites/`                 |
| `fb_file`     | (none)         | Write each frame to this file while `/dev/fb0` is missing |
| `fb_size`     | `1920x1080`    | Size of the frames written to `fb_file`                   |

If `/dev/fb0` appears later (a screen is plugged in), the program switches to
it at the next slide and stops writing the file. A missing `photos.conf`
means all defaults; a bad line is logged and skipped.

This lets the program run anywhere, including a Mac. Frames are written as
raw 32-bit BGRA pixels, and `tools/fb2png.py` (standard Python only) turns
the latest one into a PNG:

```
cargo build
printf 'photos_root = %s\nfb_file = /tmp/fb.raw\nfb_size = 800x480\n' ~/test-stick > test.conf
./target/debug/slideshow test.conf
# in another terminal, whenever you want to look:
python3 tools/fb2png.py /tmp/fb.raw screen.png 800x480
```

The buttons read `/dev/input`, so they only work on Linux (a USB keyboard's
F and M keys). On a Mac you can check the slideshow, captions and messages,
but not the buttons.

### Watching the Pi's screen from your computer

`tools/pi-screen.py` shows the Pi's screen live in a browser tab, as if a
monitor were plugged in. It copies the picture over SSH every 2 seconds;
nothing needs installing on the Pi.

```
python3 tools/pi-screen.py <user>@<name>.local
```

It reads from wherever the slideshow is drawing: `/dev/fb0` when there is a
screen (or HDMI is forced on, step 3), otherwise the `fb_file` set in the
Pi's `/etc/photos.conf`, at its `fb_size`. It follows the slideshow when it
switches, and the bar at the top says which one you're looking at.

To pick one yourself:

| Option                 | Shows                                                    |
| ---------------------- | -------------------------------------------------------- |
| `--source screen`      | Only `/dev/fb0`                                          |
| `--source file`        | Only the `fb_file`, even if there is a screen            |
| `--file PATH`          | This file instead of the config's `fb_file`              |
| `--size WxH`           | That file's size, instead of the config's `fb_size`      |
| `--config PATH`        | A different `photos.conf` on the Pi                      |

Keep `fb_file` somewhere in RAM like `/dev/shm`: the whole file is rewritten
every slide, which would wear out the SD card.

`--interval` changes how often it refreshes. Each refresh copies a whole
screen (about 8 MB at 1920x1080), so keep it at a few seconds over Wi-Fi.
It only shows the screen; press the buttons (or F and M on a keyboard
plugged into the Pi) to drive it.

## The USB stick

Format it as **exFAT** (or FAT32) and name it **`FRAME`**. Lay it out as:

```
Instructions.txt
Images/       <- put photos here
Favorites/    <- the frame moves favorites here
```

To add photos: unplug the frame, put the stick in a computer, copy JPEGs into
`Images/`, put it back, plug the frame in.

## Parts

- Raspberry Pi. A **Zero 2 W** draws the least power and is fast enough;
  any Pi 3, 4 or 5 also works.
- HDMI screen (plus a mini-HDMI adapter for the Zero)
- microSD card, official power supply
- USB stick (plus a micro-USB OTG adapter for the Zero)
- Two push buttons

## Wiring the buttons

Each button connects a GPIO pin to ground. No resistors needed.

| Button   | GPIO   | Header pin | Other leg to     |
| -------- | ------ | ---------- | ---------------- |
| Favorite | GPIO17 | pin 11     | GND (pin 9)      |
| Mode     | GPIO27 | pin 13     | GND (pin 14)     |

## Setting up the Pi

### 1. Install the OS

Use Raspberry Pi Imager to write **Raspberry Pi OS Lite (64-bit)** to the SD
card. In the Imager settings, set a username and password, add your Wi-Fi and
enable SSH, so you can do the rest from your laptop with `ssh <user>@<name>.local`.

### 2. Make the buttons act as keys

Add to the end of `/boot/firmware/config.txt`:

```
dtoverlay=gpio-key,gpio=17,keycode=33,label="favorite"
dtoverlay=gpio-key,gpio=27,keycode=50,label="mode"
```

Keycode 33 is **F** and 50 is **M**, so you can also plug in a USB keyboard
and press F and M to test without any wiring.

### 3. Keep the console off the screen

In `/boot/firmware/cmdline.txt` (it is a single line; keep it that way),
change `console=tty1` to `console=tty3` and add to the end:

```
quiet logo.nologo vt.global_cursor_default=0
```

Also add this, with your screen's resolution, so the Pi always sets up the
screen (`/dev/fb0`), even if it is switched off or unplugged at boot:

```
video=HDMI-A-1:1920x1080M@60D
```

The `D` at the end forces the HDMI output on. Without it the program still
runs, but draws nothing until a screen is found; it checks again at every
slide.

Then turn off the login prompt on the screen:

```
sudo systemctl disable getty@tty1
```

### 4. Mount the USB stick

```
sudo mkdir /mnt/photos
```

Add this line to `/etc/fstab`:

```
LABEL=FRAME  /mnt/photos  auto  noauto,nofail,x-systemd.automount,x-systemd.device-timeout=5,noatime  0  0
```

The stick is mounted when the program first looks at it, and the Pi still
boots if the stick is missing.

### 5. Build and install

On the Pi (the build takes a few minutes on a Zero 2 W):

```
curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal
. ~/.cargo/env
```

Copy this `slideshow/` folder to the Pi (for example
`scp -r slideshow <user>@<name>.local:`), then:

```
cd slideshow
cargo build --release
sudo cp target/release/slideshow /usr/local/bin/
sudo cp photos.conf /etc/photos.conf
sudo cp slideshow.service /etc/systemd/system/
sudo systemctl enable --now slideshow
```

Photos should appear. If not, `journalctl -u slideshow -e` shows why.

To build on a Mac instead, `cargo zigbuild --release --target
aarch64-unknown-linux-gnu` (with `cargo-zigbuild` and `zig` installed) makes
a binary you can copy over. There is no C code, so nothing else is needed.

### 6. Make it safe to unplug

Do this last, once everything works. Run `sudo raspi-config`, then
**Performance Options -> Overlay File System -> enable**, and make the boot
partition read-only when asked. The SD card is then never written to, so
pulling the power cannot corrupt it. (Turn it off again the same way if you
need to change something.)

The USB stick stays writable for favorites. Each move is flushed to the stick
right away.

## Using less power

The screen's backlight uses far more than the Pi does, so a smaller or dimmer
screen is the biggest saving. For the Pi itself, add to
`/boot/firmware/config.txt`:

```
dtoverlay=disable-bt
dtoverlay=disable-wifi
```

Add the Wi-Fi line only when you are finished, because it also ends SSH
access. A longer `SLIDE` time means fewer wake-ups.

## If something is wrong

- **"No photos found"**: the stick is not named `FRAME`, has no `Images/`
  folder, or holds no JPEGs. The stick is only read at start-up, so fix it,
  then unplug the frame and plug it back in.
- **A photo never shows up**: it couldn't be read and was skipped.
  `journalctl -u slideshow -e` lists each skipped file.
- **Blank screen**: check `ls /dev/fb0` exists. If it doesn't, the screen
  wasn't detected; add the `video=...D` setting from step 3.
  `journalctl -u slideshow -e` says "running headless" while there is no
  screen, and "Found /dev/fb0" once there is.
- **Stray text or a cursor over the photo**: recheck step 3.
- **Colours look banded**: the framebuffer is probably in 16-bit mode. Change
  the `video=` setting in `cmdline.txt` to `video=HDMI-A-1:1920x1080M-32@60D`
  (with your screen's resolution) to ask for 32-bit.
- **A button does nothing**: check the wiring goes to ground, then run
  `sudo apt install evtest && sudo evtest` to see if presses arrive.
