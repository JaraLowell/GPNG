# File transfer interoperability (DOS GP ↔ Rust GP)

Classic Graphic Packet does **not** use 7-Zip. Two different mechanisms matter
for binary / coded-file exchange with original GP, SP, THP, DieBox, etc.:

| Mechanism | What travels on AX.25 | GP UI / remote | Peer programs |
|-----------|----------------------|----------------|---------------|
| **AutoBin (`#BIN#`)** | Raw binary octets after a textual handshake | BIN-TX / BIN-RX, `//RPrg`, `//WPrg` | TurboPacket, SP, AHP, DieBox, FBB |
| **7Plus Code-AutoSave** | 7Plus *text* parts (ASCII + special sentinels) | Setup **7+**, remote `//7+ ON\|OFF` | Any station that dumps `.p01`/`.7pl` into the QSO |

Both must work for “old GP talks to new Rust GP”. Encode/decode of 7Plus
*payloads* can shell out to or link [`hb9xar/7plus`](https://github.com/hb9xar/7plus);
GP itself mostly **detects headers and saves streams**.

```text
                    ┌─────────────────────────────┐
   raw .EXE/.GIF ──►│ AutoBin #BIN#  (wire proto) │◄── peer terminal / BBS
                    └─────────────────────────────┘

   binary ──► 7plus.exe ──► .p01/.p02 text ──► QSO as normal I-frames
                         ▲
                         └── GP “7+ AutoSave” snips go_7+ … stop_7+ to disk
```

---

## 1. AutoBin (`#BIN#`) — wire protocol

Documented by DK4NB (1994); GP embeds `#BIN#`, `#BIN_V2#`, `#OK#`, `#NO#`,
`#ABORT#` (confirmed in `GP2862K.EXE` strings). Compatible with SP / THP as
stated in GP’s own help.

### Basic handshake

```text
Sender:    #BIN#<nbytes>\r
Receiver:  #OK#\r          or   #NO#<reason>\r
Sender:    <nbytes raw bytes>     (transparent binary)
(end)      last byte ends the transfer; both sides may print CRC status
Abort:     \r#ABORT#\r
```

Rules from the public spec (must keep for DOS GP):

- Control elements start at the **beginning of an I-frame** and fit in **one** frame.
- The frame *before* a control element ends with `\r`.
- Receiver ignores non-`#OK#` / `#NO#` while waiting to ACK.
- Chat during TX (SP style): frames prefixed with `SP-` are not file data.

### Extended header (filename + CRC + optional resume)

```text
#BIN#<nbytes>#|<filename>#$<ftime_hex>?<crc_dec>\r
                 \_________/   \________/ \______/
                  basename      Borland     CCITT CRC
                                struct      of whole
                                ftime 32b   file (decimal)
```

- `?` present → sender can **resume**; absent → overwrite / no resume.
- Receiver ACK variants:
  - `#OK#\r` or `#OK#<name>\r` — start from byte 0
  - `#OK#<name>#$<frag_len>#<frag_crc>\r` — resume after a partial file  
    (SP fragment naming: e.g. `TEST.LZH` → `TEST.$ZH`)

CRC is the classic CCITT table/`updcrc` macro from the DK4NB bulletin
(same polynomial TurboPacket / SP use). See `src/transfer/crc.rs`.

### GP-specific behaviour to mirror

- With **7+ AutoSave ON**, GP auto-answers extended `#BIN#` (no manual BIN-RX).
- Without extended header, saved name is `<peercall>.NNN` (`000`…`999`).
- DieBox titles may show `(BIN)` for AutoBin mail.
- Remotes: `//RPrg <file>` (send AutoBin), `//WPrg <file>` (receive/store AutoBin).
- `//CUT` aborts AutoBin on that channel. The receiver sends `#ABORT#` (or `#NO#` before the file starts). The sender stops, and sends `#ABORT#` once file bytes are already moving, so both sides return to chat.

`#BIN_V2#` appears in GP 1.63 — treat as a variant magic to detect; prefer
emitting classic `#BIN#` + extended fields for maximum peer compatibility
unless tests show V2 is required.

---

## 2. 7Plus Code-AutoSave — *not* a wire protocol inside GP

**7Plus** (DG1BBQ) is a store-and-forward **binary→text** coder (radix-216),
similar in role to uuencode, used in BBS mail and QSOs. It is **not** 7-Zip.

GP’s job when “7+” is on:

1. Scan RX text for a **start marker** (`go_7+.` in `GP2862K.EXE`).
2. Open/create the part file using the name implied by the 7Plus header
   (`.p01`, `.p02`, …, `.7pl`, also `.err` / `.cor`, and LCPlus).
3. Append following lines until **`stop_7+.`** (and sibling `stop_text` /
   `stop_info` markers also present in the binary).
4. Strip mailbox junk outside the markers so the saved file is clean 7Plus.

Remote `//7+ ON|OFF` toggles the same switch as the Setup “7+” / “Code-AutoSave”
menu item. When ON, AutoBin extended receive is also armed (GP ≥ 1.50).

### Implementing encode/decode

The sniffer only stores text parts. Rebuilding the original file, and
cutting a file into parts before a send, is
`transfer::sevenplus_codec`. That is a Rust port of 7PLUS 2.2. The
MS-DOS `7PLUS.EXE` is not invoked, so the same code runs on Windows,
Linux, and macOS. The port stays under the 7PLUS terms (no commercial
use, no sale, circulate freely). It is not MIT / Apache-2.0. Error
reports (`.err` / `.cor`) are not ported; the manual part
`//Read` / `//7DECODE` flow does not need them.

**Receive.** Parts are saved in `UserDir` (`data/USER`) — the same folder
`//Read` uses, as classic GP expects. The header says which part this is
and how many exist (`001 of 003`). When every part of that name is in
USER (autosave or `//7DECODE`), `decode_parts` writes the original file
(or `name (2).ext` if taken) and deletes the parts.

**Send.** `//Read` of a normal USER file runs `encode_parts` (`-sb` 3000)
and writes `.p01` / `.7pl` beside the original in USER, then reports
parts ready. The peer pulls each part with another `//Read`. `part_bytes`
is 7plus `-sb`: a part ends once at least that many source bytes are in
it, in steps of 62. A separate TEMP folder is not used. An `ok`/`okay`
gated auto-send of successive parts is not wired; the peer chooses when
to `//Read` the next part. Each part on the air is still cut into AX.25
frames of `packet_payload` bytes. `-sb` is the size of one part, not that
frame size.

Interoperability test matrix:

| Direction | Method | Success criteria |
|-----------|--------|------------------|
| DOS GP → Rust | AutoBin BIN-TX | Rust saves byte-identical file, CRC OK |
| Rust → DOS GP | AutoBin | DOS BIN-RX / 7+ autosave accepts, CRC OK |
| DOS GP → Rust | 7Plus parts in QSO | Rust autosave writes `.p01`… joinable by 7plus |
| Rust → DOS GP | send `.p01` text | DOS 7+ autosave captures; 7plus decodes |

---

## 3. Mapping to remotes / UI

| Operator action | Wire |
|-----------------|------|
| ALT-T AutoBin send / BIN-TX | `#BIN#…` then data |
| BIN-RX / auto with 7+ on | `#OK#` then sink N bytes |
| `//RPrg file` | peer pulls via AutoBin TX from our UserDir |
| `//WPrg file` | we AutoBin-RX into UserDir as `file` |
| `//7+ ON` | enable header sniffer + AutoBin auto-ACK |
| Save online | raw text capture (also works for already-encoded 7Plus) |

---

## 4. Implementation map in this crate

| Module | Role |
|--------|------|
| `transfer::crc` | CCITT CRC used by AutoBin |
| `transfer::autobin` | Parse/build `#BIN#` / `#OK#` / `#ABORT#`; RX/TX state machine |
| `transfer::sevenplus` | Detect `go_7+.` / `stop_7+.`; autosave state |
| `remote` `SevenPlusAutoSave` | Already wired to `//7+` |

7Plus Code-AutoSave is wired: with **7+** on, channel RX between `go_7+.`
and `stop_7+.` (also `go_text.` / `go_info.`) is written into `UserDir`
(`data/USER`). The part name follows hb9xar/7plus `extract.c`
(`picture.p01`, single part `picture.7pl`, part 10 `picture.p0a`).
`//Read` of a normal file encodes into USER parts; a part file is sent as
text. `//7+ ON|OFF` is the same switch as the session button. `//7DECODE`
joins a set by hand. AutoBin (`//RPrg` / `//WPrg`, BIN-TX/RX) is on the air.

---

## 5. Progress window (DOS GP 1.50+)

A sized send does not open a separate dialog. It expands the purple
status strip **above the channel pad** (about three transfer lines on
top of the usual channel/buffers rows) and leaves the QSO text in place.
Best visual reference: `docs/screenshots/Transfer80.png` (83%). Also
`Transfer30.png` at 31% and `SplitScreen.png` at 57%.

The grid becomes four rows:

| Row | Left | Right |
|-----|------|-------|
| 1 | `Channel N: CALL`, then `Name not saved` | `Information Transfer` |
| 2 | `My:CALL` and the usual Buffers / Not disp / Not sent / Tries / Unack | `GP V1.63` |
| 3 | Full path (`C:\GP\ALAS.DOC`) and `Bytes: 2048/6558` | `Status: BIN-TX` |
| 4 | Green block bar, percent in a small box at the bar's end (`31%`, `83%`) | `Elapsed Time: 00:00:05`, `Eff. Baud: 2662`, and under those `Estimated Time: 00:00:20` |

Bytes are sent/total. The percent is the bar label, not a second copy of
the byte counts. Eff. Baud is the effective rate; it moved from 2662 at
31% to 1914 at 83% on the same file, and the estimate moved with it
(`00:00:20` then `00:00:27`). Times are `hh:mm:ss`. Estimated Time is
the **total** projected duration (`total × elapsed / got`), not the
time remaining — at 83% / 23s the estimate reads `00:00:27`.

The bar and the estimate need a known total (AutoBin, 7Plus AutoSave).
A plain text capture has no total, so no bar. The channel pad keeps
showing traffic under this grid. In the ALAS.DOC shots that traffic is
the file itself; in `SplitScreen.png` the top pane is still the QSO
while `C:\GP\FREQ.GPI` is the file named in the grid.
