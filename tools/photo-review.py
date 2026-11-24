#!/usr/bin/env python3
"""Go through the photos on the USB stick one by one and keep or delete each.

    tools/.venv/bin/python tools/photo-review.py "/Volumes/UBUNTU 26_0"

First it deletes every .mov video on the stick (after showing how many and
asking y/N; skip with --keep-movies). Then a window shows each photo with the
date it was taken in the bottom-left corner:

    Right arrow    keep it, go to the next photo
    Left arrow     delete it (moved to your Mac's Trash), go to the next photo
    U / Backspace  undo the last keep or delete
    Esc / Q        quit (progress is saved; run again to carry on)

Photos you already kept are remembered in tools/.photo-review-kept.txt, so a
second run starts where you left off. Use --restart to review everything again.

Needs pygame and Pillow (pillow-heif too, for iPhone .heic photos):
    python3 -m venv tools/.venv
    tools/.venv/bin/pip install pygame pillow pillow-heif
"""
import argparse
import datetime
import os
import shutil
import sys
from pathlib import Path

import pygame
from PIL import Image, ImageOps

try:
    from pillow_heif import register_heif_opener

    register_heif_opener()
    HEIC = True
except ImportError:
    HEIC = False

IMAGE_EXTS = {".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp", ".tif", ".tiff"}
if HEIC:
    IMAGE_EXTS |= {".heic", ".heif"}

KEPT_FILE = Path(__file__).resolve().parent / ".photo-review-kept.txt"
TRASH = None  # set in main(): the stick's own Trash folder (see trash_dir)

# EXIF tags holding when the photo was taken, best first.
EXIF_IFD = 0x8769
DATE_TAGS = [(EXIF_IFD, 0x9003), (EXIF_IFD, 0x9004), (None, 0x0132)]


def visible_files(root):
    """Every file under root, skipping macOS junk (._ files, hidden folders)."""
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if not d.startswith(".")]
        for name in filenames:
            if not name.startswith("."):
                yield Path(dirpath) / name


def remove_with_companion(path):
    """Delete path, plus the ._name file macOS leaves beside it on FAT drives."""
    path.unlink()
    companion = path.with_name("._" + path.name)
    if companion.exists():
        companion.unlink()


def delete_movies(root, ask):
    movies = sorted(p for p in visible_files(root) if p.suffix.lower() == ".mov")
    if not movies:
        print("No .mov files found.")
        return
    size = sum(p.stat().st_size for p in movies) / 1e9
    print(f"Found {len(movies)} .mov files ({size:.2f} GB).")
    if ask and input("Delete them permanently? [y/N] ").strip().lower() != "y":
        print("Kept the .mov files.")
        return
    for p in movies:
        remove_with_companion(p)
    print(f"Deleted {len(movies)} .mov files.")


def photo_date(path, img):
    """When the photo was taken (from EXIF), else the file's modified time."""
    try:
        exif = img.getexif()
        for ifd, tag in DATE_TAGS:
            value = (exif.get_ifd(ifd) if ifd else exif).get(tag)
            if value:
                return datetime.datetime.strptime(str(value).strip()[:19], "%Y:%m:%d %H:%M:%S")
    except (ValueError, KeyError, OSError):
        pass
    return datetime.datetime.fromtimestamp(path.stat().st_mtime)


def load_photo(path, screen_size):
    """The photo scaled to fit the window, and its date as text."""
    with Image.open(path) as img:
        date = photo_date(path, img)
        img = ImageOps.exif_transpose(img).convert("RGB")
        img.thumbnail(screen_size, Image.LANCZOS)
        surface = pygame.image.frombytes(img.tobytes(), img.size, "RGB")
    return surface, date.strftime("%-d %B %Y")


def trash_dir(root):
    """Where Finder puts deleted files from this drive, so they show in Trash.

    On a USB stick that's /Volumes/<name>/.Trashes/<your user id>; moving there
    is instant because the file stays on the same drive.
    """
    mount = root.resolve()
    while not os.path.ismount(mount):
        mount = mount.parent
    if str(mount).startswith("/Volumes/"):
        return mount / ".Trashes" / str(os.getuid())
    return Path.home() / ".Trash"


def move_to_trash(path):
    """Move path to the Mac's Trash (unique name); return where it went."""
    dest = TRASH / path.name
    n = 1
    while dest.exists():
        dest = TRASH / f"{path.stem} ({n}){path.suffix}"
        n += 1
    TRASH.mkdir(parents=True, exist_ok=True)
    shutil.move(str(path), dest)
    companion = path.with_name("._" + path.name)
    if companion.exists():
        companion.unlink()
    return dest


def read_kept():
    if not KEPT_FILE.exists():
        return set()
    return set(KEPT_FILE.read_text().splitlines())


def write_kept(kept):
    KEPT_FILE.write_text("".join(k + "\n" for k in sorted(kept)))


def draw_text(screen, font, text, pos, anchor="bottomleft"):
    """White text with a dark shadow so it reads on any photo."""
    shadow = font.render(text, True, (0, 0, 0))
    label = font.render(text, True, (255, 255, 255))
    rect = label.get_rect(**{anchor: pos})
    for dx, dy in ((2, 2), (-1, -1), (2, -1), (-1, 2)):
        screen.blit(shadow, rect.move(dx, dy))
    screen.blit(label, rect)


def review(root, restart):
    kept = set() if restart else read_kept()
    photos = sorted(
        (p for p in visible_files(root) if p.suffix.lower() in IMAGE_EXTS),
        key=lambda p: str(p).lower(),
    )
    todo = [p for p in photos if str(p.relative_to(root)) not in kept]
    print(f"{len(photos)} photos, {len(todo)} left to review.")
    if not HEIC:
        print("Note: pillow-heif isn't installed, so .heic photos are skipped.")
    if not todo:
        return

    pygame.init()
    info = pygame.display.Info()
    size = (int(info.current_w * 0.9), int(info.current_h * 0.85))
    screen = pygame.display.set_mode(size, pygame.RESIZABLE)
    pygame.display.set_caption("Photo review  —  ← delete   → keep   U undo   Esc quit")
    big = pygame.font.SysFont("helvetica", 40, bold=True)
    small = pygame.font.SysFont("helvetica", 22)

    history = []  # ("keep", path) or ("delete", path, trash_path)
    deleted = 0
    i = 0
    cache = {}

    def current():
        p = todo[i]
        if p not in cache:
            cache.clear()
            try:
                cache[p] = load_photo(p, screen.get_size())
            except Exception as e:  # unreadable or unsupported file
                cache[p] = (None, f"Can't open this file: {e}")
        return cache[p]

    running = True
    while running and i < len(todo):
        surface, date = current()
        screen.fill((15, 15, 15))
        w, h = screen.get_size()
        if surface:
            screen.blit(surface, surface.get_rect(center=(w // 2, h // 2)))
        draw_text(screen, big, date, (24, h - 20))
        draw_text(screen, small, f"{i + 1} / {len(todo)}   {todo[i].name}", (24, 16), "topleft")
        draw_text(screen, small, "Left: delete    Right: keep    U: undo", (w - 24, 16), "topright")
        pygame.display.flip()

        event = pygame.event.wait()
        if event.type == pygame.QUIT:
            running = False
        elif event.type == pygame.VIDEORESIZE:
            cache.clear()
        elif event.type == pygame.KEYDOWN:
            p = todo[i]
            if event.key in (pygame.K_ESCAPE, pygame.K_q):
                running = False
            elif event.key == pygame.K_RIGHT:
                kept.add(str(p.relative_to(root)))
                write_kept(kept)
                history.append(("keep", p))
                i += 1
            elif event.key == pygame.K_LEFT:
                history.append(("delete", p, move_to_trash(p)))
                deleted += 1
                i += 1
            elif event.key in (pygame.K_u, pygame.K_BACKSPACE) and history:
                last = history.pop()
                if last[0] == "keep":
                    kept.discard(str(last[1].relative_to(root)))
                    write_kept(kept)
                else:
                    shutil.move(str(last[2]), last[1])
                    deleted -= 1
                cache.clear()
                i -= 1

    pygame.quit()
    print(f"Deleted {deleted} photos (they're in your Trash). "
          f"{len(todo) - i} left to review." if i < len(todo) else
          f"Deleted {deleted} photos (they're in your Trash). All done!")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("root", type=Path, help="The USB stick, e.g. '/Volumes/UBUNTU 26_0'")
    parser.add_argument("--keep-movies", action="store_true", help="Don't delete .mov files")
    parser.add_argument("--yes", action="store_true", help="Delete .mov files without asking")
    parser.add_argument("--movies-only", action="store_true", help="Delete .mov files, then stop")
    parser.add_argument("--trash", type=Path, help="Move deleted photos here instead of the Trash")
    parser.add_argument("--restart", action="store_true", help="Forget kept photos; review everything")
    args = parser.parse_args()

    if not args.root.is_dir():
        sys.exit(f"{args.root} is not a folder. Is the USB plugged in?")
    global TRASH
    TRASH = args.trash or trash_dir(args.root)
    if not args.keep_movies:
        delete_movies(args.root, ask=not args.yes)
    if not args.movies_only:
        review(args.root, args.restart)


if __name__ == "__main__":
    main()
