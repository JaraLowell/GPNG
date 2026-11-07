# Layout (GP V1.63 screenshots)

Reference PNGs live in `docs/screenshots/`.

## Pane map (pixel UI)

```text
┌── purple pre-write (history ~10 + 3-line compose) ──┬─ analog clock ─┐
├── channel buttons (2×6) ────────────────────────────┤  tall column   │
│ 1…10 / BBS-Menu / Box-List                          │  spans all 3   │
├── magenta status grid (same height as channels) ────┤                │
│ ┌──────────┬─────────────────┬──────────────┐       │                │
│ │ Monitor  │ Monitor Mode: … │ Buffers: n   │ row1  │                │
│ │ (2 high) ├─────────────────┴──────────────┤       │                │
│ │          │ Packets not yet displayed: 0   │ Mycall│                │
│ └──────────┴────────────────────────────────┘ row2  │                │
├─────────────────────────────────────────────────────┴────────────────┤
├─ full-width content (Monitor blue / Connect grey / …) ───────────────┤
├─ session strip (clickable cells) ────────────────────────────────────┤
│ [QRG] [CT] [äöü] [/|≣] [Echo] [♪]                                    │
│   Monitor: [1]…[10] spy slots                                        │
│   Channel: [7+] [Remote] [BIN-RX] [BIN-TX]                           │
│ [ScrLock] [▲] [▼] [B]                                                │
└─ ICONS.GP strip, evenly spaced; set depends on active pane ──────────┘
```

Session strip behaviour (see `src/ui/session.rs`):

- **QRG** — dialog to set MHz (default 27.235)
- **CT** — left-click edits the connect-text file on the button (default `CTEXT.GPI`); right-click picks another (e.g. `CTEXT_A.GPI`)
- **äöü** — plain vs extended DOS ASCII in the RX pane
- **/** ↔ **≣** — hard line step vs smooth scroll feel
- **Echo** — mirror TX into Monitor
- **♪** — play `assets/bell.wav` on BEL (`0x07`) when ON
- **ScrLock** — stick to end on new traffic; OFF keeps viewport (use ▲▼)
- **B** — show/hide RX scrollbar


The magenta block is **not** a single label line. It is a non-clickable cell
grid with the same visual weight as the channel buttons. The left title cell
(`Monitor` / `Channel n` / …) spans both rows.

`Buffers:` comes from config `text_buffer`. `Monitor Mode:` defaults to `IUSC`;
set with `M IUSC` (sent to TNC) and logged on the Monitor pad.
