# Graphic Packet (New Generation)

Asynchronous Rust foundation for a cross-platform rewrite of **Graphic Packet (GP)**, the classic 1990s MS-DOS AX.25 HAM Packet Radio terminal by Ulf Saran (DH1DAE).

This crate targets modern Windows / macOS / Linux hosts talking to USB-to-serial TNCs (WA8DED hostmode, TFPCX, Kenwood KISS) and soft TNCs over **TCP KISS** (Soundmodem, Direwolf, …).

![afbeelding](https://i.gyazo.com/d8bde765d19db1ee5ba339240dbeb1f7.jpg)

## Development status

This project is still under active development, and some features may not work
as expected. KISS is currently being tested; the WA8DED and TFPCX drivers are
not yet tested on hardware.

Day-to-day settings no longer need a hand-edited config for normal use. On
startup the **Radio setup** card picks callsign, protocol (**KISS** /
**TAPR TNC-2 WA8DED** / **Baycom** / **TFPCX**), serial port and baud, or TCP
host and port for Soundmodem / Direwolf. Baycom can be selected and saved, but
its hardware connection is explicitly unavailable and Apply is disabled.
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
| `src/tnc/` | Modem drivers — WA8DED / TFPCX / KISS serial or TCP / experimental FT232R BayCom (selected in Radio setup, stored as `[tnc] protocol`) |
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

### Station names and routes

Click **Name** on a channel's session bar (or press **Alt-N**) to edit
`data/NAMES.GP`. The button shows **Name: {known name}** while connected to a
named station; otherwise it stays grey and clickable, including while
disconnected. Select an existing station or **New station**, then enter CALL,
NAME, VIA, station type, and optionally IDENT and a QRG restriction.
**Remove selected station** deletes only the selected saved entry, leaving
the same CALL's entries on other QRGs intact. The editor's dropdown fields
and options use compact, square 3D bevels and single-line labels. Long labels
stay on one line and show their full text on hover if they do not fit.

VIA accepts plain digipeater calls separated by spaces or commas, or the
original typed paths such as `N>DB0ABC D>DB0XYZ T>N0CALL`. Typed paths retain
node connections, recursive routing, and `%DIGI%`. The editor preserves
unrelated lines, comments, PORT settings, DOS CP437 encoding and line endings.
Names saved by the editor are limited to 22 characters.

Choose **All QRGs**, a band preset (including **11 m - 27 MHz**),
**Current QRG**, or enter a
custom integer MHz number. IF conditions match only the number before the dot:
`<IF 145>` and legacy `<IF 145.500>` both match `145.825`, but not `144.800`.
This is an integer-MHz restriction, not the entire amateur allocation (for
example, 144 and 145 are separate choices within 2 m). Existing ELSE entries
remain editable and keep their exclusion unless their scope is changed.
Saving groups stations with the same integer QRG into one IF block, reusing
an existing block and merging older duplicate blocks. ELSE entries stay in
the ELSE branch of that shared block.
PORT / `%DIGI%` frequency matching remains exact.

In **Connect → click for NAMES.GP**, **All QRGs** is listed first, followed
by the active QRG's section (for example **27MHz List**). Entries for other
QRGs are hidden; applicable ELSE entries appear in the active section.
Selecting a station uses that exact entry's VIA path, including digipeaters
and queued node connections. A route error is reported instead of connecting
directly without its required VIA.

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
4. Press **ALT-A** to send the configured beacon immediately in KISS or Baycom mode,
   even when its period is zero. This does not reset the automatic timer.
   The DOS icon in the pixel UI sends the beacon too.
5. Press **ALT-S** to use the current window's **SAVE** action, including
   accepting a pending AutoBin offer on a channel.

The GPI editor saves CP437 where possible; text containing other characters,
such as emoji, is saved as UTF-8 with a BOM. UTF-8 GPI files may not be
compatible with classic DOS GP.

The pinned local winit backend preserves Windows Alt+numpad character input;
other platforms continue to use their native input methods.

In channel or Monitor input, Alt+27's `←` before a complete ANSI color
sequence is sent as ESC. For example, `←[1;31mNot!←[0m` sends bright red
text and then resets the color. Other arrows remain ordinary text. Channel
and Monitor ANSI color is controlled by `[session] allow_color_ansi` in
`Config.toml`. Monitor colors are interpreted independently for each frame.

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
| `kiss` | TAPR KISS | Host builds AX.25 via the [`ax25`](https://crates.io/crates/ax25) crate; `FEND`/`FESC`; serial **or** TCP (`kiss_transport`) |
| `wa8ded` (aliases `jhost`, `hostmode`) | TAPR TNC-2 WA8DED (NORD><LINK TheFirmware) | L2 lives in the TNC; typed command/data requests, extended `G` polling with classic fallback; `WA8DED.INI` open/status/close sections |
| `baycom` | Baycom | Experimental FT232R direct USB, 1200 bit/s ser12; shared software AX.25 |
| `tfpcx` | TFPCX | Legacy hostmode path, retained for later review; not a Kantronics KPC driver |

The **WA8DED TNC2** driver targets TNC2 modems running NORD><LINK/TheFirmware.
The existing `wa8ded`, `jhost`, and `hostmode` configuration names remain valid.
TNC3-specific behavior is outside this target; add a separate driver if TNC3
support is requested rather than mixing its extensions into the TNC2 driver.

The implementation follows the supplied TF2.7/TNC2 source and TF2.6 command
reference, checked against serial captures and the earlier Python implementation:

- Startup expects **terminal mode**, sends the escape/JHOST1 sequence, and
  requires a binary version reply before initialization. The GUI becomes ready
  only after successful setup; wrong baud, rejected setup, and missing replies
  are errors. After a crash that leaves the TNC in hostmode, restart the modem
  into terminal mode before reconnecting; existing RF sessions are not resumed.
- Host requests are `channel | command/data | length-minus-one | payload`.
  Command is `01`, data is `00`, and payloads contain 1..256 bytes. Success
  replies are two bytes; response types 1..5 are NUL-terminated, while 6/7 carry
  length-minus-one binary data. Serial read boundaries do not delimit replies.
- Only one request is outstanding. An ambiguous timeout stops the driver
  without replaying a possibly accepted data block. Firmware rejection is
  reported, not silently counted as delivery; rejected identity/range setup
  stops the driver before queued connections can use incorrect settings.
- Extended `FF 01 00 47` polling discovers pending logical channels, including
  monitor channel 0. Firmware rejecting extended polling falls back to `G`
  on channels 0..10. `@B` and per-channel `L` counters are queried separately;
  `@B` is the firmware's buffer count, **not free bytes of RAM**.
- Channel numbers are binary: logical channel 10 is `0A`, advertised as `0B`
  in an extended pending-channel list. The earlier field report requiring `10`
  is unresolved; this driver follows the supplied firmware, without a guessed
  remapping. Physical-radio selection is limited to port 0.
- Incoming connects are disabled during setup, then enabled by the configured
  logical-channel range after channel identities are set. Identity changes
  temporarily disable incoming connects too. Per-channel callsigns are restored after global
  `I`/`F`/`N`/`O` commands reset idle firmware link blocks. `[open]` disables
  firmware connect text (`U0`) because the application supplies it.
- Close runs `[close]` and acknowledges `JHOST0` last. If no INI exists,
  `init[]`/`deinit[]` are used; an existing INI must contain `[open]` and `[close]`.
  Missing optional `[status]` uses `F`, `@T2`, and `@T3`.

Automated tests exercise the real driver and shared session service against a
fragmenting simulated TNC: simultaneous channels 1 and 10, connect/disconnect,
binary RX, 256-byte TX, monitor header/body, counters, classic polling fallback,
and rejected initialization/identity setup. This is **not physical-modem or
on-air interoperability certification**. Hostmode RF delivery receipts for file
transfers and structured unproto/beacon requests remain unavailable; TFPCX has
not received equivalent driver-level validation.

### BayCom (experimental FT232R USB backend)

BayCom is separate from the existing TFPCX hostmode driver. The in-project
Rust core in `src/tnc/baycom.rs` has no Linux kernel-driver dependency. Linux
v6.6's [BayCom hardware driver](https://github.com/torvalds/linux/blob/v6.6/drivers/net/hamradio/baycom_ser_hdx.c)
and [HDLC driver](https://github.com/torvalds/linux/blob/v6.6/drivers/net/hamradio/hdlcdrv.c)
serve as behavioral references only:

- Streaming HDLC flags, bit stuffing, AX.25 FCS, and NRZI helpers.
- Bounded RX and TX buffering, with explicit corruption/overflow/full errors.
- One FIFO per physical radio shared by all logical channels, carrier sensing,
  slotted p-persistence, flag preamble/tail, PTT confirmation and RX turnaround.
- Explicit physical-transmit completion, distinct from an RF acknowledgement;
  failed active frames are reported and not automatically replayed.

The backend uses FTDI D2XX hardware-clocked GPIO, CTS clock recovery, and
buffered NRZI transmission. Select **Baycom**, the exact serial port or
`FTDI:<USB-serial-number>` device, and **1200 bit/s**. One physical radio
(port 0) supports multiple logical AX.25 connections through the same session
engine as KISS. Incoming valid frames and completed outgoing bursts reach the
normal monitor; local completion is not an RF acknowledgement.

Only full-handshake **FT232R/FT232RL RS232** adapters are targeted. PL2303,
TTL-only cables, three-wire adapters and 9600-bit/s BayCom hardware are not
supported. Generic COM-byte I/O cannot provide this interface.

Read [BayCom hardware, platform-driver setup and safe field tests](docs/BAYCOM.md)
before connecting a transmitter. The D2XX dependency targets Windows, macOS
and Linux, but validation so far is Windows x64 compilation and simulation,
**not physical-modem or on-air testing on any OS**. USB sampling gaps are not
fully observable; wiring, polarity, power, timing and driver ownership still
need bench verification. TFPCX remains separate and unchanged.

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
| Channel linked | I (+ RR on RX); idle **RR poll every 5 min**, T1 from Frack (default 5 s), 10 retries | `MYCALL` ↔ peer |
| Channel hangup | DISC (+ UA on peer DISC) | |

### Sessions, radio ports, and channels

The GUI sends channel-level connect, disconnect, and data requests to a shared
session service. For KISS, that service owns AX.25 modulo-8 sequencing, queued
data, acknowledgements, busy/retry recovery, and per-channel counters. The KISS
driver only transports frames. WA8DED/TFPCX adapters expose the same session
events while leaving RF AX.25 control inside the TNC; host-command acceptance
is **not** treated as an RF acknowledgement.

KISS receive frames are sent to the monitor independently of session matching,
including traffic for other stations. Each successful data-frame write to the
modem produces a TX monitor entry, including control frames and retransmissions.
This confirms a modem write, not an over-the-air transmission. The GUI's `M`
filter still applies to decoded frames; `M IUSC` lists all supported classes,
also while connected. Undecodable AX.25 packets appear as raw hex diagnostics
with their port, and malformed KISS escapes produce warnings rather than
silently disappearing. The terminal UI also lists RX and TX on its monitor,
not on pads selected by radio port. Monitor history remains bounded.

A KISS circuit is the exact **radio port + local callsign/SSID + remote
callsign/SSID**. Terminal channel numbers are separate. A reply from another
SSID is not a redirect; unmatched traffic cannot update another link's state.
Different peers can share our callsign, and the same pair can use different
radio ports independently. An additional outgoing circuit on the same port
may select an unused local source SSID, but never changes the remote endpoint.

Configure radio ports in `[tnc]` and restart the radio connection:

```toml
kiss_port = 0             # Monitor, beacons, and parameter changes
channel_ports = [0, 1, 1] # Channels 1, 2, 3; missing entries use kiss_port
```

KISS ports are 0..15. Replies use the receiving circuit's port. WA8DED/TFPCX
currently require port 0: their multi-radio selection has not been verified.
KISS uses a one-frame TX window, immediate acknowledgements, and synthetic
buffer accounting, not a measurement of modem RAM. Local self-connections
between configured endpoints are simulated inside the session service and
do not transmit invented acknowledgements over RF.

Automated tests cover link and port isolation, sequence wrap, queued data,
RR/RNR/REJ, polls, resets, disconnects, KISS framing, and monitor delivery.
A loopback TCP test covers startup RX bursts and successful TX notifications;
it does not require a physical modem. This is not hardware
certification or full AX.25 v2.2 support (extended mode/XID/SREJ are not
implemented). WA8DED and TFPCX still need hardware interoperability testing;
RF delivery receipts for their file-transfer adapters are explicitly unavailable.

- **JHOST1** — enter WA8DED binary hostmode from terminal mode  
- **JHOST0** — leave hostmode (WA8DED path)  
- Hostmode requests: `channel | command/data | length-minus-one | payload`;
  replies use response-specific framing as described above.
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
