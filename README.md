# Graphic Packet (modern)

Asynchronous Rust foundation for a cross-platform rewrite of **Graphic Packet (GP)**, the classic 1990s MS-DOS AX.25 terminal by Ulf Saran (DH1DAE).

This crate targets modern Windows / macOS / Linux hosts talking to USB-to-serial TNCs (WA8DED hostmode, TFPCX, Kenwood KISS) and soft TNCs over **TCP KISS** (Soundmodem, Direwolf, …).

## Development status

This project is still under active development, and some features may not work
as expected. KISS is currently being tested; the WA8DED and TFPCX drivers are
not yet tested.

The GUI is also still under development. The purpose and behavior of some
legacy screens, including **BBS-Menu** and **Box-List (DieBox)**, are not yet
clear because documentation for them has not been found. Remaining work
includes completing the setup menu and adding a few utilities, such as log
cleanup.

File tranfer or BIN also needs investigating as we currelty cant use the DOS
7Plus compression tool. But aiming for backwards compatibility we lookig in 
to this at a later point

## Layout

| Path | Role |
|------|------|
| `src/config.rs` | Typed `Config` + `Config.toml` loader; maps legacy `CONFIG.GP` keys (`MYCALL`, `SERPORT`, `SERBAUD`, `CHANNELS`, colours, `TNCINI`, …) |
| `src/tnc/` | Modem drivers — WA8DED / TFPCX / KISS serial or TCP (one loaded from `[tnc] protocol`) |
| `src/ui/` | Clean-room TUI (`ratatui`) recreating classic panes — **not** a BGI decompile |
| `docs/LAYOUT.md` | Pane map from V1.63 screenshot + docs |
| `docs/ICONS.md` | Decoded `ICONS.GP` format (70×40 planar VGA, 17 icons) |
| `src/ui/icons.rs` | Runtime decoder for the icon library |
| `data/` | Runtime files shipped beside the exe: GPI text, icons, sounds, `KISS.INI`, `WA8DED.INI`, and `log/` |
| `data/ICONS.GP` | Canonical toolbar icon library (loaded at runtime) |
| `docs/COMPATIBILITY.md` | `//` remotes + `%` macros recovered from `*.GPH` / docs / EXE strings |
| `src/remote/` | Abbreviation-aware command parser + GPI macro expander |
| `docs/TRANSFER.md` | AutoBin `#BIN#` + 7Plus Code-AutoSave (≠ 7-Zip) for DOS GP interop |
| `src/transfer/` | `#BIN#` state machine, CCITT CRC, `go_7+.` / `stop_7+.` sniffer |
| `src/main.rs` | Tokio entry: serial RX task ‖ ratatui UI |
| `Config.toml` | Modern defaults mirroring classic `CONFIG.GP` |

### GUI strategy (no EGAVGA.BGI)

Classic GP used Borland graphics (EGA 640×350 / VGA 640×480). We recreate the
**documented** operator layout (menu tabs, status, RX/connect, split monitor,
input, icon strip) on a modern terminal. Prefer docs + `CONFIG.GP` colour
regions over reverse-engineering `GP2862K.EXE` drawing code.

## Quick start

```bash
# Edit Config.toml — set [station].mycall
# Serial TNC/Kenwood: [serial].port + baud, [tnc] protocol / kiss_transport
# Soundmodem lab: protocol = "kiss", kiss_transport = "tcp", kiss_tcp_*
# Pixel UI loads toolbar bitmaps from data/ICONS.GP
cargo run
# Text fallback:
cargo run -- --tui
```

Optional legacy overlay: place a classic `CONFIG.GP` beside `Config.toml`; recognised keys are merged on load.

```bash
RUST_LOG=debug cargo run
cargo test
```

## Protocol notes

Only **one** modem driver is constructed at startup (`[tnc] protocol`):

| `protocol` | Driver | Notes |
|------------|--------|-------|
| `wa8ded` (aliases `jhost`, `hostmode`) | Timed WA8DED hostmode poll | L2 lives in the TNC; host sends `I`/`C`/`D` hostmode. `JHOST1` → `WA8DED.INI`; exit `[close]`; poll `G0` |
| `tfpcx` | Soft-TNC hostmode | Same hostmode framing; more host-side L2 later (deferred — no BayCom on hand) |
| `kiss` | TAPR KISS | Host builds AX.25 via the [`ax25`](https://crates.io/crates/ax25) crate; `FEND`/`FESC`; serial **or** TCP (`kiss_transport`) |

### KISS transport

| `kiss_transport` | Wire | Typical use |
|------------------|------|-------------|
| `serial` (default) | `[serial]` COM/tty + baud | Kenwood TM-D710 etc. — applies `KISS.INI` (`kiss_section` / `kiss_close_section`), DTR pulse, exit sequence |
| `tcp` | `kiss_tcp_host`:`kiss_tcp_port` | Soft TNC already in KISS — **UZ7HO Soundmodem** (often port `8100`), **Direwolf** (often `8001`); no `KISS.INI` |

Example Soundmodem lab setup in `Config.toml`:

```toml
[tnc]
protocol = "kiss"
kiss_transport = "tcp"
kiss_tcp_host = "127.0.0.1"
kiss_tcp_port = 8100
```

Session badge shows `TCP 127.0.0.1:8100` when the TCP path is active.

KISS TX builds real AX.25 frames with the `ax25` crate (hostmode `I`/`C`/`D` strings are not sent raw). Monitor uses classic GP / axlisten lines, e.g. `fm NL0MSK to NL0MSK ctl SABM+`, `… ctl UA-`, `… ctl I00^ pid F0`, `… ctl RR1v`, optional `via DIGI`:

| Pane | Frame | Addresses |
|------|-------|-----------|
| Monitor (unproto) | UI | `MYCALL` → `CQ` |
| Channel connect | SABM → peer; answer SABM with **UA** (or **DM** if busy) | `MYCALL` ↔ peer |
| Channel linked | I (+ RR on RX); idle **RR poll every 5 min**, drop after 3×30 s no reply | `MYCALL` ↔ peer |
| Channel hangup | DISC (+ UA on peer DISC) | |

- **JHOST1** — enter WA8DED binary hostmode from terminal mode  
- **JHOST0** — leave hostmode (WA8DED path)  
- Hostmode frame shape: `channel \| code \| len \| payload[len]`  
- `[serial] fast_poll` (`on` / `off` / `auto`) sets the WA8DED/TFPCX poll interval

## Credits

7Plus Code-AutoSave follows the file format of **7PLUS 2.2** by Axel Bauda, DG1BBQ. The part names (`picture.p01`, a single part as `picture.7pl`, part 10 as `picture.p0a`) and the `go_7+.` / `stop_7+.` markers are taken from the C sources maintained by HB9XAR:

[https://github.com/hb9xar/7plus](https://github.com/hb9xar/7plus)

That program is not linked or copied into this crate. GPNG only saves and sends the text parts. Decoding them still uses the external `7plus` tool. 7PLUS itself is free to circulate and is not covered by the MIT / Apache-2.0 licence below (“no commercial use, no sale”).

## License

MIT OR Apache-2.0. The original GP binaries and docs remain under their historical licence terms in the parent tree. The 7PLUS program credited above stays under its own terms.
