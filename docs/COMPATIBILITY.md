# Remote commands & macros — compatibility catalogue

Amateur operators learned GP by muscle memory: `//VERsion`, `//MHeard`,
`%V` in `CTEXT.GPI`, chat `#`, digi `%DIGI%`, etc. Missing even obscure
aliases breaks that feel. Documentation is incomplete and language packs
disagree, so we treat **compatibility recovery** as a first-class task.

## What we need vs what “full RE” means

| Goal | Preferred source | When binary help is useful |
|------|------------------|----------------------------|
| Command *names* & abbreviations | Union of `*.GPH` `@TAG` sections + `README.TXT` | Confirm tokens / aliases not listed in one language |
| Macro *letters* (`%V`, `%N#20`) | README “fifteen macros” + GPI samples | Rare tokens encoded oddly; confirm extras like `%DIGI%` |
| Exact *behaviour* | Docs + try under DOSBox against real TNC/TFPCX | Ambiguous edge cases only |
| BGI drawing / internal structs | **Not required** for remotes/macros | Avoid — licence forbids modifying the EXE; clean-room rewrite |

So: **targeted string / help-file archaeology**, not a full decompile of `GP2862K.EXE`, is the right tool for `//` and `%`.

## Recovery pipeline used here

1. Parse every `*.GPH` (`DL`, `G`, `F`, `NL`, `PL`, `OK`, `HA`, `SLO`, …) — each `@COMMAND,minlen` block is the author-maintained remote surface for that language.
2. Merge with `README.TXT` / `GP.DOC` (macros, `Remote=` / `NoRemote=`, `//C`, `//Kill`, chat `#`).
3. Confirm with printable strings from `GP2862K.EXE` (e.g. `//C `, `//END`, `KILL`, `PARAMETERS`, `Graphic Packet V1.63`).
4. Encode the union in `src/remote/` so the modern binary implements one authoritative table.
5. Only if a command’s *semantics* remain unclear: observe DOSBox + Hostmode traces — still no need to ship decompiled Pascal/BGI.

## Findings (union)

### GPI / CTEXT macros (documented “fifteen” + extras)

| Macro | Expansion |
|-------|-----------|
| `%V` | Software version (EXE banner: **1.63** on `GP2862K`) |
| `%C` | Opposite station call |
| `%N` | Opposite station name (MHeard disk list); callsign if none |
| `%Y` | Own call (channel MYCALL) |
| `%K` | Channel number |
| `%T` | GP clock `HH:MM:SS` |
| `%D` | Date |
| `%B` | Bell (`0x07`) |
| `%I` | News hint if `NEWS.GPI` present |
| `%Z` | Timezone string (`ZONE=`) |
| `%_` | CR/LF |
| `%%` | Literal `%` |
| `%O` | Random line from `ORIGIN.GPI` |
| `%?` | Prompt for name if unknown |
| `%>` | Enable remote prompt (CTEXT) |
| `%N#nn` | Width pad (any macro may take `#` + 2-digit width) |
| `%DIGI%` | Digi path substitution (DEST.GP / routing texts) |
| `%TFPCX200` | Config `<IF>` keyword (not a GPI text macro) |

### `//` remote commands (abbreviation-aware)

Minimum significant letters come from `@TAG,N` in `*.GPH` (e.g. `@VERSION,3` → `VER` matches `VERsion`).

| Canonical | Min. prefix (typical) | Notes |
|-----------|----------------------|--------|
| `//<n> <text>` | digit | Cross-channel chat to port/channel *n* |
| `//7+ <ON/OFF>` | `7+` | 7Plus AutoSave |
| `//Activity` | `AC…` | Last sysop key/mouse activity |
| `//Bell` | `B` | Ring sysop |
| `//CATalog` / `//DIRectory` | `CAT` / `DIR` | UserDir listing; wildcards; `a:b` multi-filter |
| `//CD <dir>` | `CD` | **DL/SLO help** — change/show UserDir (missing from some EN lists) |
| `//CUT` | `CUT` | Abort `//Read` transfer |
| `//DELete` / `//Kill` | `DEL` / `KILL` | Delete own `//Write` file; bare `//DEL`/`//Kill` clears `.GPM` mail |
| `//DISConnect` | `DISC` | Drop without QRT text |
| `//Echo <text>` | `E` | Echo; used by `//RTT` |
| `//END` | `END` | Finish `//Write` capture (or Ctrl-Z) |
| `//Help [cmd]` | `H` | Serve `*.GPH` |
| `//Info` | `I` | `<mycall>.GPI` / `INFO.GPI` |
| `//Language <lang>` | `L` | Switch remote `*.GPT` / help language |
| `//MHeard [n\|call]` | `MH` | MH list or last-heard for call |
| `//Name <name>` | `N` | Store opposite station name on the MHeard disk list (not shown in the pane) |
| `//NEws` | `NE` | `NEWS.GPI` |
| `//PARameters` | `PAR` | TNC/system parameters (**DL/F** help) |
| `//PRompt <ON/OFF>` | `PR` | Remote prompt (`PROMPT=` template) |
| `//Quit` | `Q` | QRT.GPI then disconnect |
| `//Read <file>` | `R` | Send text file from UserDir |
| `//RPrg <file>` | `RP` | AutoBin send |
| `//RTt <port>` | `RT` | RTT via peer `//Echo` |
| `//RUN <prog>` / `//<prog>` | `RUN` | EXTERNAL / GPRI; `//RUN ?` lists |
| `//Usage [call]` | `U` | Connected stations / call info |
| `//VERsion` | `VER` | Version string |
| `//Write <file>` | `W` | Capture following text to UserDir |
| `//WPrg <file>` | `WP` | AutoBin receive/store |
| `//C <path…>` | `C` | Remote-initiated connect (netrom/node style) — docs + EXE string |
| `#` … | `#` | Chat mode allow-list token in `Remote=` |

`Remote=` / `NoRemote=` accept comma-separated prefixes or `*` (allow all).

## Gaps still worth DOSBox / deeper probes

- Exact min-length table for every alias (HA/OK use very short forms in examples).
- Full `//PARameters` field list.
- Chat `#` line protocol details.
- Whether `%` macros beyond the fifteen + `%DIGI%` exist as single-byte tokens inside the EXE (ASCII `%V` barely appears — expansion is likely table-driven).

When probing the binary further, prefer **strings + behavioural tests** over shipping decompiled code.
