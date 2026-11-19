"""One-shot: convert data/*.GPH into locales/*.toml. Not used at runtime."""

from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "data"
OUT = ROOT / "locales"

MAP = {
    "G.GPH": ("en", "English"),
    "NL.GPH": ("nl", "Dutch"),
    "DL.GPH": ("de", "German"),
    "F.GPH": ("fr", "French"),
    "PL.GPH": ("pl", "Polish"),
    "SLO.GPH": ("sl", "Slovenian"),
    "OK.GPH": ("cs", "Czech"),
    "HA.GPH": ("hu", "Hungarian"),
}

SETUP_EN = {
    "theme": "Theme Setup",
    "prompt": "Remote control prompt",
    "remote": "Remote control",
    "backup": "Backup text buffers",
    "echo": "Echo",
    "rx_click": "RX-Click",
    "connect_bell": "Connect Bell",
    "umlaut": "Umlaut conversion",
    "input_columns": "Input columns",
    "insert_mon": "InsertMonLine",
    "softscroll": "Softscroll",
    "beacon": "Beacon parameters",
    "language": "System language",
    "zoom": "Zoom",
    "icons": "System Icons",
    "packet_len": "Packet length",
    "auto_save": "Code-AutoSave",
    "word_wrap": "Word wrapping",
    "editor_bak": "Create *.BAK File",
    "ref_channel": "Reference channel",
    "insert_from_editor": "Insert list from editor",
    "file_drawer": "File drawer",
    "auto_spy": "Auto Spy",
    "spy_auto_ignore": "Spy Auto Ignore",
    "max_spy": "Number of spies",
    "mheard_clean": "Clear MHeard older than (days)",
    "mheard_clear": "Clear MHeard List",
    "use": "Use these values",
}


def decode(raw: bytes) -> str:
    if b"\x1a" in raw:
        raw = raw.split(b"\x1a", 1)[0]
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError:
        return raw.decode("cp437", errors="replace")


def tag_header(line: str):
    t = line.strip()
    if not t.startswith("@") or t.startswith("@@"):
        return None
    rest = t[1:]
    if "," in rest:
        tag, n = rest.split(",", 1)
    else:
        tag, n = rest, "1"
    tag = tag.strip()
    if not tag or not all(c.isalnum() or c in "+#" for c in tag):
        return None
    try:
        mn = max(1, int(n.strip()))
    except ValueError:
        mn = 1
    return tag.upper(), mn


def parse_gph(text: str):
    lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    i = 0
    while i < len(lines) and not lines[i].strip().startswith("@@"):
        i += 1
    if i < len(lines) and lines[i].strip().startswith("@@"):
        i += 1
    overview: list[str] = []
    while i < len(lines):
        if tag_header(lines[i]) is not None:
            break
        if lines[i].strip().startswith("@@"):
            break
        overview.append(lines[i])
        i += 1
    sections: list[tuple[str, int, str]] = []
    while i < len(lines):
        th = tag_header(lines[i])
        if th is None:
            break
        tag, mn = th
        i += 1
        body: list[str] = []
        while (
            i < len(lines)
            and tag_header(lines[i]) is None
            and not lines[i].strip().startswith("@@")
        ):
            body.append(lines[i])
            i += 1
        sections.append((tag, mn, "\n".join(body)))
    return "\n".join(overview).rstrip() + "\n", sections


def toml_multiline(s: str) -> str:
    body = s.rstrip("\n")
    if "'''" not in body:
        return "'''\n" + body + "\n'''"
    escaped = body.replace("\\", "\\\\").replace('"""', '\\"""')
    return '"""\n' + escaped + '\n"""'


def key_for(tag: str) -> str:
    if tag.isidentifier() and tag.isascii() and tag[0].isalpha():
        return tag
    return f'"{tag}"'


def write_setup(lines: list[str]) -> None:
    lines.append("[setup]")
    for k, v in SETUP_EN.items():
        lines.append(f'{k} = "{v}"')
    lines.append("")


def main() -> None:
    OUT.mkdir(exist_ok=True)
    prefixes: dict[str, int] = {}

    for fname, (iso, name) in MAP.items():
        text = decode((DATA / fname).read_bytes())
        overview, sections = parse_gph(text)
        if iso == "en":
            for tag, mn, _ in sections:
                prefixes[tag] = mn

        lines: list[str] = []
        lines.append("[meta]")
        lines.append(f'name = "{name}"')
        lines.append("")
        if iso == "en":
            write_setup(lines)
        lines.append("[help]")
        lines.append(f"overview = {toml_multiline(overview)}")
        lines.append("")
        lines.append("[help.commands]")
        for tag, _mn, body in sections:
            store_tag = "PARAMETERS" if tag == "PAR" else tag
            lines.append(f"{key_for(store_tag)} = {toml_multiline(body)}")
        if iso == "en":
            lines.append("")
            lines.append("[help.prefix]")
            for tag, mn in sorted(prefixes.items()):
                lines.append(f"{key_for(tag)} = {mn}")
        out = OUT / f"{iso}.toml"
        out.write_text("\n".join(lines) + "\n", encoding="utf-8")
        print(f"wrote {out.name}: {len(sections)} sections")

    es_lines: list[str] = ["[meta]", 'name = "Spanish"', ""]
    write_setup(es_lines)
    es_lines.append("# Help and missing keys fall back to English.")
    (OUT / "es.toml").write_text("\n".join(es_lines) + "\n", encoding="utf-8")
    print("wrote es.toml stub")


if __name__ == "__main__":
    main()
