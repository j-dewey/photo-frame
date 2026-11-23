#!/usr/bin/env python3
"""Watch the Pi's screen live in your browser, over SSH.

    python3 tools/pi-screen.py <user>@<name>.local

Every few seconds it copies the Pi's framebuffer over SSH and draws it in a
browser tab, so you see what a connected screen would show. Needs only
Python's standard library here and SSH access to the Pi. Nothing is installed
on the Pi.

The Pi needs a framebuffer to read. With no monitor plugged in, either force
HDMI on (README step 3), or point --device at the program's fb_file (set in
/etc/photos.conf; /dev/shm/fb.raw in the example), giving --size too since a
plain file has no size information.

Options:
    --device PATH      What to read on the Pi (default /dev/fb0)
    --size WxH         Skip reading the size from the Pi; assume 32-bit pixels
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

PAGE = """<!doctype html>
<html><head><meta charset="utf-8"><title>Pi screen</title>
<style>
  body { margin: 0; background: #111; color: #aaa; font: 13px system-ui, sans-serif; }
  canvas { display: block; max-width: 100vw; max-height: calc(100vh - 28px);
           margin: 0 auto; background: #000; image-rendering: auto; }
  #status { height: 28px; line-height: 28px; text-align: center; }
  .error { color: #f77; }
</style></head>
<body>
<div id="status">Connecting...</div>
<canvas id="screen"></canvas>
<script>
const status = document.getElementById("status");
const canvas = document.getElementById("screen");
const ctx = canvas.getContext("2d");
let info, image;

function draw(buf) {
  const src = new DataView(buf);
  const px = image.data;
  const { width, height, stride, bpp } = info;
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

async function tick() {
  try {
    const res = await fetch("/frame", { cache: "no-store" });
    if (!res.ok) throw new Error(await res.text());
    draw(await res.arrayBuffer());
    status.className = "";
    status.textContent = `${info.host} · ${info.width}x${info.height} · updated ${new Date().toLocaleTimeString()}`;
  } catch (e) {
    status.className = "error";
    status.textContent = "Can't read the screen: " + e.message;
  }
  setTimeout(tick, info.interval * 1000);
}

(async () => {
  info = await (await fetch("/info")).json();
  canvas.width = info.width;
  canvas.height = info.height;
  image = ctx.createImageData(info.width, info.height);
  tick();
})();
</script>
</body></html>
"""


class Pi:
    """Runs commands on the Pi over one shared SSH connection."""

    def __init__(self, host):
        self.host = host
        # Reuse one connection (ControlMaster) so each refresh doesn't log in
        # again, and so a password is only asked for once.
        self.control = os.path.expanduser("~/.ssh/pi-screen-%C")
        self.opts = [
            "-C",  # compress: black bars and captions shrink a lot
            "-o", "ControlMaster=auto",
            "-o", f"ControlPath={self.control}",
            "-o", "ControlPersist=60",
        ]
        self.lock = threading.Lock()

    def run(self, command):
        with self.lock:
            result = subprocess.run(
                ["ssh", *self.opts, self.host, command],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
        if result.returncode != 0:
            raise RuntimeError(result.stderr.decode(errors="replace").strip()
                               or f"ssh exited with {result.returncode}")
        return result.stdout

    def close(self):
        subprocess.run(["ssh", *self.opts, "-O", "exit", self.host],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def screen_info(pi, args):
    if args.size:
        width, height = (int(n) for n in args.size.lower().split("x"))
        return {"width": width, "height": height, "stride": width * 4, "bpp": 32}
    name = os.path.basename(args.device)  # fb0
    sysfs = f"/sys/class/graphics/{name}"
    out = pi.run(f"cat {sysfs}/virtual_size {sysfs}/bits_per_pixel {sysfs}/stride")
    size, bpp, stride = out.decode().split()
    width, height = (int(n) for n in size.split(","))
    bpp = int(bpp)
    if bpp not in (16, 32):
        sys.exit(f"The framebuffer is {bpp}-bit; only 16 and 32 are supported")
    return {"width": width, "height": height, "stride": int(stride), "bpp": bpp}


def main():
    parser = argparse.ArgumentParser(
        description="Watch the Pi's screen live in your browser, over SSH.")
    parser.add_argument("host", help="e.g. pi@frame.local")
    parser.add_argument("--device", default="/dev/fb0")
    parser.add_argument("--size", help="WIDTHxHEIGHT, e.g. for an fb_file")
    parser.add_argument("--interval", type=float, default=2)
    parser.add_argument("--port", type=int, default=8765)
    parser.add_argument("--no-open", action="store_true")
    args = parser.parse_args()

    pi = Pi(args.host)
    try:
        info = screen_info(pi, args)
    except RuntimeError as e:
        sys.exit(f"Couldn't read the screen size from {args.host}: {e}\n"
                 "Is a screen connected (or HDMI forced on)? For an fb_file, pass --size.")
    info.update(host=args.host, interval=args.interval)
    frame_bytes = info["stride"] * info["height"]
    # /dev/fb0 can be bigger than one screen (room for panning): read just one.
    frame_cmd = f"head -c {frame_bytes} {shlex.quote(args.device)}"

    class Handler(BaseHTTPRequestHandler):
        def send(self, code, kind, body):
            self.send_response(code)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if self.path == "/":
                self.send(200, "text/html; charset=utf-8", PAGE.encode())
            elif self.path == "/info":
                self.send(200, "application/json", json.dumps(info).encode())
            elif self.path == "/frame":
                try:
                    frame = pi.run(frame_cmd)
                    if len(frame) < frame_bytes:
                        raise RuntimeError(
                            f"got {len(frame)} bytes, expected {frame_bytes}")
                    self.send(200, "application/octet-stream", frame)
                except RuntimeError as e:
                    self.send(502, "text/plain; charset=utf-8", str(e).encode())
            else:
                self.send(404, "text/plain", b"not found")

        def log_message(self, *_):
            pass  # keep the terminal quiet

    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    url = f"http://localhost:{args.port}/"
    print(f"{args.host}: {info['width']}x{info['height']}, {info['bpp']}-bit")
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
