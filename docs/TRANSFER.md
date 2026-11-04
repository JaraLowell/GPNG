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
- `//CUT` / `//END` do **not** abort AutoBin (help text); use `#ABORT#` / BIN toggle.

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

Do **not** reinvent radix-216 in v0.1 unless needed:

- Ship or path-configure the classic `7PLUS.EXE` / modern `7plus` port for
  encode/decode/repair (`.cor`).
- Rust side: stream sniffer + autosave + optional “encode file then `//Read` /
  plain send” helper.

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
`//Read name` sends a file from that folder; `//7+ ON|OFF` is the same
switch as the session button. AutoBin is still not on the air.

---

## 5. Progress window (DOS GP 1.50+)

During a send or a sized save, DOS GP **changes the screen** and opens a
status window. The 1.61 manual (section 5.2 and 5.3) lists what it shows:

| Field | Meaning |
|-------|---------|
| File name | Part or binary being moved |
| Size | Total length, when known |
| Sent / saved | Bytes already moved |
| Bar | Graphic percent of the total |
| Speed | Effective rate |
| Elapsed | Time so far |
| Estimate | Rough time left. The manual says this swings with the path |

The bar and the estimate exist only when the total size is known. That is
AutoBin and 7Plus AutoSave. A plain text capture has no total, so no bar.
Starting ALT-S also changes the menu for as long as the save is on.

There is no screenshot of this window in `docs/screenshots/`. The exact
layout stays open until a transfer is run on the original DOS GP and the
screen is captured. Do not invent the chrome before that.
