#!/usr/bin/env python3
"""Sort the photos on the USB stick into groups, one photo at a time.

    tools/.venv/bin/python tools/photo-group.py "/Volumes/FRAME"

Opens a page in your browser. The photo and the date it was taken are on the
left; your groups are on the right. Click a group (or press its number) to
move the photo into that group's folder, or "No group" (N) to leave it where
it is. "+ New group" makes a group. U undoes the last choice.

A group is a folder inside photos/ on the stick, and the frame shows its name
above the date. By default only photos not in a group yet are shown; photos
you passed with "No group" are remembered in tools/.photo-group-skipped.txt,
so a second run carries on where you left off.

    --all       go through every photo, including ones already in a group
                ("No group" then takes a photo out of its group)
    --restart   forget the photos you passed with "No group"

Moving a favorite keeps it a favorite: favorites.txt is updated to match.
Needs Pillow for dates (photos without one show their file date):
    tools/.venv/bin/pip install pillow
"""
import argparse
import datetime
import json
import shutil
import sys
import threading
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    Image = None

IMAGES = "photos"  # must match IMAGES in src/indexer.rs
FAVORITES_FILE = "favorites.txt"  # must match FAVORITES_FILE in src/indexer.rs
SKIPPED_FILE = Path(__file__).resolve().parent / ".photo-group-skipped.txt"
JPEG = {".jpg", ".jpeg"}
# Characters a folder name can't have on an exFAT / FAT32 stick.
BAD_CHARS = set('/\\:*?"<>|')

EXIF_IFD = 0x8769
DATE_TAGS = [(EXIF_IFD, 0x9003), (EXIF_IFD, 0x9004), (None, 0x0132)]


def visible(folder):
    """Entries of folder, skipping hidden ones (macOS "._" junk, .Trashes)."""
    if not folder.is_dir():
        return []
    return sorted((p for p in folder.iterdir() if not p.name.startswith(".")),
                  key=lambda p: p.name.lower())


def photo_date(path):
    """When the photo was taken, as text, and whether that came from EXIF."""
    if Image:
        try:
            with Image.open(path) as img:
                exif = img.getexif()
                for ifd, tag in DATE_TAGS:
                    value = (exif.get_ifd(ifd) if ifd else exif).get(tag)
                    if value:
                        d = datetime.datetime.strptime(str(value).strip()[:19], "%Y:%m:%d %H:%M:%S")
                        return d.strftime("%-d %B %Y"), True
        except (ValueError, KeyError, OSError):
            pass
    d = datetime.datetime.fromtimestamp(path.stat().st_mtime)
    return d.strftime("%-d %B %Y"), False


class Sorter:
    """The photos to go through, the groups, and what has been done so far."""

    def __init__(self, root, include_grouped, restart):
        self.root = root
        self.images = root / IMAGES
        self.lock = threading.Lock()
        self.skipped = set() if restart else self.read_skipped()
        self.history = []  # (old path, new path) for each choice, newest last
        self.queue = []
        for entry in visible(self.images):
            if entry.is_dir():
                if include_grouped:
                    self.queue += [p for p in visible(entry) if p.suffix.lower() in JPEG]
            elif entry.suffix.lower() in JPEG and self.rel(entry) not in self.skipped:
                self.queue.append(entry)
        self.pos = 0

    # --- files on the stick -------------------------------------------------

    def rel(self, path):
        """How favorites.txt and the skipped list write a photo."""
        return path.relative_to(self.images).as_posix()

    def groups(self):
        return [{"name": d.name, "count": sum(p.suffix.lower() in JPEG for p in visible(d))}
                for d in visible(self.images) if d.is_dir()]

    def read_skipped(self):
        if not SKIPPED_FILE.exists():
            return set()
        return set(SKIPPED_FILE.read_text().splitlines())

    def write_skipped(self):
        SKIPPED_FILE.write_text("".join(s + "\n" for s in sorted(self.skipped)))

    def rename_favorite(self, old, new):
        """Keep favorites.txt pointing at a photo that moved."""
        path = self.root / FAVORITES_FILE
        if not path.exists():
            return
        lines = path.read_text().splitlines()
        if self.rel(old) not in lines:
            return
        lines = [self.rel(new) if line == self.rel(old) else line for line in lines]
        tmp = path.with_suffix(".tmp")
        tmp.write_text("".join(line + "\n" for line in lines))
        tmp.replace(path)

    def move(self, src, folder, name=None):
        """Move a photo (and the "._" file macOS keeps beside it) into
        folder as `name` (default: its own name), adding (2), (3)... to the
        name if it is taken."""
        folder.mkdir(exist_ok=True)
        name = Path(name or src.name)
        dest = folder / name
        n = 2
        while dest.exists():
            dest = folder / f"{name.stem} ({n}){name.suffix}"
            n += 1
        shutil.move(str(src), dest)
        companion = src.with_name("._" + src.name)
        if companion.exists():
            shutil.move(str(companion), dest.with_name("._" + dest.name))
        self.rename_favorite(src, dest)
        return dest

    # --- actions from the page ----------------------------------------------

    def state(self, error=None):
        current = None
        if self.pos < len(self.queue):
            path = self.queue[self.pos]
            date, from_exif = photo_date(path)
            group = path.parent.name if path.parent != self.images else None
            current = {"name": path.name, "group": group, "date": date,
                       "exifDate": from_exif, "url": f"/photo/{self.pos}"}
        return {"current": current, "index": self.pos, "total": len(self.queue),
                "groups": self.groups(), "canUndo": bool(self.history), "error": error}

    def assign(self, group):
        """Put the current photo in `group`, or in no group if None."""
        if self.pos >= len(self.queue):
            return self.state()
        src = self.queue[self.pos]
        folder = self.images / group if group else self.images
        if group and not (folder.is_dir() and folder.parent == self.images):
            return self.state(f"There is no group called {group!r}")
        dest = src if src.parent == folder else self.move(src, folder)
        if not group:
            self.skipped.add(self.rel(dest))
            self.write_skipped()
        self.history.append((src, dest))
        self.queue[self.pos] = dest
        self.pos += 1
        return self.state()

    def undo(self):
        if not self.history:
            return self.state()
        old, new = self.history.pop()
        self.skipped.discard(self.rel(new))
        self.write_skipped()
        if new != old:
            new = self.move(new, old.parent, old.name)
        self.pos -= 1
        self.queue[self.pos] = new
        return self.state()

    def new_group(self, name):
        name = " ".join(name.split())  # no stray or doubled spaces
        if not name:
            return self.state("Type a name for the group")
        if name.startswith(".") or BAD_CHARS & set(name):
            return self.state('A group name can\'t start with "." or contain / \\ : * ? " < > |')
        if any(g["name"].lower() == name.lower() for g in self.groups()):
            return self.state(f"There is already a group called {name!r}")
        (self.images / name).mkdir()
        return self.state()


PAGE = """<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Photo groups</title>
<style>
  * { box-sizing: border-box; }
  [hidden] { display: none !important; }
  body { margin: 0; height: 100vh; display: flex; font: 15px -apple-system, system-ui, sans-serif;
         background: #151515; color: #eee; }
  #photo { flex: 1; display: flex; flex-direction: column; min-width: 0; padding: 16px; }
  #top { display: flex; justify-content: space-between; color: #999; margin-bottom: 10px; }
  #frame { flex: 1; min-height: 0; display: flex; align-items: center; justify-content: center; }
  #frame img { max-width: 100%; max-height: 100%; object-fit: contain; border-radius: 4px; }
  #date { font-size: 28px; font-weight: 600; margin-top: 12px; }
  #date small { font-size: 14px; font-weight: 400; color: #999; margin-left: 8px; }
  #side { width: 300px; background: #1f1f1f; padding: 16px; display: flex; flex-direction: column;
          gap: 8px; overflow-y: auto; }
  h2 { font-size: 13px; text-transform: uppercase; letter-spacing: .06em; color: #999; margin: 0 0 4px; }
  button { font: inherit; color: inherit; background: #2c2c2c; border: 1px solid #3a3a3a;
           border-radius: 6px; padding: 10px 12px; text-align: left; cursor: pointer;
           display: flex; gap: 10px; align-items: center; }
  button:hover:not(:disabled) { background: #383838; }
  button:disabled { opacity: .4; cursor: default; }
  button.current { border-color: #6aa0ff; }
  .key { color: #888; font-size: 12px; min-width: 14px; }
  .count { margin-left: auto; color: #888; font-size: 13px; }
  #none { background: none; justify-content: center; margin-top: 4px; }
  #new { display: flex; gap: 6px; }
  #new input { flex: 1; min-width: 0; font: inherit; color: inherit; background: #151515;
               border: 1px solid #3a3a3a; border-radius: 6px; padding: 8px; }
  #error { color: #ff8a80; min-height: 1em; }
  #undo { background: none; justify-content: center; margin-top: auto; }
  #done { margin: auto; text-align: center; color: #999; font-size: 18px; }
</style></head><body>
<main id="photo">
  <div id="top"><span id="progress"></span><span id="file"></span></div>
  <div id="frame"><img id="img" alt=""></div>
  <div id="date"></div>
</main>
<aside id="side">
  <h2>Groups</h2>
  <div id="groups" style="display:flex;flex-direction:column;gap:8px"></div>
  <button id="add">+ New group</button>
  <form id="new" hidden><input id="name" placeholder="Group name" maxlength="60" autocomplete="off">
    <button type="submit">Add</button></form>
  <div id="error"></div>
  <button id="none"><span class="key">N</span>No group</button>
  <button id="undo"><span class="key">U</span>Undo</button>
</aside>
<script>
const $ = id => document.getElementById(id);
let state = null;

async function call(path, body) {
  const res = await fetch(path, body === undefined ? {} :
    { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
  show(await res.json());
}

function show(s) {
  state = s;
  $("error").textContent = s.error || "";
  $("undo").disabled = !s.canUndo;
  const c = s.current;
  $("none").disabled = !c;
  $("progress").textContent = c ? `${s.index + 1} of ${s.total}` : "";
  $("file").textContent = c ? (c.group ? `${c.group} / ${c.name}` : c.name) : "";
  if (c) {
    $("img").hidden = false;
    $("img").src = c.url;
    $("date").innerHTML = "";
    $("date").append(c.date);
    if (!c.exifDate) $("date").insertAdjacentHTML("beforeend", "<small>file date (no date taken)</small>");
    $("frame").querySelector("#done")?.remove();
  } else {
    $("img").hidden = true;
    $("date").textContent = "";
    if (!$("done")) $("frame").insertAdjacentHTML("beforeend",
      `<div id="done">${s.total ? "All done!" : "No photos left to sort."}<br>You can close this tab.</div>`);
  }
  const list = $("groups");
  list.innerHTML = "";
  if (!s.groups.length) list.innerHTML = '<div style="color:#888">No groups yet</div>';
  s.groups.forEach((g, i) => {
    const b = document.createElement("button");
    b.disabled = !c;
    b.classList.toggle("current", !!c && c.group === g.name);
    b.innerHTML = `<span class="key">${i < 9 ? i + 1 : ""}</span><span></span><span class="count"></span>`;
    b.children[1].textContent = g.name;
    b.children[2].textContent = g.count;
    b.onclick = () => call("/assign", { group: g.name });
    list.append(b);
  });
}

$("none").onclick = () => call("/assign", { group: null });
$("undo").onclick = () => call("/undo", {});
$("add").onclick = () => { $("add").hidden = true; $("new").hidden = false; $("name").focus(); };
$("new").onsubmit = async e => {
  e.preventDefault();
  await call("/group", { name: $("name").value });
  if (!state.error) { $("name").value = ""; $("new").hidden = true; $("add").hidden = false; }
};
$("name").onkeydown = e => {
  if (e.key === "Escape") { $("new").hidden = true; $("add").hidden = false; $("error").textContent = ""; }
};
document.onkeydown = e => {
  if (e.target.tagName === "INPUT" || e.metaKey || e.ctrlKey) return;
  const k = e.key.toLowerCase();
  if (k === "u" && state.canUndo) call("/undo", {});
  if (!state.current) return;
  if (k === "n") call("/assign", { group: null });
  if (k >= "1" && k <= "9" && state.groups[k - 1]) call("/assign", { group: state.groups[k - 1].name });
};
call("/state");
</script></body></html>
"""


def make_handler(sorter):
    class Handler(BaseHTTPRequestHandler):
        def send(self, code, body, kind):
            self.send_response(code)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)

        def send_json(self, data):
            self.send(200, json.dumps(data).encode(), "application/json")

        def do_GET(self):
            with sorter.lock:
                if self.path == "/":
                    return self.send(200, PAGE.encode(), "text/html; charset=utf-8")
                if self.path == "/state":
                    return self.send_json(sorter.state())
                if self.path.startswith("/photo/"):
                    # Only ever serves a photo from the list, by its number.
                    try:
                        path = sorter.queue[int(self.path.split("/")[2])]
                        return self.send(200, path.read_bytes(), "image/jpeg")
                    except (ValueError, IndexError, OSError):
                        pass
            self.send(404, b"Not found", "text/plain")

        def do_POST(self):
            try:
                length = int(self.headers.get("Content-Length", 0))
                body = json.loads(self.rfile.read(length) or b"{}")
            except ValueError:
                return self.send(400, b"Bad request", "text/plain")
            with sorter.lock:
                try:
                    if self.path == "/assign":
                        return self.send_json(sorter.assign(body.get("group")))
                    if self.path == "/undo":
                        return self.send_json(sorter.undo())
                    if self.path == "/group":
                        return self.send_json(sorter.new_group(str(body.get("name", ""))))
                except OSError as e:  # stick pulled out, read-only, ...
                    return self.send_json(sorter.state(f"Couldn't change the stick: {e}"))
            self.send(404, b"Not found", "text/plain")

        def log_message(self, *args):
            pass  # keep the terminal quiet

    return Handler


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("root", type=Path, help="The USB stick, e.g. /Volumes/FRAME")
    parser.add_argument("--all", action="store_true", help="Also go through photos already in a group")
    parser.add_argument("--restart", action="store_true", help="Forget photos passed with No group")
    parser.add_argument("--port", type=int, default=8765, help="Port for the page (default 8765)")
    parser.add_argument("--no-browser", action="store_true", help="Don't open the browser")
    args = parser.parse_args()

    if not (args.root / IMAGES).is_dir():
        sys.exit(f"No {IMAGES}/ folder in {args.root}. Is the USB stick plugged in?")
    if not Image:
        print("Note: Pillow isn't installed, so photos show their file date, not the date taken.")
    sorter = Sorter(args.root, args.all, args.restart)
    print(f"{len(sorter.queue)} photos to sort, {len(sorter.groups())} groups so far.")

    # Only this computer can open the page.
    server = ThreadingHTTPServer(("127.0.0.1", args.port), make_handler(sorter))
    url = f"http://127.0.0.1:{args.port}/"
    print(f"Open {url} (Ctrl+C here to stop).")
    if not args.no_browser:
        webbrowser.open(url)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nStopped. Run again any time to carry on.")


if __name__ == "__main__":
    main()
