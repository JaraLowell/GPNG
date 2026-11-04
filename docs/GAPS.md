# DOS Graphic Packet vs GPNG — what is still missing

Compared against `docs/manual-en.pdf` (Czech HTML manual for **Graphic Packet 1.61**,
OK1CFP translation of the Ulf Saran / DH1DAE DOS program). The running program
judged here is the pixel UI (`cargo run`). The `--tui` fallback is thinner and
is called out only where it differs.

Status words:

| Word | Meaning |
|------|---------|
| **Have** | An operator can use it the way the manual describes |
| **Partial** | The control, file, or library exists, but the DOS behaviour is not there |
| **Missing** | Not implemented |
| **DOS-only** | Tied to MS-DOS hardware or the Borland graphics runtime; not a feature to port |

GPNG already speaks KISS (serial and TCP) and has WA8DED / TFPCX hostmode
drivers. KISS is the path under test. Hostmode drivers exist and are not yet
proven against a real TNC.

---

## Biggest holes

These are the DOS features an operator would notice first.

1. **BBS-Menu and Box-List do not understand mailbox lists.** The panes exist
   and keep a text log. They do not parse DieBox / F6FBB / DK5SG / DB0IE
   `CHECK` or `LIST` output, mark lines, or send `READ`. **SORT** and **FIND**
   icons on those toolbars do nothing.
2. **No autorouter and no `NAMES.GP`.** Connect is a call (and optional via
   list) typed by hand. There is no `TYPE>CALL NAME; path`, no recursive
   paths, no idents, no `PORT0`…`PORT8`, no QRG-dependent `<IF>` blocks, and
   no `%DIGI%` substitution while building a path. ALT-N “add the station I
   am talking to” does not exist.
3. **File transfer is not on the air.** `src/transfer/` has an AutoBin
   `#BIN#` state machine and a 7Plus `go_7+.` / `stop_7+.` sniffer, with
   tests. The QSO path never calls them. **BIN-RX**, **BIN-TX**, and **7+**
   on the session bar only flip a flag. The **FILETRANS** icon does nothing.
   There is no send dialog (text / raw binary / AutoBin / editor), no
   progress window, and no resume after quit.
4. **Most `//` remotes are recognised and then refused.** The parser covers
   the classic set. Only quit, disconnect, version, echo, bell, MHeard,
   `//Name`, and a GPI text file (`INFO`, `NEWS`, `QRT`, `RUN`, other
   `*.GPI` stems) actually run. Catalog, read, write, delete, help, run,
   parameters, RTT, AutoBin `//RPrg` / `//WPrg`, cross-channel `//n`, and
   `//C` answer “not implemented”. `REMOTE=` / `NOREMOTE=` are loaded and
   not enforced. A mailbox marked type `B` in `NAMES.GP` is not auto-blocked,
   because that file is not read.
5. **No station journal, no sysop passwords, no beacon.** `LOG=` and `UTC=`
   are ignored on purpose. There is no `<mycall>.GPL` log, no `*.GPW`
   password file, no Ctrl-B paste into the compose window, and no beacon
   menu (text, destination, via, period).
6. **Setup (ALT-U) is not a menu.** Colours, prompt text, line length, packet
   length, RX-click mode, backup, and the beacon live in `Config.toml` or
   are unused. The session bar covers only a subset (QRG, connect-text file,
   extended ASCII, line step, echo, bell, 7+, remote, BIN flags, scroll
   lock). Settings are not saved as `CSTAT.GPB`.
7. **Classic hotkeys are missing on the pixel UI.** Channels, icons, and the
   session bar are mouse-driven. F1–F12, ALT-C/D/S/T/Y/N/U/H, Shift-F1–F12
   (`F1.GPI`…`F12.GPI`), Ctrl-D/T/Y/C/P/O, and Ctrl-Enter are not bound.
   The text UI binds a few of them (F-keys, ALT-M/E/X, ALT-F1/F2, split
   ALT-F8/F9/F10) and still stubs setup, save, and ESC TNC commands.

---

## By manual chapter

### 1–2 Installation and multi-call texts

| DOS feature | Status | Notes |
|-------------|--------|-------|
| Install to a directory, `CONFIG.GP` | **Partial** | `Config.toml`, with an optional `CONFIG.GP` overlay for recognised keys |
| `UserDir`, `ExternalDir`, `SaveDir`, `LogDir`, `SpyDir` | **Partial** | Paths are created and remembered. Remote file commands, external programs, continuous save, the journal, and spy-to-disk do not use them yet |
| Per-call `CTEXT???.GPI`, `<call>.GPC`, `<call>.GPM`, `<mycall>.GPI`, `QRT.GPI` | **Partial** | One connect-text file (session **CT**, default `GTEXT.GPI`) and `QRT.GPI` on `//Quit`. No per-call `.GPC` / `.GPM`, and no “try MYCALL file, then shared file” search |
| `GP.EXE` vs `GP286.EXE`, overlay, EMS/XMS | **DOS-only** | One native binary. QSO spy is included (it was the GP286-only extra) |

### 3 Configuration and data files

| DOS feature | Status | Notes |
|-------------|--------|-------|
| VGA colour names for every window | **Partial** | Loaded from config and applied. No live colour picker (manual 11.1) |
| Scroll-back sizes (`TextBuffer`, `EditBuffer`, `CheckBuffer`, `ListBuffer`, `SpyBuffer`, `MHBuffer`, `InputBuffer`) | **Partial** | Used as line caps. Not the DOS 40–780 × 84-byte segment model |
| `Screensave`, `PopUpTime`, `Time`, `Zone`, `ConnectBell` | **Partial** | `Zone` feeds `%Z`. `Time` offset is stored. Screensaver, popup lifetime, and connect-bell repeat count are stored and not acted on. Connect / disconnect WAVs play once |
| Serial `SerNr` / `SerBaud` | **Have** | Modern port name + baud. Raw `SerPort` / `SerIRQ` are stored and unused (the OS owns the UART) |
| `FastPol` | **Partial** | Sets the hostmode poll interval. No 16550-vs-slow auto detection |
| `TNCINI` / `TNCDEI` | **Partial** | Applied on the WA8DED and TFPCX drivers (`WA8DED.INI` first on WA8DED). KISS uses `KISS.INI`, not these lines. `Date`/`Time` as a GP password that becomes TNC clock commands is not translated |
| `TNCDOS` | **Missing** | No DOS shell, so nothing rewrites the TNC while a shell is open |
| `DefaultPrint` / `InverzPrint` | **Missing** | Parsed only |
| `Channels`, per-channel `MYCALL` | **Have** | Up to 10. Empty slots follow the monitor call. Pixel UI can edit the call for the active pane |
| `NoName`, `InsertMonLine`, `RemoteLanguage` | **Partial** | Stored. `%N` falls back to the callsign, not the `NoName` salutation. Monitor blank lines and `*.GPT` language packs are not applied |
| `REMOTE` / `NOREMOTE` | **Partial** | Stored. The session **Remote** switch is a single on/off for the channel, not the allow-list |
| `Questions`, `PopEnableKeyboard`, `MultiPort` | **Missing** | Parsed only. No yes/no/cancel prompts. No DRSI / TFPCX multiport |
| `<IF>` / `<ELSE>` / `<END>` / `%TFPCX200` in `CONFIG.GP` | **Missing** | Legacy overlay is flat `KEY = value` |
| `*.GPI` and the fifteen `%` macros | **Have** | `%V %C %N %Y %K %T %D %B %I %Z %_ %% %O %? %>`, plus width `#nn`. `%DIGI%` expands from context but nothing fills a real digi path |
| `CTEXT???.GPI`, `NEWS.GPI`, `QRT.GPI`, `RUN.GPI`, `ORIGIN.GPI`, `F1.GPI`–`F12.GPI` | **Partial** | Files can sit in `data/` and be served by name. Shift-F1…F12 does not send them. `RUN.GPI` is sent as text; it does not launch a program |
| `<call>.GPC` / `<call>.GPM` | **Missing** | |
| `MHEARD.GP` | **Partial** | JSON list, up to 500 UI frames, names via `//Name` (hidden in the pane, used by `%N`). Not the DOS 40-line MH file, and not a link into `NAMES.GP` |
| `NAMES.GP` | **Missing** | |
| `*.GPB` (including `CSTAT.GPB` and startup `BINDATA.GP`) | **Missing** | No greeting screen from `BINDATA.GP`. Session chrome is written back into `Config.toml` |
| `ICONS.GP` | **Have** | Decoded and shown. Clicks that have no action yet: **SETUP**, **FILETRANS**, **SORT**, **FIND** |
| `<mycall>.GPL` journal | **Missing** | |
| `*.GPT` system texts, `*.GPH` remote help | **Missing** | `//Help` does not send a help file |

### 4 Keyboard

| DOS feature | Status | Notes |
|-------------|--------|-------|
| F1–F10 channels, ALT-M monitor, ALT-E editor, ALT-X quit | **Partial** | Mouse (and TUI keys). Pixel UI has no ALT/F-key map |
| F11 / ALT-F1 BBS-Menu, F12 / ALT-F2 Box-List | **Partial** | Panes exist. TUI can switch to them. They are not mailbox menus |
| ALT-C connect, ALT-D disconnect, ALT-Y mycall | **Partial** | Icons and dialogs, not the ALT keys |
| ALT-O DOS shell | **DOS-only** | Pixel **DOS** icon minimises the window. It does not run a shell or apply `TNCDOS` |
| ALT-R softscroll, ALT-Z zoom | **Partial** / **DOS-only** | Session line-step toggles a scroll feel. VGA zoom and the three-button mouse do not apply |
| ALT-U setup | **Missing** | |
| ALT-F4 clear window, ALT-F6 last link status, ALT-F7 last error | **Partial** | Session clear-pad button empties the visible log. No “last status” / “last error” popup |
| ALT-G hourly gong | **Have** | Clock double-note toggles it |
| ALT-B scroll lock, ALT-S continuous save, ALT-F send file, ALT-Q QRG, ALT-N save name, ALT-T connect text | **Partial** | Scroll lock, QRG, and connect-text picker exist on the session bar. Continuous save, file send, and “store this station in `NAMES.GP`” do not |
| ALT-F8/F9/F10 splitscreen | **Partial** | Text UI only. Pixel UI has no monitor split under the QSO |
| Shift-F1…F12 macro send | **Missing** | |
| Ctrl-D date, Ctrl-T time, Ctrl-O origin line, Ctrl-Y / Ctrl-C / Ctrl-P line clipboard, Ctrl-K copy to editor, Ctrl-Enter send without CR, Ins insert/overwrite | **Missing** | Compose is a normal text field with history |
| BBS cursor / Space mark / Enter = `READ` / Esc leave | **Missing** | |
| ALT + numpad for ASCII 1–31 | **DOS-only** | The OS keyboard does this |

### 5 QSO operation

| DOS feature | Status | Notes |
|-------------|--------|-------|
| 10 QSO channels, unread-channel colour | **Partial** | Channel buttons and link state are there. A channel with unseen text does not change menu colour the way `MenuInverzAttr` describes |
| Connect with digipeaters from `NAMES.GP`, or typed `CALL via via` | **Partial** | Typed via works. Name lookup does not |
| Compose window, auto line wrap, auto-send of a full line | **Partial** | Pre-write history and a compose box. No per-channel line length (80, or 65 in CONVERSE) and no automatic send when the line fills |
| ALT-T file send: text, raw binary, AutoBin, editor contents | **Missing** | See `docs/TRANSFER.md` for the protocol we intend to match |
| Transfer progress (bytes, percent, elapsed, estimate) and resume after quit | **Missing** | |
| ALT-S continuous text save, print to LPT1–LPT4, save a marked range, AutoBin receive names `<call>.000`–`.999` | **Missing** | |
| 7Plus / LCplus Code AutoSave (`go_7+.` … `stop_7+.`, ERR/COR parts) | **Partial** | Sniffer in `src/transfer/sevenplus.rs`. Not attached to received I-frames. The **7+** button does not start a capture |
| Mouse-mark QSO lines and Ctrl-Ins / Ctrl-K into the editor | **Missing** | |
| Remote control from the peer, and ESC `//` from the operator | **Partial** | Incoming `//` lines are parsed when **Remote** is on. ESC command entry is a TUI placeholder only |
| External programs and GPRI (`ExternalDir`, `//RUN`) | **Missing** | `RUN.GPI` is sent as text |
| Splitscreen while in QSO | **Partial** | Text UI only |
| Scroll-lock per channel | **Partial** | One session-bar switch for the visible pad, not a TNC poll hold, and not per channel in the DOS 1.50+ sense |
| ESC TNC command window | **Missing** | |
| Station journal `LOG=ON` plus `QRG` | **Missing** | QRG on the session bar is a label and a log line, not a `<mycall>.GPL` entry |
| Alternate connect texts `CTEXT_A.GPI` and fallback to `CTEXT.GPI` | **Partial** | Any one GPI can be picked. No automatic fallback chain, no per-mycall file |
| Sysop password (`SY`/`PW`, `*.GPW`, TheNet / BayCom / FlexNet / DieBox / FALCon types) | **Missing** | |
| DieBox `(BIN)` mail via AutoBin | **Missing** | Depends on the unwired AutoBin path |

### 6 Mailbox menus

| DOS feature | Status | Notes |
|-------------|--------|-------|
| Recognise DieBox, F6FBB 5.15, DK5SG, DB0IE check/list formats | **Missing** | |
| BBS-Menu (F11): mark, Enter sends mailbox `READ`, keep list across a timeout on the same channel, replace-or-append prompt | **Missing** | One shared buffer, as in DOS, is the right model when this is built |
| Box-List (F12): one rubric, same mark/read behaviour, DieBox only | **Missing** | |
| ALT-F find, ALT-S sort (BBS menu only) | **Missing** | Icons are on the strip |
| Import a list from the editor, change reference channel / rubric | **Missing** | |
| Unknown list formats | **Missing** | Manual 6.4 is an ongoing compatibility job, same as DOS GP |

### 7 Editor

| DOS feature | Status | Notes |
|-------------|--------|-------|
| Edit GPI / config text without leaving GP, CP437 load and save | **Have** | `ALT-L` / `ALT-S` are the load and save icons, not keys |
| Insert vs overwrite, 80-column screen lines, 255-column logical lines, line-break setting | **Missing** | Ordinary editor widget. Lines are not hard-wrapped at 80 on save |
| 40–780 line buffer from `EditBuffer` | **Partial** | Cap exists; the DOS minimum/maximum rules are not enforced in the UI |

### 8 MHeard (ALT-H)

| DOS feature | Status | Notes |
|-------------|--------|-------|
| Live list from the monitor | **Have** | UI frames on KISS. Hostmode monitor text is not folded into the same list yet |
| `//MHeard` sends the list | **Have** | |
| Enter on a row checks `NAMES.GP` / starts a connect the way the BBS menu does | **Partial** | **CONNECT** uses the selected call on the first free channel. No `NAMES.GP` test |
| Fixed 40-entry DOS window | **Partial** | Cap is 500 |

### 9 QSO spy (GP286)

| DOS feature | Status | Notes |
|-------------|--------|-------|
| Up to 10 spy windows, `SpyBuffer`, `MaxSpy` | **Partial** | Ten slots. `MaxSpy` is stored. The pixel UI always offers ten |
| Pick a QSO from MHeard (ALT-S / **SPY**), both directions, two colours, show N(S) | **Partial** | A slot watches one callsign and copies matching monitor lines. No pair detection, no “please select QSO”, no sequence number, no save-this-QSO prompt |
| Kill spy (ALT-K / **KILL SPY**) | **Partial** | Clears the call in the current spy slot. Does not pick a QSO out of MHeard to stop |
| AutoSpy / NoAutoSpy, files `CALL.000`–`.999` in `SpyDir`, close on DISC/DM/UA, skip `ctl UI` | **Missing** | The filter strings are loaded and never applied |
| Copy 7Plus parts out of a spy file | **Missing** | Would fall out of Code AutoSave once that is wired |

### 10 Autorouter

All of chapter 10 is **missing**:

- `NAMES.GP` syntax `TYPE>CALL NAME; path` and `TYPE>IDENT:CALL`
- Types `B` mailbox, `D` L2, `F` FlexNet, `N` NetRom, `!` (`C!`), `T` terminal
- Recursive paths and idents
- SP-gateway dummy node (`D>0` port trick)
- `PORTx = digi, qrg` for multiport TNCs (0–8)
- QRG `<IF>` blocks inside `NAMES.GP`
- `%DIGI%` meaning “digi on the port that matches the current QRG”
- ALT-N append the connected station onto `NAMES.GP`

`MultiPort` in config does not select a port or a frequency.

### 11 Setup (ALT-U)

| Menu item | Status |
|-----------|--------|
| Colours, with ENTER/ESC, not written back unless `CONFIG.GP` is edited | **Missing** as a menu. Colours load from config |
| Remote prompt template (`PROMPT`, macros, `//PRompt ON`) | **Missing**. `//PRompt` is unimplemented. A prompt string is stored |
| Remote on/off for this channel | **Partial**. Session **Remote** flag, one global switch |
| Backup of the scroll buffer (`BACKUP`) | **Missing**. Flag stored. Pads are saved as `log/*.txt` on exit, which is a different mechanism |
| Echo | **Have** (session bar; monitor mirror, not strictly per channel) |
| RX click 0 / 1 / 2 | **Missing**. Connect bell audio exists; RX click mode is stored and unused |
| Line break per channel | **Missing** |
| Packet length for all channels | **Partial**. `PACLEN` / `packet_payload` is in config. No setup control |
| Softscroll | **Partial**. Cosmetic line-step only. Chapter 15’s pixel soft-scroll is **DOS-only** |
| 7Plus / LCplus Code AutoSave per channel | **Partial**. One **7+** flag, not wired to the sniffer |
| Beacon menu (monitor only) | **Missing** |
| Session-bar switches: QRG, CT, umlauts, echo, line step, bell, 7+, remote, BIN-RX, BIN-TX, ScrLock | **Partial**. All of these buttons exist. Umlauts are an extended-ASCII decode toggle. BIN and 7+ do not transfer files. ScrLock does not freeze a TNC poll |

### 12–18 and the appendices

| DOS feature | Status |
|-------------|--------|
| DOS shell, `EXIT`, `TNCDOS = M N` | **DOS-only** |
| Mouse on icons, menus, channel buttons | **Have** |
| Right-click = Space in CHECK/LIST, drag-select text, scrollbar page clicks, hide scrollbar with ALT-B | **Partial**. Scrollbar show/hide is the **B** session button. List marking and drag-select are missing |
| `/e` `/v` graphics, `/h` skip hostmode init, `/NOXMS` `/NOEMS`, `/XTKBD` `/ATKBD`, `/3MOUSE`, `/NOFIFO`, `/CHECK` virus scan | **DOS-only** |
| `/KISSOFF` leave KISS at startup | **Partial**. KISS serial applies the close section from `KISS.INI` on exit, not a DOS `/KISSOFF` switch at start |
| Softscroll implementation notes (chapter 15) | **DOS-only** |
| What hostmode is (chapter 16) | **Partial**. WA8DED hostmode driver exists (JHOST1, poll, `WA8DED.INI`). Not yet tested on hardware |
| Resync / lost V24 characters (chapter 17) | **Missing** as a UI. USB-serial replaces the DOS UART path; a resync banner is not shown |
| Licence and author appendices | Not a product feature |

---

## `//` commands

Parsed in `src/remote/commands.rs`. Executed from the pixel UI in
`handle_remote_on_channel`.

| Command | Status |
|---------|--------|
| `//Quit`, `//DISConnect` | **Have** |
| `//Info`, `//NEws`, other `*.GPI` by stem or prefix | **Have** (file must exist under `data/`) |
| `//VERsion`, `//Echo`, `//Bell`, `//MHeard`, `//Name` | **Have** |
| `//7+`, `//Activity`, `//CATalog` / `//DIR`, `//CD`, `//CUT`, `//DELete` / `//Kill`, `//END`, `//Help`, `//Language`, `//PARameters`, `//PRompt`, `//Read`, `//RPrg`, `//RTt`, `//RUN`, `//Usage`, `//Write`, `//WPrg`, `//C`, `//<n> text`, chat `#` | **Missing** (parsed, then “not implemented”, except `//RUN` which only sends `RUN.GPI` if that file is present) |

`#` as a chat allow-list token in `REMOTE=` is not applied.

---

## Already in place (so this list stays honest)

- Pixel layout: channel grid, analog clock with hourly gong, status grid, monitor and channel pads, compose history, icon strip from `ICONS.GP`, session bar.
- KISS AX.25: UI monitor, SABM/UA connect, I + RR, DISC, idle RR, self-connect between two local channels, TCP KISS (Soundmodem / Direwolf) and serial KISS.
- Hostmode drivers for WA8DED and TFPCX (untested on a real TNC).
- Per-channel callsigns, connect and disconnect, connect text with `%` macros, MHeard with `//Name`.
- Manual spy slots that follow a callsign on the monitor.
- Editor load/save in CP437.
- Pad logs under `log/` for monitor, channels, spy, BBS, and box.
- Remote command catalogue and macro expander, ready for the commands that are still stubs.
- AutoBin and 7Plus libraries, documented in `docs/TRANSFER.md`, not yet hooked to a live channel.

---

## Suggested build order

1. Wire 7Plus AutoSave and AutoBin to the channel that has **7+** / **BIN-RX** / **BIN-TX** on, including `//RPrg`, `//WPrg`, `//Read`, `//Write`, `//CUT`.
2. `NAMES.GP` plus autorouter on Connect, then ALT-N.
3. BBS-Menu and Box-List parsers (DieBox first), mark, and `READ`.
4. Finish `//` remotes that only need files: help (`*.GPH`), catalog, delete, prompt, parameters, usage, language.
5. Journal (`LOG`, `QRG`, `UTC`, `<mycall>.GPL`) and the sysop `*.GPW` helper.
6. Setup menu for the items that are still config-only: colours, line length, paclen, RX click, beacon.
7. Pixel-UI hotkeys that operators still hit from muscle memory (F-keys, ALT-C/D/X, Shift-F macros, Ctrl-D/T/O).
