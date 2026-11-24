#!/usr/bin/env python3
"""Watch the Pi's screen live in your browser, over SSH.

    python3 tools/pi-screen.py <user>@<name>.local

Every few seconds it copies what the slideshow is drawing over SSH and shows
it in a browser tab, so you see what a connected screen would show. Needs only
Python's standard library here and SSH access to the Pi. Nothing is installed
on the Pi.

It reads from the same place the slideshow writes to:
  - /dev/fb0 when the Pi has a screen (or HDMI is forced on);
  - otherwise the fb_file from the Pi's /etc/photos.conf, at its fb_size.
It follows the slideshow when it switches between the two.

Options:
    --source WHICH     auto (default), screen (/dev/fb0 only) or file (fb_file only)
    --config PATH      photos.conf on the Pi (default /etc/photos.conf)
    --file PATH        Read this file instead of the config's fb_file
    --size WxH         Size of that file instead of the config's fb_size
    --interval SECS    How often to refresh (default 2)
    --port PORT        Local port for the viewer page (default 8765)
    --no-open          Don't open a browser tab; just print the address
"""
import argparse
import json
import os
import shlex
import subprocess
import sys
import threading
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# Same defaults as the slideshow (src/config.rs).
DEFAULT_FB_SIZE = (1920, 1080)

PAGE = """<!doctype html>
<html><head><meta charset="utf-8"><title>Pi screen</title>
<style>
  body { margin: 0; background: #111; color: #aaa; font: 13px system-ui, sans-serif; }
  canvas { display: block; max-width: 100vw; max-height: calc(100vh - 28px);
           margin: 0 auto; background: #000; }
  #status { height: 28px; line-height: 28px; text-align: center; }
  .error { color: #f77; }
</style></head>
<body>
<div id="status">Connecting...</div>
<canvas id="screen" width="0" height="0"></canvas>
<script>
const status = document.getElementById("status");
const canvas = document.getElementById("screen");
const ctx = canvas.getContext("2d");
let image;

function draw(buf, width, height, stride, bpp) {
  if (!image || image.width !== width || image.height !== height) {
    canvas.width = width;
    canvas.height = height;
    image = ctx.createImageData(width, height);
  }
  const src = new DataView(buf);
  const px = image.data;
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const o = (y * width + x) * 4;
      if (bpp === 32) {
        const s = y * stride + x * 4;           // B G R A
        px[o] = src.getUint8(s + 2);
        px[o + 1] = src.getUint8(s + 1);
        px[o + 2] = src.getUint8(s);
      } else {
        const v = src.getUint16(y * stride + x * 2, true);  // RGB565
        px[o] = (v >> 11) << 3;
        px[o + 1] = ((v >> 5) & 63) << 2;
        px[o + 2] = (v & 31) << 3;
      }
      px[o + 3] = 255;
    }
  }
  ctx.putImageData(image, 0, 0);
}

async function tick(info) {
  try {
    const res = await fetch("/frame", { cache: "no-store" });
    if (!res.ok) throw new Error(await res.text());
    const h = (name) => res.headers.get(name);
    const [width, height] = [+h("X-Width"), +h("X-Height")];
    draw(await res.arrayBuffer(), width, height, +h("X-Stride"), +h("X-Bpp"));
    status.className = "";
    status.textContent = `${info.host} · ${h("X-Source")} · ${width}x${height} · ` +
      `updated ${new Date().toLocaleTimeString()}`;
  } catch (e) {
    status.className = "error";
    status.textContent = "Can't read the screen: " + e.message;
  }
  setTimeout(() => tick(info), info.interval * 1000);
}

fetch("/info").then((r) => r.json()).then(tick);
</script>
</body></html>
"""


class Pi:
    """Runs commands on the Pi over one shared SSH connection."""

    def __init__(self, host):
        self.host = host
        # Reuse one connection (ControlMaster) so each refresh doesn't log in
        # again, and so a password is only asked for once.
        self.opts = [
            "-C",  # compress: black bars and captions shrink a lot
            "-o", "ControlMaster=auto",
            "-o", "ControlPath=" + os.path.expanduser("~/.ssh/pi-screen-%C"),
            "-o", "ControlPersist=60",
        ]
        self.lock = threading.Lock()

    def run(self, command, check=True):
        with self.lock:
            result = subprocess.run(
                ["ssh", *self.opts, self.host, command],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
        if check and result.returncode != 0:
            raise RuntimeError(result.stderr.decode(errors="replace").strip()
                               or f"ssh exited with {result.returncode}")
        return result

    def close(self):
        subprocess.run(["ssh", *self.opts, "-O", "exit", self.host],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def parse_size(text):
    """'1920x1080' -> (1920, 1080), or None if it isn't a size."""
    try:
        w, h = (int(n) for n in text.lower().split("x"))
    except ValueError:
        return None
    return (w, h) if w > 0 and h > 0 else None


def read_config(pi, path):
    """fb_file and fb_size from photos.conf on the Pi, read the same way the
    slideshow reads them. A missing file means defaults."""
    fb_file, fb_size = None, DEFAULT_FB_SIZE
    result = pi.run(f"cat {shlex.quote(path)}", check=False)
    if result.returncode != 0:
        print(f"Couldn't read {path} on the Pi; using defaults (no fb_file)")
        return fb_file, fb_size
    for line in result.stdout.decode(errors="replace").splitlines():
        key, _, value = line.split("#", 1)[0].partition("=")
        key, value = key.strip(), value.strip()
        if key == "fb_file" and value:
            fb_file = value
        elif key == "fb_size" and parse_size(value):
            fb_size = parse_size(value)
    return fb_file, fb_size


def frame_command(source, fb_file, fb_size):
    """A shell command for the Pi that prints one header line describing what
    it read, then the raw pixels:

        screen WIDTH,HEIGHT STRIDE BPP     (from /dev/fb0)
        file                               (from fb_file, at fb_size)
        none REASON                        (nothing to read)
    """
    sysfs = "/sys/class/graphics/fb0"
    screen = (
        f'if [ -e /dev/fb0 ] && [ -r {sysfs}/virtual_size ]; then '
        f'echo screen $(cat {sysfs}/virtual_size {sysfs}/stride {sysfs}/bits_per_pixel); '
        f'exec head -c $(( $(cat {sysfs}/stride) * $(cut -d, -f2 {sysfs}/virtual_size) )) /dev/fb0; '
        f'fi; '
    )
    if fb_file:
        w, h = fb_size
        file = (
            f'if [ -e {shlex.quote(fb_file)} ]; then '
            f'echo file; exec head -c {w * h * 4} {shlex.quote(fb_file)}; fi; '
            f'echo none {shlex.quote(fb_file + " does not exist yet")}'
        )
    else:
        file = "echo none no fb_file is set in photos.conf"
    if source == "screen":
        return screen + "echo none there is no /dev/fb0"
    if source == "file":
        return file
    return screen + file


def main():
    parser = argparse.ArgumentParser(
        description="Watch the Pi's screen live in your browser, over SSH.")
    parser.add_argument("host", help="e.g. pi@frame.local")
    parser.add_argument("--source", choices=["auto", "screen", "file"], default="auto",
                        help="where to read: follow the slideshow (default), "
                             "/dev/fb0 only, or fb_file only")
    parser.add_argument("--config", default="/etc/photos.conf",
                        help="photos.conf on the Pi")
    parser.add_argument("--file", help="read this file instead of the config's fb_file")
    parser.add_argument("--size", help="WIDTHxHEIGHT of that file, instead of fb_size")
    parser.add_argument("--interval", type=float, default=2)
    parser.add_argument("--port", type=int, default=8765)
    parser.add_argument("--no-open", action="store_true")
    args = parser.parse_args()
    if args.size and not parse_size(args.size):
        parser.error("--size must be WIDTHxHEIGHT")

    pi = Pi(args.host)
    try:
        fb_file, fb_size = read_config(pi, args.config)
    except OSError as e:
        sys.exit(f"Couldn't run ssh: {e}")
    fb_file = args.file or fb_file
    fb_size = parse_size(args.size) if args.size else fb_size
    command = frame_command(args.source, fb_file, fb_size)
    info = {"host": args.host, "interval": args.interval}

    def read_frame():
        """(headers, pixels) from the Pi, or raise RuntimeError."""
        out = pi.run(command).stdout
        header, _, pixels = out.partition(b"\n")
        kind, *rest = header.decode(errors="replace").split(" ", 1)
        if kind == "screen":
            size, stride, bpp = rest[0].split()
            width, height = (int(n) for n in size.split(","))
            stride, bpp = int(stride), int(bpp)
            if bpp not in (16, 32):
                raise RuntimeError(f"/dev/fb0 is {bpp}-bit; only 16 and 32 are supported")
            source = "/dev/fb0"
        elif kind == "file":
            (width, height), stride, bpp = fb_size, fb_size[0] * 4, 32
            source = fb_file
        else:
            raise RuntimeError(rest[0] if rest else header.decode(errors="replace"))
        if len(pixels) < stride * height:
            raise RuntimeError(
                f"{source} has {len(pixels)} bytes, expected {stride * height} "
                f"for {width}x{height}" + (" (check fb_size / --size)" if kind == "file" else ""))
        headers = {"X-Source": source, "X-Width": width, "X-Height": height,
                   "X-Stride": stride, "X-Bpp": bpp}
        return headers, pixels

    class Handler(BaseHTTPRequestHandler):
        def send(self, code, kind, body, headers=None):
            self.send_response(code)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            for name, value in (headers or {}).items():
                self.send_header(name, str(value))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if self.path == "/":
                self.send(200, "text/html; charset=utf-8", PAGE.encode())
            elif self.path == "/info":
                self.send(200, "application/json", json.dumps(info).encode())
            elif self.path == "/frame":
                try:
                    headers, pixels = read_frame()
                    self.send(200, "application/octet-stream", pixels, headers)
                except (RuntimeError, ValueError) as e:
                    self.send(502, "text/plain; charset=utf-8", str(e).encode())
            else:
                self.send(404, "text/plain", b"not found")

        def log_message(self, *_):
            pass  # keep the terminal quiet

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    url = f"http://localhost:{args.port}/"
    where = {"auto": "/dev/fb0, falling back to the fb_file",
             "screen": "/dev/fb0", "file": "the fb_file"}[args.source]
    print(f"{args.host}: reading {where}")
    if args.source != "screen":
        print(f"  fb_file: {fb_file or '(not set)'} at {fb_size[0]}x{fb_size[1]}")
    print(f"Viewing at {url} (Ctrl+C to stop)")
    if not args.no_open:
        webbrowser.open(url)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
        pi.close()


if __name__ == "__main__":
    main()
