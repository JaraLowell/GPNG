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

1. **Mailbox lists other than DieBox are still missing.** DieBox `CHECK`
   fills the BBS menu and DieBox `LIST` fills the Box list, on the setup
   reference channel. F6FBB 5.15, DK5SG, and DB0IE are not parsed. A CHECK
   list is not split into rubric sections (manual 6.1).
2. **`NAMES.GP` is in use.** Connect accepts a call, an ident, or a name from
   `data/NAMES.GP` and builds the path (digipeaters, then node `C` / `C!`).
   **Name** on the channel strip is ALT-N, and `//Name` writes the same file.
   `%N` reads it. MHeard no longer stores names. Still open: a type `B`
   mailbox does not block remotes, and `PORTx` does not switch a multiport TNC.
3. **7Plus Code-AutoSave is untested.** The sniffer and `//7+` are in the
   program, and `//Read` sends a file from `data/USER` as plain text. No live
   DOS part (`go_7+.` … `stop_7+.`) has been captured, so the save into
   `data/USER` is not confirmed. BIN-TX, `//7+`, and `//Read` do not run the
   7Plus encoder. **AutoBin is on the air** with DOS GP 1.63 (`#BIN#`, `#OK#`,
   `#NO#`, `#ABORT#`, the GP 1.63 CRC). Still open there: `//RPrg` / `//WPrg`,
   resume, and ALT-T.
   Later, and only GPNG to GPNG: on connect, ask if the other station has 7+
   support. If it does, send CTEXT and files as `#7+#` plus the data. Our side
   puts that back together and shows it on the channel. Classic DOS `go_7+.`
   stays as it is. Not built.
4. **Most `//` remotes are recognised and then refused.** The parser covers
   the classic set. Only quit, disconnect, version, echo, bell, MHeard,
   `//Name`, and a GPI text file (`INFO`, `NEWS`, `QRT`, `RUN`, other
   `*.GPI` stems) actually run. Catalog, read, write, delete, help, run,
   parameters, RTT, AutoBin `//RPrg` / `//WPrg`, cross-channel `//n`, and
   `//C` answer “not implemented”. `REMOTE=` / `NOREMOTE=` are loaded and
   not enforced. A mailbox marked type `B` in `NAMES.GP` is not auto-blocked,
   because that file is not read.
5. **The journal and the sysop helper are in.** The station journal runs when `LOG=ON`
   (or `log = true`). A disconnect appends one line to
   `data/log/<base call>.GPL`. One file per base call, not per SSID.
   `QRG` (at most 7 characters) and `UTC` (minutes from the PC clock) are
   read. The same folder gets `gpng.log`: the shell and debug stream,
   because a release build has no console. `*.GPW` answers TheNet / BayCom
   character positions and the old FlexNet 5-digit sum, and puts the reply
   in the compose box. Ctrl-B pastes it again. A DieBox / FALCon 60×27 grid
   is recognised and not calculated. The monitor beacon menu sends a UI frame
   on KISS. Hostmode stores the beacon and does not invent a TNC command.
6. **Setup (ALT-U) is a menu.** The rows follow the window that was open
   (monitor, channel, editor, BBS, Box-List, MHeard, spy). Yellow rows open
   colours, the prompt, the file drawer, or the beacon. Green rows toggle
   or step a number. Esc from the main menu saves. Colours, the beacon, and
   the other setup values are written to `Config.toml`. DOS wrote
   `CSTAT.GPB`, and the manual left colours out of that file unless
   `CONFIG.GP` was edited. Word wrapping and input columns are stored and
   do not wrap the editor yet. The reference channel is where the DieBox
   list is read, and the file drawer is the Box-List rubric.
7. **A few DOS keys still have nothing to run.** The pixel UI binds the
   chapter 4 keys that already have an action (channels, panes, connect,
   QRG, Shift-F macros, compose Ctrl keys). Still ahead: continuous save
   (ALT-S on a channel), file send (ALT-F),
   insert/overwrite, and copying marked QSO text
   (Ctrl-K). ALT-Z zoom stays a DOS VGA function.

---

## By manual chapter

### 1–2 Installation and multi-call texts

| DOS feature | Status | Notes |
|-------------|--------|-------|
| Install to a directory, `CONFIG.GP` | **Partial** | `Config.toml`, with an optional `CONFIG.GP` overlay for recognised keys |
| `UserDir`, `ExternalDir`, `SaveDir`, `LogDir`, `SpyDir` | **Partial** | `data/USER` is created and used by 7Plus autosave and `//Read`. Incoming parts are intended for `data/SAVE`, joined by `transfer::sevenplus_codec`. `data/TEMP` is where a send is split. Those folders are not wired to the channel yet |
| Per-call `CTEXT???.GPI`, `<call>.GPC`, `<call>.GPM`, `<mycall>.GPI`, `QRT.GPI` | **Partial** | `{CALL}.GPM` lives in `data/mail`. On connect, and on bare `//Read`, that station receives its mail instead of the USER list. `//DEL` / `//Kill` with no file name deletes only that mail. Another station's file is refused, and `//CATalog` does not list the folder. No per-call `.GPC` yet. `QRT.GPI` still goes out on `//Quit` |
| `GP.EXE` vs `GP286.EXE`, overlay, EMS/XMS | **DOS-only** | One native binary. QSO spy is included (it was the GP286-only extra) |

### 3 Configuration and data files

| DOS feature | Status | Notes |
|-------------|--------|-------|
| VGA colour names for every window | **Have** | The setup colour box cycles them. Confirming writes the names into `Config.toml` and refreshes the pane |
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
| `Questions`, `PopEnableKeyboard`, `MultiPort` | **Partial** | A new DieBox list asks replace, append, or interrupt. `Questions=OFF` does not suppress that. Popup keyboard dismissal is unused. No DRSI / TFPCX multiport |
| `<IF>` / `<ELSE>` / `<END>` / `%TFPCX200` in `CONFIG.GP` | **Missing** | Legacy overlay is flat `KEY = value` |
| `*.GPI` and the fifteen `%` macros | **Have** | `%V %C %N %Y %K %T %D %B %I %Z %_ %% %O %? %>`, plus width `#nn`. `%DIGI%` expands from context but nothing fills a real digi path |
| `CTEXT???.GPI`, `NEWS.GPI`, `QRT.GPI`, `RUN.GPI`, `ORIGIN.GPI`, `F1.GPI`–`F12.GPI` | **Partial** | Files in `data/` are served by name. Shift-F1…F12 sends `F1.GPI`…`F12.GPI` on a channel. `RUN.GPI` is not sent; `//RUN` answers `not supported` |
| `<call>.GPC` / `<call>.GPM` | **Missing** | |
| `MHEARD.GP` | **Partial** | JSON list, up to 500 UI frames. Names are not stored here. Not the DOS 40-line MH file |
| `NAMES.GP` | **Have** | `data/NAMES.GP`. Call, ident, and name lookup. `%N`, `//Name`, and the channel **Name** button read and write it |
| `*.GPB` (including `CSTAT.GPB` and startup `BINDATA.GP`) | **Missing** | No greeting screen from `BINDATA.GP`. Setup and session chrome are written back into `Config.toml`, not `CSTAT.GPB` |
| `ICONS.GP` | **Have** | Decoded and shown. **SETUP** opens the same menu as ALT-U. **SORT** sorts the BBS menu. **FIND** searches the BBS menu or the Box list. **FILETRANS** still has no action |
| `<mycall>.GPL` journal | **Have** | `data/log/<base call>.GPL` when `LOG=ON`. One file per base call. A disconnect appends date, UTC time, the other call, QRG, and the duration |
| `*.GPT` system texts, `*.GPH` remote help | **Partial** | `//Help` sends the overview or one `@TAG` section from the language pack. `G.GPH` is English. `//Language` switches `NL`, `DL`, `F`, `PL`, `SLO`, `OK`, `HA`. `*.GPT` system texts are still unused |

### 4 Keyboard

| DOS feature | Status | Notes |
|-------------|--------|-------|
| F1–F10 channels, ALT-M monitor, ALT-E editor, ALT-X quit | **Have** | Pixel UI and the mouse. ALT-X closes the window |
| F11 / ALT-F1 BBS-Menu, F12 / ALT-F2 Box-List | **Partial** | The keys open the panes. DieBox `CHECK` and `LIST` lines from the reference channel become the list. F6FBB, DK5SG, and DB0IE do not |
| ALT-C connect, ALT-D disconnect, ALT-Y mycall | **Have** | Same dialogs as the icons. ALT-Y only while that channel is disconnected |
| ALT-O DOS shell | **DOS-only** | ALT-O and the **DOS** icon minimise the window. They do not run a shell or apply `TNCDOS` |
| ALT-R softscroll, ALT-Z zoom | **Partial** / **DOS-only** | ALT-R cycles the session line step. The button shows `/` or a drawn stair (manual 11.12). Chapter 15’s pixel soft-scroll is **DOS-only**. ALT-Z zoom says so |
| ALT-U setup | **Have** | ALT-U and the **SETUP** icon open Program parameters for the current window. Esc outside BBS-Menu and Box-List opens the TNC command box. On hostmode that line is sent to the TNC. On KISS the status line says there is no command channel |
| ALT-F4 clear window, ALT-F6 last link status, ALT-F7 last error | **Have** | ALT-F4 clears the visible pad (or the editor buffer). ALT-F6 and ALT-F7 show the last link report and the last modem warning on the status line |
| ALT-G hourly gong | **Have** | ALT-G and the clock double-note toggle it |
| ALT-B scroll lock, ALT-S continuous save, ALT-F send file, ALT-Q QRG, ALT-N save name, ALT-T connect text | **Partial** | ALT-B, ALT-Q, ALT-N, and ALT-T (sends the connect text on a live channel) work. On the BBS menu ALT-S sorts and ALT-F finds. On the Box list ALT-F finds. Elsewhere ALT-S on a channel and ALT-F answer that continuous save and file send are not built yet. In the editor, ALT-S saves and ALT-L loads |
| ALT-F8/F9/F10 splitscreen | **Have** | Pixel UI, channel only (`docs/screenshots/SplitScreen.png`). ALT-F8 or a right-click on the QSO, the session bar, or the lower pane toggles it. ALT-F9 / ▲ move the bar up (more monitor); ALT-F10 / ▼ move it down. Drag the ▲▼ pair as well. The lower pane is the same monitor pad, not a second buffer. On Monitor the arrows still nudge that pad. They do nothing on Spy |
| Shift-F1…F12 macro send | **Have** | On a channel, sends `data/F1.GPI` … `F12.GPI` with the `%` macros expanded |
| Ctrl-D date, Ctrl-T time, Ctrl-O origin line, Ctrl-Y / Ctrl-C / Ctrl-P line clipboard, Ctrl-K copy to editor, Ctrl-Enter send without CR, Ins insert/overwrite | **Partial** | Ctrl-D, Ctrl-T, and Ctrl-O insert into the compose box (or the editor line). Ctrl-Y / Ctrl-C / Ctrl-P cut, copy, and paste that line. Ctrl-B pastes the last sysop reply. Enter sends the compose line with a CR; Ctrl-Enter sends it without one. Ctrl-K (marked QSO text) and Insert/overwrite are not built yet |
| BBS cursor / Space mark / Enter = `READ` / Esc leave | **Have** | Up and down move the bar. Space marks and steps. Enter sends `R` for every marked line, or for the bar when nothing is marked. Esc returns to Monitor. A right-click marks. Elsewhere Esc opens the TNC command box |
| ALT + numpad for ASCII 1–31 | **DOS-only** | The OS keyboard does this |

### 5 QSO operation

| DOS feature | Status | Notes |
|-------------|--------|-------|
| 10 QSO channels, unread-channel colour | **Partial** | Channel buttons and link state are there. A channel with unseen text does not change menu colour the way `MenuInverzAttr` describes |
| Connect with digipeaters from `NAMES.GP`, or typed `CALL via via` | **Have** | A single token is a call, ident, or name in `NAMES.GP`. `CALL,VIA` is still an explicit path and skips the file |
| Compose window, auto line wrap, auto-send of a full line | **Partial** | Pre-write history and a compose box. No per-channel line length (80, or 65 in CONVERSE) and no automatic send when the line fills |
| ALT-T file send: text, raw binary, AutoBin, editor contents | **Missing** | See `docs/TRANSFER.md` for the protocol we intend to match |
| Transfer progress (bytes, percent, elapsed, estimate) and resume after quit | **Partial** | Layout is the magenta status grid, not a popup. `Transfer30.png` / `Transfer80.png`: path, `Bytes: sent/total`, green bar with the percent, `Elapsed Time`, `Eff. Baud`, `Estimated Time`, `Status: BIN-TX`, mode title `Information Transfer`. Details in `docs/TRANSFER.md`. The grid is not built yet. Resume after quit is still missing |
| ALT-S continuous text save, print to LPT1–LPT4, save a marked range, AutoBin receive names `<call>.000`–`.999` | **Missing** | |
| 7Plus / LCplus Code AutoSave (`go_7+.` … `stop_7+.`, ERR/COR parts) | **Partial** | Untested on the air. No sample part yet. The sniffer would save channel RX into `data/USER` when **7+** is on. `transfer::sevenplus_codec` can encode and join parts in Rust. `.err` / `.cor`, `data/SAVE`, `data/TEMP`, and waiting for `ok`/`okay` are not wired. Later, GPNG to GPNG only: ask on connect, then send CTEXT and files as `#7+#` plus the data and show the rebuilt text on the channel |
| Mouse-mark QSO lines and Ctrl-Ins / Ctrl-K into the editor | **Missing** | |
| Remote control from the peer, and ESC `//` from the operator | **Partial** | Incoming `//` lines are parsed when **Remote** is on. Esc opens the TNC command box (manual 5.9). A line that starts with `//` is not run as the operator’s own remote (manual 5.5, `ESC //h`). On KISS the box does not send |
| External programs and GPRI (`ExternalDir`, `//RUN`) | **Missing** | `//RUN` answers `not supported`. It does not start a program |
| Splitscreen while in QSO | **Have** | Channel only. Top pane is the QSO, the session bar is the divider, and the monitor pad is drawn under it. Not on Spy, BBS-Menu, or Box-List. Shot: `docs/screenshots/SplitScreen.png`. ALT-F8, right-click, ▲▼, and dragging the arrows |
| Scroll-lock per channel | **Partial** | One session-bar switch for the visible pad, not a TNC poll hold, and not per channel in the DOS 1.50+ sense |
| ESC TNC command window | **Partial** | Esc opens the box. Esc again closes it without sending. Enter sends the line on hostmode. KISS has no TNC command channel. ALT-O does not restore `TNCINI` |
| Station journal `LOG=ON` plus `QRG` | **Have** | `LOG`, `QRG`, and `UTC` come from `CONFIG.GP` or `Config.toml`. The line is written under `data/log` (or `LogDir`). `gpng.log` in that folder is the shell log |
| Alternate connect texts `CTEXT_A.GPI` and fallback to `CTEXT.GPI` | **Partial** | Any one GPI can be picked. No automatic fallback chain, no per-mycall file |
| Sysop password (`SY`/`PW`, `*.GPW`, TheNet / BayCom / FlexNet / DieBox / FALCon types) | **Partial** | `DB0IZ-9` reads `data/DB0IZ9.GPW`. Type `N`, `D`, and `W` pick 1-based characters. Type `F` uses the old 5-digit product-sum, or character positions when the secret is longer. The reply is placed in the compose box; Ctrl-B pastes it again. A type `B` 60×27 grid is recognised and not calculated |
| DieBox `(BIN)` mail via AutoBin | **Missing** | Depends on the unwired AutoBin path |

### 6 Mailbox menus

| DOS feature | Status | Notes |
|-------------|--------|-------|
| Recognise DieBox, F6FBB 5.15, DK5SG, DB0IE check/list formats | **Partial** | DieBox `CHECK` (`CALL > BOARD......nr date`) and `LIST` (`nr CALL date time bytes title`) from the DB0HB handbook. F6FBB, DK5SG, and DB0IE are not parsed |
| BBS-Menu (F11): mark, Enter sends mailbox `READ`, keep list across a timeout on the same channel, replace-or-append prompt | **Have** | One shared buffer. Space marks (manual 4.4). Enter sends `R BOARD nr` on the BBS reference channel for every marked line, or for the bar when nothing is marked. The list stays until that channel is disconnected locally. Rubric sections are not split out |
| Box-List (F12): one rubric, same mark/read behaviour, DieBox only | **Have** | `R nr`, or `R drawer nr` when the file drawer is set. Same mark and read keys. The reference channel is the Box-List one |
| ALT-F find, ALT-S sort (BBS menu only) | **Have** | Find (manual 6.3) opens an empty box. The same word continues from the bar; a different word starts at the top. Sort is rubric then number, and only on the BBS menu (manual 4.4). On a channel, ALT-S and ALT-F are still the unwired save and file send |
| Import a list from the editor, change reference channel / rubric | **Have** | Insert appends DieBox lines from the editor. ALT-F4 clears the list. The reference channel and the file drawer are the setup values |
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
| `//MHeard` sends the list | **Have** | Bare `//mh` sends the newest 10. `//mh 50` / `//mh 5` set the count. A callsign asks for that station only |
| Enter on a row checks `NAMES.GP` / starts a connect the way the BBS menu does | **Partial** | **CONNECT** uses the selected call on the first free channel and applies a `NAMES.GP` path when that call is listed. Enter on the row still does not connect. The BBS menu’s Enter sends `READ`, which is a different window |
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

| DOS feature | Status | Notes |
|-------------|--------|-------|
| `TYPE>CALL NAME; path` and `TYPE>IDENT:CALL` | **Have** | Types `B` `D` `F` `N` `!` `T`. The first 22 letters of the name are what Connect and `%N` use |
| Recursive paths and idents | **Have** | A hop that names another station is replaced by that station's path |
| Digipeater connect and node `C` / `C!` | **Have** | A path of only `D>` hops is one connect. `N` / `F` / `!` connect the first node, then send `C` or `C!` when the reply names that station. The first link is always plain `C`, even when that hop is `!` |
| SP-gateway `D>0` | **Have** | A numeric `D>` hop becomes `-port` on the next call (`C DB0XYZ-0`) |
| `<IF qrg>` / `<ELSE>` / `<END>` | **Have** | The block is used when `qrg` equals the session QRG printed with a dot (`430.625`). The comma form (`430,625`) stays a different key, as in the manual, and does not match that number |
| `%DIGI%` and `PORTx = digi, qrg` | **Partial** | `%DIGI%` becomes the digi whose `PORTx` QRG matches. `PORTx` does not switch a multiport TNC or a radio |
| ALT-N | **Have** | Channel **Name** button, and `//Name`. Updates the first line for that call, or appends `T>CALL NAME;` |

A mailbox marked `B` is stored and still does not block remotes. `MultiPort` in config does not select a port.

### 11 Setup (ALT-U)

| Menu item | Status |
|-----------|--------|
| Colours, with ENTER/ESC | **Have**. Left/right cycle the VGA name. Esc leaves the draft. “Use these colours and exit” applies them and they are written to `Config.toml` with the rest of the menu. DOS did not write colours back unless `CONFIG.GP` was edited |
| Remote prompt template (`PROMPT`, macros, `//PRompt ON`) | **Have**. The channel menu edits the template. `//PRompt ON` sends it after each remote command. `//PRompt OFF` stops that |
| Remote on/off for this channel | **Partial**. Session **Remote** flag, one global switch |
| Backup of the scroll buffer (`BACKUP`) | **Have**. When it is on, exit still writes the pads as `data/log/*.txt`. When it is off, that write is skipped |
| Echo | **Have** (session bar and the menu; monitor mirror, not strictly per channel) |
| RX click 0 / 1 / 2 | **Have**. 0 is off, 1 plays the bell when the incoming line is not the active channel, 2 always plays it |
| Line break per channel | **Missing**. Input columns are stored and do not wrap the compose box |
| Packet length for all channels | **Have**. The channel menu sets `packet_payload`, which is the size used when a line is sent |
| Softscroll | **Partial**. The menu and ALT-R cycle the line step. The session button shows a stair when it is on and `/` when it is off (manual 11.12). Chapter 15’s pixel soft-scroll is **DOS-only** |
| 7Plus / LCplus Code AutoSave per channel | **Partial**. The channel menu flips the one **7+** switch for every channel. Live capture into `data/USER` is untested |
| Word wrapping, `*.BAK` | **Partial**. `*.BAK` is copied before an editor save when the toggle is on. Word wrapping is stored and the editor does not hard-wrap yet |
| Insert list from editor, reference channel, file drawer | **Have**. Insert appends DieBox lines from the editor. The reference channel is where the live list is read and where `READ` is sent. The file drawer is the Box-List rubric |
| Beacon menu (monitor only) | **Partial**. Address, text, and period are saved. Period 0 is off. Manual 11.11: `ALL` is `fm CALL to ALL ctl UI^`, and `ALL DB0FN` is `fm CALL to ALL via DB0FN ctl UI^`. The same path is what a line typed on the monitor uses, so `APMI03 WIDE1-1 WIDE2-2` is `to APMI03 via WIDE1-1,WIDE2-2`. On KISS the period sends that one UI frame. The heard copy with `via DB0FN*` is the digipeater’s repeat, not a second send. On hostmode the values are stored and no TNC beacon command is invented |
| InsertMonLine | **Partial**. Stored. Monitor blank lines are not inserted |
| System language | **Have**. Cycles `EN`, `NL`, `DL`, `F`, `PL`, `SLO`, `OK`, `HA` into `remote_language` |
| Session-bar switches: QRG, CT, umlauts, echo, stair or `/`, bell, 7+, remote, BIN-RX, BIN-TX, Name, ScrLock | **Partial**. The soft-scroll mark is a drawn stair or `/`. Umlauts are also the setup toggle. **7+** is the Code-AutoSave switch and is untested on the air. **Name** writes `NAMES.GP`. BIN-RX / BIN-TX run the `#BIN#` transfer. ScrLock does not freeze a TNC poll, and it is not per channel |

### 12–18 and the appendices

| DOS feature | Status |
|-------------|--------|
| DOS shell, `EXIT`, `TNCDOS = M N` | **DOS-only** |
| Mouse on icons, menus, channel buttons | **Have** |
| Right-click = Space in CHECK/LIST, drag-select text, scrollbar page clicks, hide scrollbar with ALT-B | **Partial**. A right-click on a BBS or Box-List row marks it. Scrollbar show/hide is the **B** session button. Drag-select is missing |
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
| `//7+`, `//Read`, `//CATalog` / `//DIR` | **Have** | `//7+ ON\|OFF` is the session switch. `//Read file` and `//CAT` / `//DIR` stay inside `data/USER`. A pattern such as `*.txt;*.doc` is a DOS wildcard. The reply says `USER`, not a disk path |
| `//Help`, `//Language` | **Have** | `//Help` and `//H H` use `data/G.GPH` (English) or the pack selected with `//Language` |
| `//PRompt`, `//Usage` | **Have** | Prompt on/off, and this channel’s link state or a `NAMES.GP` lookup |
| `//PARameters` / `//P` | **Have** | TNC timing dump (P-Persistence, Slottime, Frack, T2, T3, TXDelay, MaxUnack). WA8DED re-queries `F` / `@T2` / `@T3` live. KISS applies `T`/`P`/`W` from `init[]` / `WA8DED.INI` as TAPR param frames after bring-up; `//P` reports those (T2/T3 as `?`). TFPCX sends the same lines as hostmode text into the soft-TNC |
| `//CSTATUS` / `//CS` | **Have** | PK-88-style dump of every channel: connected / disconnected / connect or disconnect in progress, peer call, digi path, `IO` on the asking channel |
| `//CD` | **Have** | Shows `USER`. Any other directory is refused, so the remote cannot browse the rest of the disk |
| `//Write`, `//END`, `//DELete` / `//Kill` | **Have** | Text capture into `data/USER` until `//END` or Ctrl-Z. `//DELete` of a named file removes only a file this session stored with `//Write`. Bare `//DEL` / `//Kill` deletes `data/mail/{CALL}.GPM` for the linked station only |
| `//RUN` | **Have** | Answers `not supported`. It does not start a program and does not send `RUN.GPI` |
| `//Activity`, `//CUT`, `//RPrg`, `//RTt`, `//WPrg`, `//C`, `//<n> text`, chat `#` | **Missing** | Parsed, then “not implemented” |

`#` as a chat allow-list token in `REMOTE=` is not applied.

---

## Already in place (so this list stays honest)

- Pixel layout: channel grid, analog clock with hourly gong, status grid, monitor and channel pads, compose history, icon strip from `ICONS.GP`, session bar.
- KISS AX.25: UI monitor, SABM/UA/DM connect, I + RR/REJ, DISC/UA teardown with T1, T3 idle poll with T1 retry, digi path on replies, self-connect between two local channels, TCP KISS (Soundmodem / Direwolf) and serial KISS.
- Hostmode drivers for WA8DED and TFPCX (untested on a real TNC).
- Per-channel callsigns, connect and disconnect, connect text with `%` macros. Names and the autorouter come from `data/NAMES.GP` (`//Name`, channel **Name**). MHeard is heard stations only.
- Manual spy slots that follow a callsign on the monitor.
- Editor load/save in CP437.
- Pad logs under `log/` for monitor, channels, spy, BBS, and box.
- Remote command catalogue and macro expander, ready for the commands that are still stubs.
- AutoBin on a live KISS link with DOS GP 1.63 (`#BIN#` / `#OK#` / `#ABORT#`, file in `data/SAVE`). 7Plus Code-AutoSave is implemented and untested. See `docs/TRANSFER.md`.
- Setup menu (ALT-U / **SETUP**). Rows follow the current window. Colours, the KISS beacon, and the Esc TNC box are live. Settings go to `Config.toml`.
- DieBox BBS menu and Box list: mark, `READ`, find, sort, and the replace / append / interrupt question. Other mailbox formats are not parsed.
- Session soft-scroll button: `/` or a stair. The text still steps a whole line.

---

## Suggested build order

1. 7Plus Code-AutoSave stays untested until a real `go_7+.` part is on the link. The later GPNG-only `#7+#` connect handshake is not this item. AutoBin on the channel is in. Still open: `//RPrg`, `//WPrg`, `//CUT`, and resume.
2. `NAMES.GP` plus autorouter on Connect, then ALT-N. **Done.** Connect uses `data/NAMES.GP`. **Name** is ALT-N. A type `B` mailbox still does not block remotes, and `PORTx` does not change the radio.
3. BBS-Menu and Box-List parsers (DieBox first), mark, and `READ`. **Done** for DieBox `CHECK` and `LIST` on the setup reference channel. F6FBB, DK5SG, and DB0IE are still ahead.
4. Finish `//` remotes that only need files: help (`*.GPH`), catalog, delete, prompt, parameters, usage, language. **Done.** Help follows `data/G.GPH` (English) and the other language packs. Catalog, read, write, and delete stay in `data/USER`. `//CD` cannot leave that folder. AutoBin `//RPrg` / `//WPrg` / `//CUT` remain with item 1.
5. Journal (`LOG`, `QRG`, `UTC`, `<mycall>.GPL`) and the sysop `*.GPW` helper. **Done** for the journal and for TheNet, BayCom, and FlexNet. Shell and debug lines go to `data/log/gpng.log`. The DieBox 60×27 grid is not calculated.
6. Setup menu. **Done** for the DOS rows that have a live effect: colours, prompt, remote, backup, echo, RX click, umlauts, the stair / `/` line step, packet length, Code-AutoSave, language, editor `*.BAK`, insert-from-editor, the reference channel, the file drawer, the KISS beacon (manual 11.11, also the monitor unproto path), and the Esc TNC command box. Word wrapping, input columns, and InsertMonLine are stored. Hostmode does not send the beacon. A `//` line in the Esc box is not the operator remote from manual 5.5. Settings go to `Config.toml`, not `CSTAT.GPB`.
7. Pixel-UI hotkeys that match DOS GP chapter 4. **Done** for the keys that already have an action: F1–F12, ALT-M/E/X/U/O/R/G/Q/C/D/Y/N/T/B, ALT-F1/F2/F4/F6/F7/F8/F9/F10, Shift-F1–F12, Ctrl-D/T/Y/C/P/O/B, Ctrl-Enter, Esc (TNC command, or back to Monitor from BBS/List), and editor ALT-L/S. Still ahead: ALT-S continuous save, ALT-F file send, Insert/overwrite, and Ctrl-K. ALT-Z stays DOS VGA zoom. On the BBS menu ALT-S sorts and ALT-F finds; on the Box list ALT-F finds.
