# Graphic Packet (New Generation)

Asynchronous Rust foundation for a cross-platform rewrite of **Graphic Packet (GP)**, the classic 1990s MS-DOS AX.25 HAM Packet Radio terminal by Ulf Saran (DH1DAE).

This crate targets modern Windows / macOS / Linux hosts talking to USB-to-serial TNCs (WA8DED hostmode, TFPCX, Kenwood KISS) and soft TNCs over **TCP KISS** (Soundmodem, Direwolf, …).

![afbeelding](https://i.gyazo.com/d8bde765d19db1ee5ba339240dbeb1f7.jpg)

## Development status

This project is still under active development, and some features may not work
as expected. KISS is currently being tested; the WA8DED and TFPCX drivers are
not yet tested on hardware.

Day-to-day settings no longer need a hand-edited config for normal use. On
startup the **Radio setup** card picks callsign, protocol (KISS / WA8DED /
TFPCX), serial port and baud, or TCP host and port for Soundmodem / Direwolf.
**Use this** opens the link and writes those values back. Inside the session,
**ALT-U** / the **SETUP** icon opens Program parameters for the current
window (colours, remote, echo, bells, beacon, language, packet length, and
the other live rows). Session-bar switches and channel MYCALL dialogs also
persist. `Config.toml` is still the on-disk store, and an optional
`CONFIG.GP` overlay is merged on load for migration.

Connect, disconnect, chat, Shift-F macros, MHeard, DieBox BBS-Menu / Box-List,
`//` remotes, and AutoBin file transfer are in. Still ahead are some legacy GP
corners (other mailbox list formats, continuous save / file-send ALT keys,
hostmode TNC-Statistics, and a few transfer resume paths). Mail is closer to
“leave a note” (`*.GPM`) than a full BBS. See `docs/GAPS.md` for the checklist.

![Language](https://img.shields.io/badge/language-RUST-dea584.svg) 

## Layout

| Path | Role |
|------|------|
| `src/config.rs` | Typed `Config` + `Config.toml` persistence; maps legacy `CONFIG.GP` keys. Operators change most values from Radio setup and ALT-U |
| `src/tnc/` | Modem drivers — WA8DED / TFPCX / KISS serial or TCP (chosen on the startup Radio setup card, stored as `[tnc] protocol`) |
| `src/ui/` | Pixel UI (egui): startup Radio setup, ALT-U Program parameters, session chrome. `--tui` is the thinner text UI |
| `docs/LAYOUT.md` | Pane map from V1.63 screenshot + docs |
| `docs/ICONS.md` | Decoded `ICONS.GP` format (70×40 planar VGA, 17 icons) |
| `src/ui/icons.rs` | Runtime decoder for the icon library |
| `data/` | Runtime tree beside the exe (or the crate, while developing) |
| `data/ICONS.GP` | Toolbar icons |
| `data/NAMES.GP` | Station list for Connect, `%N`, and `//Name` |
| `data/*.GPI`, `data/*.GPH` | Connect texts and remote help. `G.GPH` is English. `//Info` prefers `<mycall>.GPI` (your base call); a release ships `INFO.GPI` as the fallback example |
| `data/*.GPW` | Sysop password. `DB0IZ-9` is `DB0IZ9.GPW` |
| `data/USER` | `//` catalog, read, write, and delete. 7Plus autosave writes here too. Browsing stays inside this folder |
| `data/EXTERNAL` | `//RUN` programs: Python, PHP, Perl, Ruby, Node, Lua, or Java. `FREE` reports memory. 300 seconds with no input and no new output closes a run. The command cannot name a file outside this folder |
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
| `Config.toml` | On-disk defaults and what the menus write back. Optional `CONFIG.GP` beside it overlays recognised keys on load |

### GUI strategy (no EGAVGA.BGI)

Classic GP used Borland graphics (EGA 640×350 / VGA 640×480). We recreate the
**documented** operator layout (menu tabs, status, RX/connect, split monitor,
input, icon strip) in a native window. Prefer docs + `CONFIG.GP` colour
regions over reverse-engineering `GP2862K.EXE` drawing code.

## Quick start

```bash
cargo run
# Text fallback:
cargo run -- --tui
```

1. The splash shows **Radio setup**. Set **Call sign**, **Protocol**, then either
   serial **Port** / **Baud to TNC**, or (KISS + TCP) **IP address** / **TCP port**.
2. Click **Use this** — the modem link opens and the choices are saved to
   `Config.toml`.
3. Inside the session, **ALT-U** or **SETUP** opens Program parameters for the
   active window. Colours, remote, echo, bells, beacon (on Monitor), language,
   packet length, and related rows write back when you leave with **Use these
   values**.

You do not need to edit `Config.toml` for ordinary station / radio / session
settings. Hand-editing is still fine for paths, buffer sizes, `TNCINI` lines,
`KISS.INI` sections, and other keys the menus do not expose. An optional
classic `CONFIG.GP` beside `Config.toml` merges recognised keys on load.

```bash
RUST_LOG=debug cargo run
```

## Protocol notes

Only **one** modem driver is constructed when you confirm Radio setup (stored as
`[tnc] protocol`):

| `protocol` | Driver | Notes |
|------------|--------|-------|
| `wa8ded` (aliases `jhost`, `hostmode`) | Timed WA8DED hostmode poll | L2 lives in the TNC; host sends `I`/`C`/`D` hostmode. `JHOST1` → `WA8DED.INI`; exit `[close]`; poll `G0` |
| `tfpcx` | Soft-TNC hostmode | Same hostmode framing; more host-side L2 later (deferred — no BayCom on hand) |
| `kiss` | TAPR KISS | Host builds AX.25 via the [`ax25`](https://crates.io/crates/ax25) crate; `FEND`/`FESC`; serial **or** TCP (`kiss_transport`) |

### KISS transport

Pick **Connection: Serial** or **TCP** on the Radio setup card (same fields as
`kiss_transport` / `[serial]` / `kiss_tcp_*` in `Config.toml`):

| `kiss_transport` | Wire | Typical use |
|------------------|------|-------------|
| `serial` (default) | COM/tty + baud | Kenwood TM-D710 etc. — applies `KISS.INI` (`kiss_section` / `kiss_close_section`), DTR pulse, exit sequence |
| `tcp` | host:port | Soft TNC already in KISS — **UZ7HO Soundmodem** (often port `8100`), **Direwolf** (often `8001`); no `KISS.INI` |

Equivalent hand-edit for a Soundmodem lab (optional — the startup card writes the same):

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

**GP Paint** was written by Stefan Pfeiffer (DL1ELY). While we had no access to any source material, the concept and visual examples for Icons and how to Edit was a great aid.

**7Plus** Code-AutoSave follows the file format of 7PLUS 2.2 by Axel Bauda (DG1BBQ). The part names (`picture.p01`, a single part as `picture.7pl`, part 10 as `picture.p0a`) and the `go_7+.` / `stop_7+.` markers are taken from the C sources maintained by HB9XAR:

[https://github.com/hb9xar/7plus](https://github.com/hb9xar/7plus)

The C sources are not compiled into this crate. `transfer::sevenplus_codec` is a Rust port of the 7PLUS 2.2 codec so the same parts can be built and joined on Windows, Linux, and macOS. 7PLUS itself is free to circulate and is not for sale or commercial use. That port stays under those terms, not under the MIT / Apache-2.0 licence below. With **7+** on, a live capture is written to `data/USER`. Joined parts are intended for `data/SAVE`, and a file chosen for send is split in `data/TEMP` (`-sb` is the part size, separate from the AX.25 packet length). Those two folders are not wired to the channel yet.

**AX.25** encode/decode library for Rust by Thomas Karpiniec (VK7XT)

## License

MIT OR Apache-2.0. The original GP binaries and docs remain under their historical licence terms in the parent tree. The 7PLUS program credited above stays under its own terms.
