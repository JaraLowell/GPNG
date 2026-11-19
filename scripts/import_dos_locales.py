"""Import missing/updated locales from D:\\GPDos\\GP\\USER\\GP\\tmp GPH packs."""

from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "locales"
DOS_TMP = Path(r"D:\GPDos\GP\USER\GP\tmp")

# Absolute GPH path → (iso, English name)
SOURCES: list[tuple[Path, str, str]] = [
    (DOS_TMP / "ea_lang" / "E.GPH", "es", "Spanish"),
    (DOS_TMP / "la_lang" / "LA.GPH", "no", "Norwegian"),
    (DOS_TMP / "lz_lang" / "BG.GPH", "bg", "Bulgarian"),
    (DOS_TMP / "py_gpt" / "PY.GPH", "pt", "Portuguese"),
    (DOS_TMP / "ra_lang" / "RUS.GPH", "ru", "Russian"),
    # Accented Hungarian (HG) replaces the ASCII-only HA pack we had.
    (DOS_TMP / "ha_lang" / "HG.GPH", "hu", "Hungarian"),
]


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


def main() -> None:
    OUT.mkdir(exist_ok=True)
    for path, iso, name in SOURCES:
        if not path.is_file():
            print(f"MISSING {path}")
            continue
        overview, sections = parse_gph(decode(path.read_bytes()))
        lines: list[str] = [
            "[meta]",
            f'name = "{name}"',
            "",
            "[help]",
            f"overview = {toml_multiline(overview)}",
            "",
            "[help.commands]",
        ]
        for tag, _mn, body in sections:
            store_tag = "PARAMETERS" if tag == "PAR" else tag
            if store_tag == "CSTATUS" or store_tag == "CStatus".upper():
                store_tag = "CSTATUS"
            lines.append(f"{key_for(store_tag)} = {toml_multiline(body)}")
        out = OUT / f"{iso}.toml"
        out.write_text("\n".join(lines) + "\n", encoding="utf-8")
        print(f"wrote {out.name} from {path.name}: {len(sections)} sections")


if __name__ == "__main__":
    main()
