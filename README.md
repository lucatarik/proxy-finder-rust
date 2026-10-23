# Proxy Finder

Desktop app and CLI in Rust that downloads public proxy lists from GitHub, tests every proxy
(HTTP and HTTPS/CONNECT, http/socks4/socks5) and keeps the working ones. It rates each source,
provider network (ASN) and /24 by how many good proxies it yields, and tests the most promising
first. A local relay on `127.0.0.1:8899` routes any program through the proxy you pick;
`proxy-finder --auto` finds, verifies and connects to the fastest one by itself.

A speedtest (2 MB through each working proxy, 3 at a time; slower ones are cut off as soon as
the fastest finishes) measures download speed: in the GUI with **Speedtest** on the Proxy tab,
from the command line with `proxy-finder --auto --speed`, which picks the fastest download instead
of the lowest latency. Speeds are saved, and the proxy list can be sorted by them (GUI, or
`proxy-finder list --sort speed`).

Prebuilt binaries for Linux (x86_64), macOS (Apple Silicon) and Windows (x86_64) are on the
[Releases](https://github.com/lucatarik/proxy-finder-rust/releases) page. GUI, CLI and relay work
on all three; the system tray icon (`proxy-finder tray`) is Linux only, because it uses the
D-Bus StatusNotifierItem protocol and GNOME's proxy settings. The macOS binary is unsigned: run
`xattr -d com.apple.quarantine proxy-finder` once before starting it.

Build from source:

```bash
cargo build --release        # binary: target/release/proxy-finder
./proxy-finder.sh            # GUI (egui)
./proxy-finder.sh --auto     # find a working HTTPS proxy and start the relay
./proxy-finder.sh help       # CLI commands
```

Requires Rust 1.82+ and, on Linux, X11 or Wayland libraries. Detailed docs (in Italian):
[GUIDA.md](GUIDA.md) for the GUI and source rating, [CLI.md](CLI.md) for the command line.

Licensed under the [MIT License](LICENSE).

Public proxies can see and alter unencrypted traffic: do not use them for banking, passwords or
payments.
