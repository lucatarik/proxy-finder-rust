# Proxy Finder

Desktop app and CLI in Rust that downloads public proxy lists from GitHub, tests every proxy
(HTTP and HTTPS/CONNECT, http/socks4/socks5), keeps the working ones and rates each source by how
many good proxies it yields. A local relay on `127.0.0.1:8899` routes any program through the
proxy you pick; `proxy-finder --auto` finds, verifies and connects to the fastest one by itself.

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
