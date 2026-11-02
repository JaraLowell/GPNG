# `ICONS.GP` format (GP ≥ 1.50)

Recovered from the on-disk library (not by decompiling drawing code).
`GPPAINT.DOC` confirms the post-1.50 format; structure verified against
`ICONS.GP` / `ICONS\ICONS-*.GP` (all 25324 bytes).

## File layout

```text
0x0000  ASCII banner, CRLF-terminated:
        "Graphic Packet  Icon Libary\r\n"
        "(C) Ulf Saran, DH1DAE  1990-93\r\n\r\n"
0x003F  0x1A (DOS EOF)
0x0040  spaces padding through 0x007F
0x0080  u16 LE  — library checksum / fingerprint (e.g. 0xDFB7)
0x0082  icon[0] …
```

## Per-icon record

| Field | Size | Value |
|-------|------|--------|
| width | u16 LE | **70** |
| height | u16 LE | **40** |
| pixels | 1440 bytes | VGA 16-color, **4 bitplanes**, **row-interleaved** |
| padding | 38 bytes | unused / alignment → record **1482** bytes |

- Icon count in stock files: **17** (`0x82 + 17×1482 = 25324`).
- Bytes per plane-row: `ceil(70/8) = 9`.
- For each row `y`: planes 0..3 stored back-to-back (`9×4` bytes), then next row.
- Pixel colour = EGA/VGA index with **file plane0 = MSB (bit 3)**:
  `bit3*plane0 + bit2*plane1 + bit1*plane2 + bit0*plane3`.
  (LSB-first decode swaps LightGray↔Yellow vs GP / GPPAINT / DOSBox.)

Row-interleaved planar packing (4 plane strips per row), not nibble-packed.

## Decode

See `src/ui/icons.rs`. Canonical library: **`tools/ICONS.GP`**.

## Screenshot mapping (toolbar)

`ICONS.GP` slot labels (`ICON_SLOT_LABELS` in `src/ui/icons.rs`):

| idx | label | idx | label |
|----:|-------|----:|-------|
| 0 | CONNECT | 9 | SAVE |
| 1 | DISCONNECT | 10 | SETUP |
| 2 | DOS | 11 | FILETRANS |
| 3 | EDIT | 12 | MHEARD |
| 4 | EXIT | 13 | SORT |
| 5 | LOAD | 14 | SPY |
| 6 | MONITOR | 15 | KILL SPY |
| 7 | MYCALL | 16 | FIND |
| 8 | EMPTY | | |

Per-screen strips (`toolbar_for` in `icons.rs`):

- Monitor: MHEARD EDIT SETUP MONITOR DOS SAVE MYCALL EXIT
- Channel: CONNECT EDIT SETUP MONITOR DOS SAVE MYCALL EXIT
- MHeard: MHEARD EDIT SETUP MONITOR DOS SPY EMPTY EXIT
- Editor: MHEARD EDIT SETUP MONITOR DOS SAVE LOAD EXIT
- BBS-Menu: MHEARD EDIT SETUP MONITOR DOS SORT FIND EXIT
- Box-List: MHEARD EDIT SETUP MONITOR DOS EMPTY FIND EXIT

Channel tabs `1:`…`10:`, `BBS-Menu`, `Box-List` are **drawn chrome**, not
`ICONS.GP` bitmaps. The analog clock is BGI vector drawing, also not an icon.

## Modern use

1. Decode → RGBA at runtime (MSB-first planes; icons already use LightGray paper).
2. Pixel UI blits 70×40 (scaled) onto a grey bevelled cell on the dark-red strip.
3. ratatui can only approximate labels until a bitmap backend exists.
