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

1. **Mailbox lists other than DieBox need live BBS traffic.** DieBox `CHECK`
   fills the BBS menu and DieBox `LIST` fills the Box list, on the setup
   reference channel. F6FBB 5.15, DK5SG, and DB0IE parsers are not written.
   GP / GPNG are only the client: those list formats come from older mail /
   BBS servers (DieBox and others), so extra parsers need captured CHECK/LIST
   dumps from a live box — not something we can exercise from GAPS alone.
   A CHECK list is not split into rubric sections (manual 6.1).
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
4. **A few `//` remotes are still stubs.** The parser covers the classic
   set, and most of it runs (see the `//` table below). Cross-channel
   `//n` / `//0` chat mode is in (join/leave notices, both-way relay on
   the air). `//Quit` also accepts `//Q` and `//BYE`. Still parsed and
   then refused: `//Activity`, `//RPrg`, `//RTt`, `//WPrg`, `//C`, and
   chat `#`. `REMOTE=` / `NOREMOTE=` are loaded and not enforced; the
   **Remote** button is the one global switch, and with it off a `//RUN`
   is answered, not started. A mailbox marked type `B` in `NAMES.GP`
   still does not block remotes.
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
   or step a number. Esc from the main menu saves. RX-Click counts
   `NEWLINE.WAV` (0–2, default 2, one second apart). Connect Bell counts
   `CONNECT.WAV` (0–4, each copy as the last ends; 0 also silences
   `DISCONECT.WAV`). Colours, the beacon, and the other setup values are
   written to `Config.toml`. DOS wrote
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
| `UserDir`, `ExternalDir`, `SaveDir`, `LogDir`, `SpyDir` | **Partial** | `data/USER` is `//Read` and 7Plus autosave. `data/EXTERNAL` is `//RUN` (Python, PHP, Perl, Ruby, Node, Lua, or Java). Incoming parts are intended for `data/SAVE`. `data/TEMP` is where a send is split. SAVE and TEMP are not wired to the channel yet |
| Per-call `CTEXT???.GPI`, `<call>.GPC`, `<call>.GPM`, `<mycall>.GPI`, `QRT.GPI` | **Have** | Connect greeting: `{CALL}.GPC` in `data/` for the peer (SSID dropped), else the channel/global CT file (`CTEXT.GPI` …). Mail: `{CALL}.GPM` in `data/MAIL`; connect announces it, then the greeting. `//MAIL` / bare `//Read` send mail; `//DEL` / `//Kill` with no name delete only that mail. `//Info`: `<mycall>.GPI` (our station base call) wins over `INFO.GPI`. A release ships `INFO.GPI` as the example because the callsign is unknown at pack time; operators who want a personal info text add `<call>.GPI`. `QRT.GPI` on `//Quit` / `//Q` / `//BYE` |
| `GP.EXE` vs `GP286.EXE`, overlay, EMS/XMS | **DOS-only** | One native binary. QSO spy is included (it was the GP286-only extra) |

### 3 Configuration and data files

| DOS feature | Status | Notes |
|-------------|--------|-------|
| VGA colour names for every window | **Have** | The setup colour box cycles them. Confirming writes the names into `Config.toml` and refreshes the pane |
| Scroll-back sizes (`TextBuffer`, `EditBuffer`, `CheckBuffer`, `ListBuffer`, `SpyBuffer`, `MHBuffer`, `InputBuffer`) | **Partial** | Used as line caps. Not the DOS 40–780 × 84-byte segment model |
| `Screensave`, `PopUpTime`, `Time`, `Zone`, `ConnectBell` | **Partial** | `%Z` is the PC clock zone (CET in winter, CEST in summer in Holland), not the stored `Zone`. Screensaver and popup lifetime are stored and not acted on. Connect Bell is live: 0 is off (and silences `DISCONECT.WAV`), 1–4 plays `CONNECT.WAV` that many times, each copy as the last ends. The ♪ button silences connect, disconnect, and newline sounds on every channel. The clock chime is separate: `CHIME.WAV`, struck on a 12-hour clock (3 at 3:00 and at 15:00), 40 ms apart |
| Serial `SerNr` / `SerBaud` | **Have** | Modern port name + baud. Raw `SerPort` / `SerIRQ` are stored and unused (the OS owns the UART) |
| `FastPol` | **Partial** | Sets the hostmode poll interval. No 16550-vs-slow auto detection |
| `TNCINI` / `TNCDEI` | **Partial** | Applied on the WA8DED and TFPCX drivers (`WA8DED.INI` first on WA8DED). KISS uses `KISS.INI`, not these lines. `Date`/`Time` as a GP password that becomes TNC clock commands is not translated |
| `TNCDOS` | **Missing** | No DOS shell, so nothing rewrites the TNC while a shell is open |
| `DefaultPrint` / `InverzPrint` | **Missing** | Parsed only |
| `Channels`, per-channel `MYCALL` | **Have** | Up to 10. Empty slots follow the monitor call. Pixel UI can edit the call for the active pane |
| `NoName`, `InsertMonLine`, `RemoteLanguage` | **Partial** | Stored. `%N` falls back to the callsign, not the `NoName` salutation. Monitor blank lines and `*.GPT` language packs are not applied |
| `REMOTE` / `NOREMOTE` | **Partial** | Stored and not enforced as an allow-list. The session **Remote** switch is one global on/off for every channel. Off answers a `//RUN` with `<GPNG>: Remote commands are turned off by the sysop.` and starts nothing |
| `Questions`, `PopEnableKeyboard`, `MultiPort` | **Partial** | A new DieBox list asks replace, append, or interrupt. `Questions=OFF` does not suppress that. Popup keyboard dismissal is unused. No DRSI / TFPCX multiport |
| `<IF>` / `<ELSE>` / `<END>` / `%TFPCX200` in `CONFIG.GP` | **Missing** | Legacy overlay is flat `KEY = value` |
| `*.GPI` and the fifteen `%` macros | **Have** | `%V %C %N %Y %K %T %D %B %I %Z %_ %% %O %? %>`, plus width `#nn`. `%Z` is the PC clock zone (CET/CEST in Holland), not a fixed UTC and not the config `Zone`. `%DIGI%` expands from context but nothing fills a real digi path |
| `CTEXT???.GPI`, `NEWS.GPI`, `QRT.GPI`, `RUN.GPI`, `ORIGIN.GPI`, `F1.GPI`–`F12.GPI` | **Partial** | Files in `data/` are served by name. Shift-F1…F12 sends `F1.GPI`…`F12.GPI` on a channel. The **CT** button sets the global file on Monitor and a file for that channel on a channel (green while the channel has its own). Picking the global file on a channel drops the green and inherits the global file again. Bare `//RUN` sends `RUN.GPI`; `//RUN name` starts a script in `data/EXTERNAL` |
| `<call>.GPC` / `<call>.GPM` | **Have** | `data/{CALL}.GPC` replaces the channel connect text. `data/MAIL/{CALL}.GPM` is mail for the callsign with a numeric SSID removed. Connect announces it and still sends the greeting. `//MAIL` sends the mail, or `You have no mail`. Bare `//Read` sends the mail too |
| `MHEARD.GP` | **Partial** | JSON list, up to 500. Every shown monitor `fm` line records its FROM station (SABM, I, RR, UA, DISC, UI, …), except our own calls; a repeat moves that station to the top with the new time. Names are not stored here. Not the DOS 40-line MH file |
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
| ALT-G hourly gong | **Have** | ALT-G and the clock double-note toggle it. It strikes `CHIME.WAV` on a 12-hour clock (3 times at 3:00 and at 15:00, 12 at noon and midnight), 40 ms apart. `//BELL` and the ♪ sounds are not the chime |
| ALT-B scroll lock, ALT-S continuous save, ALT-F send file, ALT-Q QRG, ALT-N save name, ALT-T connect text | **Partial** | ALT-B, ALT-Q, ALT-N, and ALT-T (sends the connect text on a live channel) work. On the BBS menu ALT-S sorts and ALT-F finds. On the Box list ALT-F finds. Elsewhere ALT-S on a channel and ALT-F answer that continuous save and file send are not built yet. In the editor, ALT-S saves and ALT-L loads |
| ALT-F8/F9/F10 splitscreen | **Have** | Pixel UI, channel only (`docs/screenshots/SplitScreen.png`). ALT-F8 or a right-click on the QSO, the session bar, or the lower pane toggles it. ALT-F9 / ▲ move the bar up (more monitor); ALT-F10 / ▼ move it down. Drag the ▲▼ pair as well. The lower pane is the same monitor pad, not a second buffer. On Monitor the arrows still nudge that pad. They do nothing on Spy |
| Shift-F1…F12 macro send | **Have** | On a channel, sends `data/F1.GPI` … `F12.GPI` with the `%` macros expanded |
| Ctrl-D date, Ctrl-T time, Ctrl-O origin line, Ctrl-Y / Ctrl-C / Ctrl-P line clipboard, Ctrl-K copy to editor, Ctrl-Enter send without CR, Ins insert/overwrite | **Partial** | Ctrl-D, Ctrl-T, and Ctrl-O insert into the compose box (or the editor line). Ctrl-Y / Ctrl-C / Ctrl-P cut, copy, and paste that line. Ctrl-B pastes the last sysop reply. Enter sends the compose line with a CR; Ctrl-Enter sends it without one. Ctrl-K (marked QSO text) and Insert/overwrite are not built yet |
| Ctrl+PrintScreen (`STRG+Druck`) TNC-Statistics | **Missing** | Undocumented DOS hotkey (period tip scrap, not `GP.DOC`). `GP286` paints a host/serial status dialog titled `TNC-Statistics`: Interface (TFPCR / TFPCX), Version, COM-Nr, Baudrate, 550 FIFO on/off, Poll (`turbo` / `safe`, optional `, extended (DG3DBI)`), Polls/Sec, Errors (Resync). Hostmode / soft-TNC only — skip on KISS. Build and verify when exercising WA8DED and TFPCX |
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
| Remote control from the peer, and ESC `//` from the operator | **Partial** | Incoming `//` lines are parsed when **Remote** is on. Cross-channel `//n` / `//0` joins a TERM channel’s QSO and relays both ways; the room hears Joined/Left. **Remote** is one global switch: off answers a `//RUN` with `<GPNG>: Remote commands are turned off by the sysop.` and starts nothing; on runs it. Esc opens the TNC command box (manual 5.9). A typed `//` line on an unlinked channel runs here; on a linked channel it goes to the other station. On KISS the box does not send |
| External programs (`ExternalDir`, `//RUN`) | **Have** | GPNG addon, below. One text program from `data/EXTERNAL` per channel. Bare `//RUN` sends `RUN.GPI`. Samples: `FREE` (memory, then exits) and `DOS` (MS-DOS 6.20 window, `C:` is `data/USER`). DOS GPRI Pascal programs are not started |
| Splitscreen while in QSO | **Have** | Channel only. Top pane is the QSO, the session bar is the divider, and the monitor pad is drawn under it. Not on Spy, BBS-Menu, or Box-List. Shot: `docs/screenshots/SplitScreen.png`. ALT-F8, right-click, ▲▼, and dragging the arrows |
| Scroll-lock per channel | **Partial** | One session-bar switch for the visible pad, not a TNC poll hold, and not per channel in the DOS 1.50+ sense |
| ESC TNC command window | **Partial** | Esc opens the box. Esc again closes it without sending. Enter sends the line on hostmode. KISS has no TNC command channel. ALT-O does not restore `TNCINI` |
| Station journal `LOG=ON` plus `QRG` | **Have** | `LOG`, `QRG`, and `UTC` come from `CONFIG.GP` or `Config.toml`. The line is written under `data/log` (or `LogDir`). `gpng.log` in that folder is the shell log |
| Alternate connect texts `CTEXT_A.GPI` and fallback to `CTEXT.GPI` | **Have** | The CT button picks the channel file (`CTEXT.GPI`, `CTEXT_A.GPI`, or another GPI). A `data/{CALL}.GPC` for the station that connected replaces that file. Mail, when present, is one line before that greeting |
| Sysop password (`SY`/`PW`, `*.GPW`, TheNet / BayCom / FlexNet / DieBox / FALCon types) | **Partial** | `DB0IZ-9` reads `data/DB0IZ9.GPW`. Type `N`, `D`, and `W` pick 1-based characters. Type `F` uses the old 5-digit product-sum, or character positions when the secret is longer. The reply is placed in the compose box; Ctrl-B pastes it again. A type `B` 60×27 grid is recognised and not calculated |
| DieBox `(BIN)` mail via AutoBin | **Missing** | Depends on the unwired AutoBin path |

### 6 Mailbox menus

| DOS feature | Status | Notes |
|-------------|--------|-------|
| Recognise DieBox, F6FBB 5.15, DK5SG, DB0IE check/list formats | **Partial** | DieBox `CHECK` / `LIST` from the DB0HB handbook work as the client. F6FBB, DK5SG, and DB0IE need sample dumps from those BBS servers — GPNG cannot self-test them |
| BBS-Menu (F11): mark, Enter sends mailbox `READ`, keep list across a timeout on the same channel, replace-or-append prompt | **Have** | One shared buffer. Space marks (manual 4.4). Enter sends `R BOARD nr` on the BBS reference channel for every marked line, or for the bar when nothing is marked. The list stays until that channel is disconnected locally. Rubric sections are not split out |
| Box-List (F12): one rubric, same mark/read behaviour, DieBox only | **Have** | `R nr`, or `R drawer nr` when the file drawer is set. Same mark and read keys. The reference channel is the Box-List one |
| ALT-F find, ALT-S sort (BBS menu only) | **Have** | Find (manual 6.3) opens an empty box. The same word continues from the bar; a different word starts at the top. Sort is rubric then number, and only on the BBS menu (manual 4.4). On a channel, ALT-S and ALT-F are still the unwired save and file send |
| Import a list from the editor, change reference channel / rubric | **Have** | Insert appends DieBox lines from the editor. ALT-F4 clears the list. The reference channel and the file drawer are the setup values |
| Unknown list formats | **Missing** | Manual 6.4 is an ongoing compatibility job, same as DOS GP |

### 7 Editor

| DOS feature | Status | Notes |
|-------------|--------|-------|
| Edit GPI / config text without leaving GP, CP437 load and save | **Have** | Leaving EDIT and coming back keeps the open file, unsaved text, and caret. The first EDIT of a run opens CTEXT. A loaded file replaces the held one; quitting drops it. `ALT-L` / `ALT-S` are the load and save icons, not keys |
| Insert vs overwrite, 80-column screen lines, 255-column logical lines, line-break setting | **Missing** | Ordinary editor widget. Lines are not hard-wrapped at 80 on save |
| 40–780 line buffer from `EditBuffer` | **Partial** | Cap exists; the DOS minimum/maximum rules are not enforced in the UI |

### 8 MHeard (ALT-H)

| DOS feature | Status | Notes |
|-------------|--------|-------|
| Live list from the monitor | **Have** | The FROM station of every shown `fm` line, on KISS and in hostmode monitor text, except our own calls. Newest at the top, and opening MHeard starts at the top again |
| `//MHeard` sends the list | **Have** | Bare `//mh` sends the newest 10. `//mh 50` / `//mh 5` set the count. A callsign asks for that station only |
| Enter on a row checks `NAMES.GP` / starts a connect the way the BBS menu does | **Partial** | **CONNECT** uses the selected call on the first free channel and applies a `NAMES.GP` path when that call is listed. Enter on the row still does not connect. The BBS menu’s Enter sends `READ`, which is a different window |
| Fixed 40-entry DOS window | **Partial** | Cap is 500 |

### 9 QSO spy (GP286)

| DOS feature | Status | Notes |
|-------------|--------|-------|
| Up to 10 spy windows, `SpyBuffer`, `MaxSpy` | **Partial** | Ten slots. `MaxSpy` is stored. The pixel UI always offers ten |
| Pick a QSO from MHeard (ALT-S / **SPY**), both directions, two colours, show N(S) | **Partial** | A slot watches one callsign and copies matching monitor lines. No pair detection, no “please select QSO”, no sequence number, no save-this-QSO prompt |
| Kill spy (ALT-K / **KILL SPY**) | **Partial** | Clears the call in the current spy slot. Does not pick a QSO out of MHeard to stop |
| AutoSpy / NoAutoSpy, files `CALL.000`–`.999` in `SpyDir`, close on DISC/DM/UA, skip `ctl UI` | **Partial** | `AutoSpy=` / `NoAutoSpy=` load from config; Spy Setup edits them. Matching lines open a free spy slot and append `data/SPY/CALL.NNN` (next free 000–999 on disk; same call reuses that number until the program exits). Manual SPY still uses `data/SAVE/CALL.spy`. Not yet: auto-close on DISC/DM/UA, or `AutoSpyMinimumSave` |
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
| Remote on/off | **Have**. One global switch for every channel. Off answers a `//RUN` and starts nothing |
| Backup of the scroll buffer (`BACKUP`) | **Have**. When it is on, exit still writes the pads as `data/log/*.txt`. When it is off, that write is skipped |
| Echo | **Have**. Per channel show/hide of our own lines; a channel with no choice of its own follows Monitor. Our lines are always stored, so turning Echo back on brings them back |
| RX click 0 / 1 / 2 | **Have**. How many times `NEWLINE.WAV` plays, one second apart. Default 2. 0 is off. The ♪ button silences it everywhere |
| Connect Bell 0–4 | **Have**. How many times `CONNECT.WAV` plays, each copy as the last ends. 0 is off and also silences `DISCONECT.WAV`. The ♪ button silences both everywhere |
| Line break per channel | **Missing**. Input columns are stored and do not wrap the compose box |
| Packet length for all channels | **Have**. The channel menu sets `packet_payload`, which is the size used when a line is sent |
| Softscroll | **Partial**. The menu and ALT-R cycle the line step. The session button shows a stair when it is on and `/` when it is off (manual 11.12). Chapter 15’s pixel soft-scroll is **DOS-only** |
| 7Plus / LCplus Code AutoSave per channel | **Partial**. The channel menu flips the one **7+** switch for every channel. Live capture into `data/USER` is untested |
| Word wrapping, `*.BAK` | **Partial**. `*.BAK` is copied before an editor save when the toggle is on. Word wrapping is stored and the editor does not hard-wrap yet |
| Insert list from editor, reference channel, file drawer | **Have**. Insert appends DieBox lines from the editor. The reference channel is where the live list is read and where `READ` is sent. The file drawer is the Box-List rubric |
| Beacon menu (monitor only) | **Partial**. Address, text, and period are saved. Period 0 is off. Manual 11.11: `ALL` is `fm CALL to ALL ctl UI^`, and `ALL DB0FN` is `fm CALL to ALL via DB0FN ctl UI^`. The same path is what a line typed on the monitor uses, so `APMI03 WIDE1-1 WIDE2-2` is `to APMI03 via WIDE1-1,WIDE2-2`. On KISS the period sends that one UI frame. The heard copy with `via DB0FN*` is the digipeater’s repeat, not a second send. On hostmode the values are stored and no TNC beacon command is invented |
| InsertMonLine | **Partial**. Stored. Monitor blank lines are not inserted |
| System language | **Have**. Cycles `EN`, `NL`, `DL`, `F`, `PL`, `SLO`, `OK`, `HA` into `remote_language` |
| Session-bar switches: QRG, CT, umlauts, echo, stair or `/`, bell, 7+, remote, BIN-RX, BIN-TX, Name, ScrLock | **Partial**. **CT** is global on Monitor and per channel (green) on a channel; picking the global file there inherits again. **Echo** is per channel. **♪** is global for connect, disconnect, and newline sounds. **Remote** is global. The soft-scroll mark is a drawn stair or `/`. Umlauts are also the setup toggle. **7+** is the Code-AutoSave switch and is untested on the air. **Name** writes `NAMES.GP`. BIN-RX / BIN-TX run the `#BIN#` transfer. ScrLock does not freeze a TNC poll, and it is not per channel |

### 12–18 and the appendices

| DOS feature | Status |
|-------------|--------|
| DOS shell, `EXIT`, `TNCDOS = M N` | **DOS-only** |
| Mouse on icons, menus, channel buttons | **Have** |
| Right-click = Space in CHECK/LIST, drag-select text, scrollbar page clicks, hide scrollbar with ALT-B | **Partial**. A right-click on a BBS or Box-List row marks it. Scrollbar show/hide is the **B** session button. Drag-select is missing |
| `/e` `/v` graphics, `/h` skip hostmode init, `/NOXMS` `/NOEMS`, `/XTKBD` `/ATKBD`, `/3MOUSE`, `/NOFIFO`, `/CHECK` virus scan | **DOS-only** |
| `/KISSOFF` leave KISS at startup | **Partial**. KISS serial applies the close section from `KISS.INI` on exit, not a DOS `/KISSOFF` switch at start |
| Softscroll implementation notes (chapter 15) | **DOS-only** |
| What hostmode is (chapter 16) | **Partial**. WA8DED hostmode driver exists (JHOST1, poll, `WA8DED.INI`). Not yet tested on hardware. When that check happens, also cover Ctrl+PrintScreen TNC-Statistics (keyboard table) on WA8DED and TFPCX |
| Resync / lost V24 characters (chapter 17) | **Missing** as a UI. USB-serial replaces the DOS UART path; a resync banner is not shown. DOS also exposes the running Errors (Resync) count on the Ctrl+PrintScreen TNC-Statistics dialog |
| Licence and author appendices | Not a product feature |

---

## `//` commands

Parsed in `src/remote/commands.rs`. Executed from the pixel UI in
`handle_remote_on_channel`.

| Command | Status |
|---------|--------|
| `//Quit` / `//Q` / `//BYE`, `//DISConnect` | **Have** | `//Quit` (and `//Q` / `//BYE`) sends `QRT.GPI`, then disconnects when the TX queue is empty. `//DISC` drops at once |
| `//Info`, `//NEws`, other `*.GPI` by stem or prefix | **Have** | `//Info` uses `<mycall>.GPI` if present, else the shipped `INFO.GPI`. Other stems need a matching `data/*.GPI` |
| `//VERsion`, `//Echo`, `//Bell`, `//MHeard`, `//Name` | **Have** |
| `//7+`, `//Read`, `//CATalog` / `//DIR` | **Have** | `//7+ ON\|OFF` is the session switch. `//Read file` and `//CAT` / `//DIR` stay inside `data/USER`. A pattern such as `*.txt;*.doc` is a DOS wildcard. The reply says `USER`, not a disk path |
| `//Help`, `//Language` | **Have** | `//Help` and `//H H` use `data/G.GPH` (English) or the pack selected with `//Language` |
| `//PRompt`, `//Usage` | **Have** | Prompt on/off, and this channel’s link state or a `NAMES.GP` lookup |
| `//PARameters` / `//P` | **Have** | TNC timing dump (P-Persistence, Slottime, Frack, T2, T3, TXDelay, MaxUnack). WA8DED re-queries `F` / `@T2` / `@T3` live. KISS applies `T`/`P`/`W` from `init[]` / `WA8DED.INI` as TAPR param frames after bring-up; `//P` reports those (T2/T3 as `?`). TFPCX sends the same lines as hostmode text into the soft-TNC |
| `//CSTATUS` / `//CS` | **Have** | PK-88-style dump of every channel: connected / disconnected / connect or disconnect in progress, peer call, digi path, `IO` on the asking channel |
| `//CD` | **Have** | Shows `USER`. Any other directory is refused, so the remote cannot browse the rest of the disk |
| `//MAIL` | **Have** | Prints `data/MAIL/{CALL}.GPM` for the linked station as text. It does not start a `//Read` file transfer. No file: `You have no mail` |
| `//SEND <call>` | **Have** | The next line is mail for that callsign (`data/MAIL/{CALL}.GPM`, numeric SSID dropped). An existing file gets two blank lines, then `from to call at date time` and the message. The answer is `GPNG: Mail send to CALL` |
| `//Write`, `//END`, `//DELete` / `//Kill` | **Partial** | DOS `//Write` is the 7+ store. This build still captures text into `data/USER` until `//END` or Ctrl-Z. `//DELete` of a named file removes only a file this session stored that way. Bare `//DEL` / `//Kill` deletes `data/MAIL/{CALL}.GPM` for the linked station only |
| `//RUN` | **Have** | Addon below. Bare `//RUN` sends `RUN.GPI` and starts nothing. `//RUN ?` lists `data/EXTERNAL` and the languages found on this computer. `//RUN FREE` reports memory. `//RUN DOS` opens the MS-DOS window. `//name` starts a program that is not already a command. With **Remote** off, a `//RUN` is answered `<GPNG>: Remote commands are turned off by the sysop.` |
| `//CUT` | **Have** | Aborts BIN-RX, BIN-TX, or a 7+ capture on that channel. The receiver sends `#ABORT#` (`#NO#` if the BIN file has not started). The sender stops. Chat resumes |
| `//<n> [text]` / `//0` | **Have** | Bare `//n` joins chat on channel *n*; `//0` leaves. One-shot `//n text` sends a line. Traffic is relayed both ways onto the radio. Room hears `<GPNG>: CALL (Name) Joined/Left your channel!` |
| `//Activity`, `//RPrg`, `//RTt`, `//WPrg`, `//C`, chat `#` | **Missing** | Parsed, then “not implemented” |

`#` as a chat allow-list token in `REMOTE=` is not applied.

---

## Addons

GPNG additions. An operator can use them. They are not a DOS gap.

### EXTERNAL programs (`//RUN`)

**Have.** `//RUN name` starts one text program from `data/EXTERNAL`. `//name` does the same when that name is not already a command. Bare `//RUN` starts nothing and sends `data/RUN.GPI`. `//RUN ?` lists the folder and which languages are installed on this computer. **Remote** is one global switch: off answers a `//RUN` with `<GPNG>: Remote commands are turned off by the sysop.` and starts nothing.

The samples are `FREE` and `DOS`. `//RUN FREE` (and `//FREE`) prints memory total, used, and free, reads no input, and exits. `//RUN DOS` (and `//DOS`) opens an MS-DOS 6.20 window whose drive `C:` is `data/USER`. Inside it: `DIR` (switches `/W /B /L /S /AD /A-D /O:…`, also attached, as in `dir/w` and `dir *.txt/ad`), `CD`, `ECHO text`, `TYPE name`, `HELP`, and `EXIT` in any letter case. Another drive letter answers `Drive D does not excist!` (with that letter). Nothing outside `data/USER` can be reached.

The languages are Python, PHP, Perl, Ruby, Node, Lua, and Java. The first line may name the program, for example `#! /usr/bin/env python3`, `#! /usr/bin/perl`, `#! /usr/lib/cgi-bin/php7.2`, or `#! /usr/bin/env java --source 11`. GPNG reads that line on Linux, macOS, and Windows and starts the language if it is installed. A Linux path that is not on this computer falls back to the same program on `PATH`. A `.py`, `.php`, `.pl`, `.rb`, `.js`, `.lua`, `.java`, or `.jar` name is enough when there is no such line. Java is started headless, so it cannot open a window. A shell, a batch file, and a Windows program are refused. The command cannot name a file outside `data/EXTERNAL`, and the other station cannot upload the program.

Printed text goes back to the station that started it. If the program reads a line, the next text from that connection is the line, and that same process stays open while it waits. A packet for a running program is handed to it as one line at once, so a command that arrives without a following Enter (an `exit` is the usual one) is not held until the next packet. A second `//RUN` on that channel answers `program is running` and does not start another process. When the script exits, the process is closed at once. A run that is still printing, or still being typed to, stays open. If 300 seconds pass with no typed line and no new text, it is closed, and a station that is still connected is told `closed run program after timeout`. Disconnect closes it too. Ten channels is ten programs at most.

This is the stand-in for the DOS GPRI Pascal interface. A DOS program in `EXTERNAL` is not run. Bare `//RUN` sends `RUN.GPI`; a named `//RUN` starts the program.

---

## Already in place (so this list stays honest)

- Pixel layout: channel grid, analog clock with hourly gong, status grid, monitor and channel pads, compose history, icon strip from `ICONS.GP`, session bar.
- KISS AX.25: UI monitor, SABM/UA/DM connect, I + RR/REJ, DISC/UA teardown with T1, T3 idle poll with T1 retry, digi path on replies, self-connect between two local channels, TCP KISS (Soundmodem / Direwolf) and serial KISS. A DISC addressed to another station is monitored only; it is not answered and does not tear down our link.
- Hostmode drivers for WA8DED and TFPCX (untested on a real TNC).
- Per-channel callsigns, connect and disconnect, connect text with `%` macros. Names and the autorouter come from `data/NAMES.GP` (`//Name`, channel **Name**). MHeard is heard stations only.
- Manual spy slots that follow a callsign on the monitor.
- Editor load/save in CP437.
- Pad logs under `log/` for monitor, channels, spy, BBS, and box.
- Remote command catalogue and macro expander. Cross-channel `//n` / `//0` chat mode (both-way RF relay, Joined/Left notices). `//Quit` / `//Q` / `//BYE` sends `QRT.GPI`. A few remotes are still stubs (`//Activity`, `//RPrg`, `//RTt`, `//WPrg`, `//C`).
- `//RUN` addon: one Python, PHP, Perl, Ruby, Node, Lua, or Java program from `data/EXTERNAL` per channel. Shebang on Linux, macOS, and Windows. Java is headless. Input or new output keeps that process; 300 seconds with neither, exit, or disconnect closes it. A linked station is told `closed run program after timeout`. Bare `//RUN` sends `RUN.GPI`. Samples: `FREE` and the `DOS` window on `data/USER`.
- Sounds and session switches: RX-Click counts `NEWLINE.WAV`, Connect Bell counts `CONNECT.WAV` (off also silences `DISCONECT.WAV`), ♪ silences connect/disconnect/newline everywhere, the clock strikes `CHIME.WAV` on a 12-hour clock, **CT** and **Echo** can be set per channel, and **Remote** is one global switch.
- AutoBin on a live KISS link with DOS GP 1.63 (`#BIN#` / `#OK#` / `#ABORT#`, file in `data/SAVE`). 7Plus Code-AutoSave is implemented and untested. See `docs/TRANSFER.md`.
- Setup menu (ALT-U / **SETUP**). Rows follow the current window. Colours, the KISS beacon, and the Esc TNC box are live. Settings go to `Config.toml`.
- DieBox BBS menu and Box list: mark, `READ`, find, sort, and the replace / append / interrupt question. Other mailbox formats are not parsed.
- Session soft-scroll button: `/` or a stair. The text still steps a whole line.

---

## Suggested build order

1. 7Plus Code-AutoSave stays untested until a real `go_7+.` part is on the link. The later GPNG-only `#7+#` connect handshake is not this item. AutoBin on the channel is in. Still open: `//RPrg`, `//WPrg`, and resume. `//CUT` aborts a running BIN or 7+ transfer.
2. `NAMES.GP` plus autorouter on Connect, then ALT-N. **Done.** Connect uses `data/NAMES.GP`. **Name** is ALT-N. A type `B` mailbox still does not block remotes, and `PORTx` does not change the radio.
3. BBS-Menu and Box-List parsers (DieBox first), mark, and `READ`. **Done** for DieBox `CHECK` and `LIST` on the setup reference channel. F6FBB, DK5SG, and DB0IE are still ahead.
4. Finish `//` remotes that only need files: help (`*.GPH`), catalog, delete, prompt, parameters, usage, language. **Done.** Help follows `data/G.GPH` (English) and the other language packs. Catalog, read, write, and delete stay in `data/USER`. `//CD` cannot leave that folder. Cross-channel `//n` / `//0` chat mode and `//BYE` as a Quit alias are **Done**. AutoBin `//RPrg` / `//WPrg` remain with item 1.
5. Journal (`LOG`, `QRG`, `UTC`, `<mycall>.GPL`) and the sysop `*.GPW` helper. **Done** for the journal and for TheNet, BayCom, and FlexNet. Shell and debug lines go to `data/log/gpng.log`. The DieBox 60×27 grid is not calculated.
6. Setup menu. **Done** for the DOS rows that have a live effect: colours, prompt, remote (one global switch), backup, echo (per channel show/hide, lines always stored), RX-Click (a count of `NEWLINE.WAV`), Connect Bell (a count of `CONNECT.WAV`; off also silences `DISCONECT.WAV`), umlauts, the stair / `/` line step, packet length, Code-AutoSave, language, editor `*.BAK`, insert-from-editor, the reference channel, the file drawer, the KISS beacon (manual 11.11, also the monitor unproto path), and the Esc TNC command box. The ♪ button is the global connect/disconnect/newline mute, **CT** can hold a file per channel, and EDIT keeps its file, text, and caret until another file is loaded. Word wrapping, input columns, and InsertMonLine are stored. Hostmode does not send the beacon. A `//` line in the Esc box is not the operator remote from manual 5.5. Settings go to `Config.toml`, not `CSTAT.GPB`.
7. Pixel-UI hotkeys that match DOS GP chapter 4. **Done** for the keys that already have an action: F1–F12, ALT-M/E/X/U/O/R/G/Q/C/D/Y/N/T/B, ALT-F1/F2/F4/F6/F7/F8/F9/F10, Shift-F1–F12, Ctrl-D/T/Y/C/P/O/B, Ctrl-Enter, Esc (TNC command, or back to Monitor from BBS/List), and editor ALT-L/S. Still ahead: ALT-S continuous save, ALT-F file send, Insert/overwrite, Ctrl-K, and Ctrl+PrintScreen TNC-Statistics (hostmode / TFPCX only, not KISS). ALT-Z stays DOS VGA zoom. On the BBS menu ALT-S sorts and ALT-F finds; on the Box list ALT-F finds.
8. Still ahead, in short: the other mailbox list formats (F6FBB, DK5SG, DB0IE) and rubric sections; a live 7Plus part on the air, `//RPrg` / `//WPrg`, transfer resume and ALT-T / ALT-S / ALT-F; a type `B` mailbox blocking remotes and `PORTx` switching a multiport TNC; the `REMOTE=` allow-list; editor Insert/overwrite, hard word wrap, and Ctrl-K; AutoSpy close on DISC/DM/UA; stub remotes `//Activity`, `//RTt`, `//C`; Ctrl+PrintScreen TNC-Statistics when WA8DED / TFPCX are checked on hardware or soft-TNC. The `//RUN` samples (`FREE`, `DOS`), bare `//RUN` → `RUN.GPI`, cross-channel `//n` chat, `//BYE`, Auto Spy / Spy Auto Ignore setup with `CALL.NNN` files, the global **Remote** answer, and the per-channel **CT** / **Echo** switches are done.
