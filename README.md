# Graphic Packet (modern)

Asynchronous Rust foundation for a cross-platform rewrite of **Graphic Packet (GP)**, the classic 1990s MS-DOS AX.25 terminal by Ulf Saran (DH1DAE).

This crate targets modern Windows / macOS / Linux hosts talking to USB-to-serial TNCs (WA8DED hostmode, TFPCX, and related soft-TNCs).

## Layout

| Path | Role |
|------|------|
| `src/config.rs` | Typed `Config` + `Config.toml` loader; maps legacy `CONFIG.GP` keys (`MYCALL`, `SERPORT`, `SERBAUD`, `CHANNELS`, colours, `TNCINI`, …) |
| `src/tnc/` | Modem drivers — WA8DED / TFPCX / KISS (one loaded from `[tnc] protocol`) |
| `src/ui/` | Clean-room TUI (`ratatui`) recreating classic panes — **not** a BGI decompile |
| `docs/LAYOUT.md` | Pane map from V1.63 screenshot + docs |
| `docs/ICONS.md` | Decoded `ICONS.GP` (70×40 planar VGA, 17 icons) |
| `src/ui/icons.rs` | Runtime decoder for the icon library |
| `assets/icons/export/` | PNG previews of all toolbar icons |
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
cd gp-rs
# Edit Config.toml — set [station].mycall and [serial].port
# Pixel UI (ICONS.GP bitmaps + analog clock) — run beside the GP tree so ICONS.GP resolves:
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
| `wa8ded` (aliases `jhost`, `hostmode`) | Timed WA8DED hostmode poll | `JHOST1` / `JHOST0`; poll `G0`; frames from `DOC/WA8DED Page1.txt` |
| `tfpcx` | Soft-TNC hostmode | Same `channel\|code\|len\|payload` framing; no hard `JHOST0` on exit |
| `kiss` | TAPR KISS | `FEND`/`FESC`; applies `KISS.INI` section then listens; port → channel |

- **JHOST1** — enter WA8DED binary hostmode from terminal mode  
- **JHOST0** — leave hostmode (WA8DED path)  
- Hostmode frame shape: `channel \| code \| len \| payload[len]`  
- `[serial] fast_poll` (`on` / `off` / `auto`) sets the WA8DED/TFPCX poll interval

## License

MIT OR Apache-2.0. The original GP binaries and docs remain under their historical licence terms in the parent tree.
