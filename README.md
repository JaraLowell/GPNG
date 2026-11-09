# Graphic Packet (New Generation)

Asynchronous Rust foundation for a cross-platform rewrite of **Graphic Packet (GP)**, the classic 1990s MS-DOS AX.25 HAM Packet Radio terminal by Ulf Saran (DH1DAE).

This crate targets modern Windows / macOS / Linux hosts talking to USB-to-serial TNCs (WA8DED hostmode, TFPCX, Kenwood KISS) and soft TNCs over **TCP KISS** (Soundmodem, Direwolf, …).

![afbeelding](https://i.gyazo.com/984bd59351bff904028a41b37ec625aa.png)

## Development status

This project is still under active development, and some features may not work
as expected. KISS is currently being tested; the WA8DED and TFPCX drivers are
not yet tested.

The GUI is also still under development. The purpose and behavior of some
legacy screens, including **BBS-Menu** and **Box-List (DieBox)**, are not yet
clear because documentation for them has not been found. Remaining work
includes completing the setup menu and adding a few utilities, such as log
cleanup.

Most Functions are working now, connect, disconect, chat, file transfer. To do still is some of the legacy GP plots and plays and the as how they would call mail; wish is more like a leave a note.

![Language](https://img.shields.io/badge/language-RUST-dea584.svg) 

## Layout

| Path | Role |
|------|------|
| `src/config.rs` | Typed `Config` + `Config.toml` loader; maps legacy `CONFIG.GP` keys (`MYCALL`, `SERPORT`, `SERBAUD`, `CHANNELS`, colours, `TNCINI`, …) |
| `src/tnc/` | Modem drivers — WA8DED / TFPCX / KISS serial or TCP (one loaded from `[tnc] protocol`) |
| `src/ui/` | Pixel UI (egui). `--tui` is the thinner text UI |
| `docs/LAYOUT.md` | Pane map from V1.63 screenshot + docs |
| `docs/ICONS.md` | Decoded `ICONS.GP` format (70×40 planar VGA, 17 icons) |
| `src/ui/icons.rs` | Runtime decoder for the icon library |
| `data/` | Runtime tree beside the exe (or the crate, while developing) |
| `data/ICONS.GP` | Toolbar icons |
| `data/NAMES.GP` | Station list for Connect, `%N`, and `//Name` |
| `data/*.GPI`, `data/*.GPH` | Connect texts and remote help. `G.GPH` is English |
| `data/*.GPW` | Sysop password. `DB0IZ-9` is `DB0IZ9.GPW` |
| `data/USER` | `//` catalog, read, write, and delete. 7Plus autosave writes here too. Browsing stays inside this folder |
| `data/EXTERNAL` | `//RUN` programs: Python, PHP, Perl, Ruby, Node, Lua, or Java. `FREE` reports memory. 300 seconds of silence closes a run. The command cannot name a file outside this folder |
| `data/mail` | `{CALL}.GPM` for the linked station. Not listed by `//` catalog |
| `data/log/gpng.log` | Shell and debug log. A release build has no console |
| `data/log/<CALL>.GPL` | Station journal when `log = true` or `LOG=ON`. One file per base call, not per SSID |
| `data/log/pads/` | Saved monitor, channel, spy, BBS, and box pads |
| `data/SAVE`, `data/TEMP` | Intended 7Plus join and split folders. Not wired to the channel yet |
| `docs/COMPATIBILITY.md` | `//` remotes + `%` macros recovered from `*.GPH` / docs / EXE strings |
| `src/remote/` | Abbreviation-aware command parser, GPI macros, USER jail, EXTERNAL programs, and mail |
| `docs/TRANSFER.md` | AutoBin `#BIN#` + 7Plus Code-AutoSave (≠ 7-Zip) for DOS GP interop |
| `src/transfer/` | `#BIN#` state machine, CCITT CRC, `go_7+.` / `stop_7+.` sniffer, and `sevenplus_codec` |
| `src/main.rs` | Tokio entry. Default window is the pixel UI; `--tui` selects the text UI |
| `Config.toml` | Modern defaults. An optional `CONFIG.GP` beside it overlays recognised keys |

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

**Graphic Packet** was written by Ulf Saran (DH1DAE). This repository is a new implementation of that terminal for current operating systems. It does not include the DOS binary.

**GP Paint** was written be Stefan Pfeiffer (DL1ELY). While we had no access to any sourse material, the concept and visual examples for Icons and how to Edit was a great aid.

**7Plus** Code-AutoSave follows the file format of 7PLUS 2.2 by Axel Bauda (DG1BBQ). The part names (`picture.p01`, a single part as `picture.7pl`, part 10 as `picture.p0a`) and the `go_7+.` / `stop_7+.` markers are taken from the C sources maintained by HB9XAR:

[https://github.com/hb9xar/7plus](https://github.com/hb9xar/7plus)

The C sources are not compiled into this crate. `transfer::sevenplus_codec` is a Rust port of the 7PLUS 2.2 codec so the same parts can be built and joined on Windows, Linux, and macOS. 7PLUS itself is free to circulate and is not for sale or commercial use. That port stays under those terms, not under the MIT / Apache-2.0 licence below. With **7+** on, a live capture is written to `data/USER`. Joined parts are intended for `data/SAVE`, and a file chosen for send is split in `data/TEMP` (`-sb` is the part size, separate from the AX.25 packet length). Those two folders are not wired to the channel yet.

**AX.25** Encode and decode librarie for rust by Thomas Karpiniec (VK7XT)

## License

MIT OR Apache-2.0. The original GP binaries and docs remain under their historical licence terms in the parent tree. The 7PLUS program credited above stays under its own terms.
