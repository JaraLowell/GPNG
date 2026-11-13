# GPNG vs DOS Graphic Packet — remaining gaps

Compared to classic **Graphic Packet** (Ulf Saran / DH1DAE; manual in
`docs/manual-en.pdf` / `docs/GP.DOC`). Judged on the **pixel UI** (`cargo run`).
`--tui` is thinner and is not tracked here.

This file lists **what is still missing or incomplete**. Features that already
work are not catalogued here — see the code, `docs/COMPATIBILITY.md` (`//` /
macros), and `docs/TRANSFER.md` (AutoBin / 7Plus).

| Word | Meaning |
|------|---------|
| **Partial** | Present, but not full DOS behaviour |
| **Missing** | Not implemented |
| **DOS-only** | MS-DOS / BGI / UART hardware — not a port target |

---

## Still open (priority)

### Transfers and files

| Gap | Status | Notes |
|-----|--------|-------|
| ALT-T send menu (text / raw / AutoBin / editor) | **Missing** | AutoBin and 7+ already run via session bar / `//RPrg` / `//WPrg` / `//Read` |
| Transfer resume after quit | **Missing** | Progress strip is live; resume is not |
| ALT-S continuous channel save | **Missing** | On BBS menu ALT-S sorts; elsewhere unanswered |
| ALT-F send file (channel) | **Missing** | On BBS/Box-List ALT-F finds |
| Marked QSO → editor (Ctrl-K / drag-select) | **Missing** | |
| Print to LPT / DefaultPrint | **Missing** | Keys parsed in legacy config only |
| DieBox `(BIN)` mail via AutoBin on READ | **Partial** | Channel AutoBin works; CHECK `(BIN)` titles are not special-cased |

### Mailbox lists (BBS-Menu / Box-List)

DieBox `CHECK` / `LIST` on the setup reference channel: **done** (mark, READ,
find, sort, replace/append). Still open:

| Gap | Status | Notes |
|-----|--------|-------|
| F6FBB / LinBPQ list rows (`L` / `LM` / …, read by msg#) | **Missing** | Best offline shape: `fbb-7.0.15`; lab: stock LinBPQ + TCP KISS |
| OpenBCM / BayCom CHECK·LIST | **Missing** | Likely successor to DieBox board-mail; capture a live dump before parsing |
| DK5SG, DB0IE | **Missing** | Need sample dumps |
| CHECK rubric section chrome | **Missing** | Manual 6.1 |
| Unknown formats | **Missing** | Same ongoing job as DOS GP |

### Remotes and routing

| Gap | Status | Notes |
|-----|--------|-------|
| `//Activity`, `//RTt`, `//C`, chat `#` | **Missing** | Parsed, then refused |
| `REMOTE=` / `NOREMOTE=` call allow-lists | **Partial** | Loaded; not enforced. Per-channel **Remote** bit is the live gate |
| `NAMES.GP` type `B` blocks remotes | **Missing** | Type stored; remotes still run |
| `PORTx` / multiport radio switch | **Partial** | `%DIGI%` / `PORTx` QRG match in paths; no TNC/radio port change |
| `NoName` salutation | **Partial** | Stored; `%N` falls back to the callsign |
| `*.GPT` system language packs | **Partial** | `//Language` + `*.GPH` help work; GPT strings unused |

### Spy / AutoSpy

| Gap | Status | Notes |
|-----|--------|-------|
| Pair QSO pick from MHeard (“please select QSO”) | **Missing** | Slot watches one call; no pair / N(S) chrome |
| Auto-close spy on DISC / DM / UA | **Missing** | AutoSpy open + `CALL.NNN` files exist |
| `AutoSpyMinimumSave` | **Missing** | |
| `MaxSpy` limits the UI | **Partial** | Stored; pixel UI still offers the configured spy set |

### Editor and compose

| Gap | Status | Notes |
|-----|--------|-------|
| Insert vs overwrite | **Missing** | |
| Hard word-wrap / 80-column save rules | **Partial** | Toggle stored; editor does not hard-wrap |
| Input-columns wrap / auto-send full line | **Partial** | Stored; compose does not enforce |
| Unread-channel menu colour | **Missing** | |

### Hostmode / hardware

| Gap | Status | Notes |
|-----|--------|-------|
| WA8DED / TFPCX on a real TNC | **Partial** | Drivers exist; not proven on hardware |
| Ctrl+PrintScreen TNC-Statistics | **Missing** | Hostmode / soft-TNC only (not KISS) |
| Hostmode beacon TX | **Partial** | Values saved; no invented TNC beacon command |
| `TNCINI` Date/Time → TNC clock | **Partial** | Lines applied; GP password → clock cmds not translated |
| `TNCDOS` while a shell is open | **DOS-only** | ALT-O minimises; no shell / TNC rewrite |
| Resync / V24 error UI | **Missing** | USB-serial replaces the DOS UART path |
| Screensaver / PopUpTime | **Partial** | Stored, unused |
| `BINDATA.GP` / `CSTAT.GPB` | **Missing** | Settings go to `Config.toml` |
| CONFIG.GP `<IF>` / `%TFPCX200` | **Missing** | Flat `KEY = value` overlay only |

### Sysop / DieBox polish

| Gap | Status | Notes |
|-----|--------|-------|
| DieBox / FALCon 60×27 PRIV grid | **Partial** | Recognised; not calculated (PWLIST algorithm known — see Research) |
| DieBox CHECK 50-line paging | **Partial** | First page works; `CHECK 50-` not automated |

### Keyboard leftovers

| Gap | Status |
|-----|--------|
| ALT-Z VGA zoom | **DOS-only** |
| ALT + numpad ASCII 1–31 | **DOS-only** |
| DOS shell / `/e` `/v` `/NOXMS` … | **DOS-only** |

---

## Stub `//` remotes

Everything else in the remote catalogue runs. These four still return
“not implemented” after parse:

- `//Activity`
- `//RTt`
- `//C` (remote connect)
- chat `#` (and `#` tokens inside `REMOTE=`)

Full command / macro table: `docs/COMPATIBILITY.md`.

---

## Recent behaviour (so agents do not re-open closed work)

- **InsertMonLine** — ON inserts a blank monitor row before each new `fm …` header.
- **Session chrome bits** — umlaut, softscroll, echo, bell, 7+, remote are
  per-slot bit masks in `[session.chrome]`. Monitor toggles set every bit that
  setting uses; a channel / spy / BBS / Box-List toggle flips only that slot.
  7+ and remote apply to QSO channels only.
- **7Plus / AutoBin / Transfer80** — live on KISS with DOS GP; see `docs/TRANSFER.md`.
- **DieBox BBS-Menu / Box-List** — live client for `CHECK` / `LIST` on the
  reference channel.

---

## Research — mailbox formats (keep short)

GPNG is a **client** only. Prefer live dumps over guessing.

| Source | Role |
|--------|------|
| `D:\GPDos\MB19` | DieBox 1.9 authority (`CHECK` / `LIST`, PRIV / PWLIST) — **legacy** path already in `bbs.rs` |
| `D:\GPDos\fbb-7.0.15` | F6FBB list row macros — draft FBB/BPQ BBS-Menu parser |
| Stock **LinBPQ** + TCP KISS ↔ GPNG | Best lab for FBB-style `LM`/`LL` fixtures |
| **OpenBCM** (Linux/WSL) | Living board-mail successor; capture `CHECK`/`LIST` before writing a parser |
| `bbs_spec.*` / hierarchical HLOC | S&F / routing docs — **not** list-pane layouts |
| `linbpq-light` | No BBS — do not use for mail lists |

**Direction:** DieBox = compatibility. Prefer OpenBCM for board-mail and
LinBPQ/FBB for bulletin BBS-Menu. DK5SG / DB0IE stay blocked without dumps.

DieBox PRIV (if ever needed): PWLIST 1620 chars (60×27); four chars from
`row = (min+day) mod 60`, `offset = row*27 + hour`.

---

## Suggested next slices

1. F6FBB/BPQ BBS-Menu parser (LinBPQ TCP KISS capture → fixtures).
2. OpenBCM CHECK/LIST capture → parser or confirm DieBox match.
3. ALT-T / ALT-S / ALT-F / transfer resume.
4. Stub remotes (`//Activity`, `//RTt`, `//C`) or drop from the help packs.
5. `REMOTE=` allow-list + type `B` remote block.
6. Hostmode hardware pass + Ctrl+PrintScreen TNC-Statistics.
