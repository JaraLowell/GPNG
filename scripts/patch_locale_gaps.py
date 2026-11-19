"""Add GPNG help topics missing from non-English locale packs.

Re-imports Spanish from DOS E.GPH (fixes ESC corruption), then merges
missing `help.commands` and refreshes RUN/CUT/READ/LANGUAGE from translated
GPNG text. Overview lines for the new commands are inserted when absent.
"""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "data" / "LOCALES"
DOS_E = Path(r"D:\GPDos\GP\USER\GP\tmp\ea_lang\E.GPH")

# Keys that every pack should have (from en.toml GPNG surface).
REQUIRED = [
    "7+",
    "7DECODE",
    "ACTIVITY",
    "BELL",
    "CATALOG",
    "CSTATUS",
    "CUT",
    "DELETE",
    "DIRECTORY",
    "DISCONNECT",
    "ECHO",
    "END",
    "HELP",
    "INFO",
    "LANGUAGE",
    "MAIL",
    "MHEARD",
    "N",
    "#",
    "NAME",
    "NEWS",
    "PARAMETERS",
    "PROMPT",
    "QUIT",
    "READ",
    "RPRG",
    "RTT",
    "RUN",
    "SEND",
    "USAGE",
    "VERSION",
    "WPRG",
    "WRITE",
]

# Always refresh these with GPNG wording (translated below).
REFRESH = {"RUN", "CUT", "READ", "LANGUAGE", "7DECODE", "MAIL", "SEND", "CSTATUS", "N", "#", "PARAMETERS"}

# Overview lines to ensure (English command forms; matching en.toml).
OVERVIEW_LINES = [
    "  //# [Text]",
    "  //7+ <ON/OFF>",
    "  //7DECODE <file>",
    "  //Activity",
    "  //Bell",
    "  //CATalog [wildcard]",
    "  //CSTATUS",
    "  //CUT",
    "  //DELete",
    "  //DIRectory [wildcard]",
    "  //DISConnect",
    "  //Echo <text>",
    "  //END",
    "  //Help [command]",
    "  //Info",
    "  //Language <language>",
    "  //MAil",
    "  //MHeard [number]",
    "  //MHeard <call>",
    "  //Name <name>",
    "  //NEws",
    "  //PARameters",
    "  //PRompt <ON/OFF>",
    "  //Quit",
    "  //Read <filename>",
    "  //RPrg <filename>",
    "  //RTt <portnr>",
    "  //RUn <program>",
    "  //SEnd <call>",
    "  //Usage [call]",
    "  //VERsion",
    "  //Write <filename>",
    "  //WPrg <filename>",
]


def decode(raw: bytes) -> str:
    raw = raw.replace(b"\x1b", b"")
    if b"\x1a" in raw:
        raw = raw.split(b"\x1a", 1)[0]
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError:
        return raw.decode("cp437", errors="replace")


def tag_header(line: str):
    t = line.strip().lstrip("\x1b")
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
    sections: dict[str, str] = {}
    order: list[str] = []
    while i < len(lines):
        th = tag_header(lines[i])
        if th is None:
            break
        tag, _mn = th
        i += 1
        body: list[str] = []
        while (
            i < len(lines)
            and tag_header(lines[i]) is None
            and not lines[i].strip().startswith("@@")
        ):
            body.append(lines[i])
            i += 1
        if tag == "PAR":
            tag = "PARAMETERS"
        if tag not in sections:
            order.append(tag)
        sections[tag] = "\n".join(body).rstrip("\n") + "\n"
    return "\n".join(overview).rstrip() + "\n", sections, order


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


def parse_locale_toml(text: str) -> tuple[dict, dict[str, str], str]:
    """Minimal parser: meta name, setup table, help.overview, help.commands."""
    meta: dict = {}
    setup: dict[str, str] = {}
    commands: dict[str, str] = {}
    overview = ""
    # Strip ESC leftovers
    text = text.replace("\x1b", "")
    # Extract [meta] name
    m = re.search(r'(?m)^name\s*=\s*"([^"]*)"', text)
    if m:
        meta["name"] = m.group(1)
    # setup keys (simple quoted)
    for m in re.finditer(
        r'(?m)^([a-z_]+)\s*=\s*"([^"]*)"',
        text[text.find("[setup]") : text.find("[help]")] if "[setup]" in text else "",
    ):
        setup[m.group(1)] = m.group(2)
    # overview
    om = re.search(
        r"(?ms)^overview\s*=\s*'''(.*?)'''",
        text,
    )
    if not om:
        om = re.search(r'(?ms)^overview\s*=\s*"""(.*?)"""', text)
    if om:
        overview = om.group(1)
        if overview.startswith("\n"):
            overview = overview[1:]
    # commands: KEY = '''...''' or "KEY" = '''...'''
    cmd_block = text
    if "[help.commands]" in text:
        cmd_block = text.split("[help.commands]", 1)[1]
        if "[help.prefix]" in cmd_block:
            cmd_block = cmd_block.split("[help.prefix]", 1)[0]
    for m in re.finditer(
        r"(?ms)^(?:\"([^\"]+)\"|([A-Za-z0-9+#]+))\s*=\s*'''(.*?)'''",
        cmd_block,
    ):
        tag = (m.group(1) or m.group(2)).upper()
        body = m.group(3)
        if body.startswith("\n"):
            body = body[1:]
        # Drop accidental embedded @TAG headers left from bad imports
        body = re.sub(r"(?m)^@+[A-Za-z0-9+#]+,\d+\s*$", "", body)
        commands[tag] = body
    for m in re.finditer(
        r'(?ms)^(?:"([^"]+)"|([A-Za-z0-9+#]+))\s*=\s*"""(.*?)"""',
        cmd_block,
    ):
        tag = (m.group(1) or m.group(2)).upper()
        if tag in commands:
            continue
        body = m.group(3)
        if body.startswith("\n"):
            body = body[1:]
        commands[tag] = body
    return meta, setup, commands, overview


# --- GPNG section translations (command names stay English) -----------------

GPNG: dict[str, dict[str, str]] = {}

GPNG["en"] = {
    "7DECODE": """
  //7DECODE <file>
      Joins 7Plus part files already stored in USER. <file> may be a part
      name (README.P01) or the original name (README.TXT). The first part
      supplies the part count. Missing parts are reported; when every part
      is present the original file is written and the parts are deleted.
""",
    "MAIL": """
  //MAIL
      Prints the mail waiting for your callsign. This is not a file transfer.
      //Read sends a file. A numeric SSID is the same mailbox, so
      NL1STK-2 reads NL1STK. If there is no mail the answer is:
      You have no mail
""",
    "SEND": """
  //SEND <call>
      The next line is a mail message for that station. A numeric SSID
      is ignored, so NL1STK-2 is NL1STK. The line is stored as that
      call's mail. If mail is already there, two blank lines are
      written and the new message is added:
      <from> to <call> at <date> <time>, then the message.
      The answer is: GPNG: Mail send to <call>
""",
    "CSTATUS": """
  //CSTATUS
      Lists only channels that have a connection. The first line is
      Channel state, then the date, time, and timezone (%D %T %Z).
      Each link is one line: this station's call, an arrow, and the
      other station. <-- means they called in. --> means this station
      called out.
             Chan   1: NL1STK     <--  NL0MSK
""",
    "N": """
  //<n> [text]
      Where n is a number. Bare //n joins chat on channel n. Everything
      you type is sent to that channel, and text from that channel (and
      other joiners) is sent back to you on the air. The room is told
      "<GPNG>: CALL (Name) Joined your channel!". //0 leaves and announces
      Left. With text, //n sends one line only and does not stay in chat
      mode. Target must be a connected TERM channel.
""",
    "#": """
  //# [text]
      Where # is a number. Bare //# joins chat on channel #. Everything
      you type is sent to that channel, and text from that channel (and
      other joiners) is sent back to you on the air. The room is told
      "<GPNG>: CALL (Name) Joined your channel!". //0 leaves and announces
      Left. With text, //# sends one line only and does not stay in chat
      mode. Target must be a connected TERM channel.
""",
    "PARAMETERS": """
  //PARameters
  //P
      Queries TNC timing parameters (P-Persistence, Slottime, Frack,
      T2, T3, TXDelay, MaxUnack). On WA8DED the values are read live from
      the TNC; otherwise the last known or WA8DED.INI / init[] values.
""",
    "RUN": """
  //RUN <program>
      Runs a program from the EXTERNAL folder and sends what it prints
      back on this link. //RUN ? lists the programs. //<program> is the
      same when that name is not already a command.

      A program is a text script the sysop placed in EXTERNAL: Python,
      PHP, Perl, Ruby, Node, Lua, or Java. Java cannot open a window.
      The first line may name the program, for example
      #! /usr/bin/env python3, #! /usr/bin/perl, or
      #! /usr/lib/cgi-bin/php7.2.
      That line is read on Linux, macOS, and Windows. The language must
      be installed on the sysop computer. Words after the name are
      arguments, not a path.

      //RUN FREE
          Memory total, used, and still free.

      If the script reads a line, the next text from this connection is
      that line, and the program stays open while it waits. A second
      //RUN answers "program is running". When the script exits, it is
      closed. It also closes after 300 seconds with no typed line
      and no new text. A station still connected is told
      "closed run program after timeout".
      One program per channel. It also stops when the link drops.
""",
    "CUT": """
  //CUT
      Aborts the file transfer on this channel and returns to chat.
      BIN-RX, or a 7+ capture, sends #ABORT# (#NO# if the BIN file has
      not started). BIN-TX stops. A sender that has already sent file
      bytes also sends #ABORT# so the receiver leaves the file.
""",
    "READ": """
  //Read <filename>
      This command sends you a text file. You can get a list of all readable
      files by sending the command "//CATalog".
      Mail is //MAIL, not this file transfer.
""",
    "LANGUAGE": """
  //Language <language>
      Changes the UI and remote help language (ISO codes such as en, de, nl).
      A call without parameters returns a list of all supported languages.
""",
}


def tr(lang: str, key: str, text: str) -> None:
    GPNG.setdefault(lang, {})[key] = text


# German
tr(
    "de",
    "7DECODE",
    """
  //7DECODE <file>
      Fuegt 7Plus-Teildateien zusammen, die bereits in USER liegen. <file>
      kann ein Teilname (README.P01) oder der Originalname (README.TXT) sein.
      Der erste Teil liefert die Anzahl. Fehlende Teile werden gemeldet;
      sind alle da, wird die Originaldatei geschrieben und die Teile geloescht.
""",
)
tr(
    "de",
    "MAIL",
    """
  //MAIL
      Zeigt die Mail fuer Ihr Rufzeichen. Das ist keine Dateiuebertragung.
      //Read sendet eine Datei. Eine numerische SSID gehoert zum gleichen
      Postfach, NL1STK-2 liest also NL1STK. Ohne Mail lautet die Antwort:
      You have no mail
""",
)
tr(
    "de",
    "SEND",
    """
  //SEND <call>
      Die naechste Zeile ist eine Mail an diese Station. Eine numerische
      SSID wird ignoriert (NL1STK-2 = NL1STK). Die Zeile wird als Mail
      gespeichert. Bei bestehender Mail folgen zwei Leerzeilen und die
      neue Nachricht: <from> to <call> at <date> <time>, dann der Text.
      Antwort: GPNG: Mail send to <call>
""",
)
tr(
    "de",
    "CSTATUS",
    """
  //CSTATUS
      Listet nur Kanaele mit Verbindung. Erste Zeile: Channel state, dann
      Datum, Zeit und Zeitzone (%D %T %Z). Jede Verbindung eine Zeile:
      eigenes Rufzeichen, Pfeil, Gegenstation. <-- Angerufen. --> selbst
      herausgerufen.
             Chan   1: NL1STK     <--  NL0MSK
""",
)
tr(
    "de",
    "N",
    """
  //<n> [text]
      n ist eine Nummer. Alleiniges //n tritt dem Chat auf Kanal n bei.
      Alles Tippen geht dorthin; Text von diesem Kanal (und anderen
      Teilnehmern) kommt zurueck. Der Raum sieht
      "<GPNG>: CALL (Name) Joined your channel!". //0 verlaesst und meldet
      Left. Mit Text sendet //n nur eine Zeile ohne Chat-Modus.
      Ziel muss ein verbundener TERM-Kanal sein.
""",
)
tr(
    "de",
    "#",
    """
  //# [text]
      # ist eine Nummer. Alleiniges //# tritt dem Chat auf Kanal # bei.
      Alles Tippen geht dorthin; Text von diesem Kanal (und anderen
      Teilnehmern) kommt zurueck. Der Raum sieht
      "<GPNG>: CALL (Name) Joined your channel!". //0 verlaesst und meldet
      Left. Mit Text sendet //# nur eine Zeile ohne Chat-Modus.
      Ziel muss ein verbundener TERM-Kanal sein.
""",
)
tr(
    "de",
    "PARAMETERS",
    """
  //PARameters
  //P
      Fragt TNC-Zeitparameter ab (P-Persistence, Slottime, Frack, T2, T3,
      TXDelay, MaxUnack). Bei WA8DED live vom TNC; sonst letzte bekannte
      oder WA8DED.INI / init[]-Werte.
""",
)
tr(
    "de",
    "RUN",
    """
  //RUN <program>
      Startet ein Programm aus EXTERNAL und sendet dessen Ausgabe auf
      dieser Verbindung. //RUN ? listet Programme. //<program> geht auch,
      wenn der Name kein anderer Befehl ist.

      Skripte in EXTERNAL: Python, PHP, Perl, Ruby, Node, Lua oder Java.
      Java ohne Fenster. Erste Zeile kann z.B. sein
      #! /usr/bin/env python3, #! /usr/bin/perl oder
      #! /usr/lib/cgi-bin/php7.2 (Linux, macOS, Windows). Die Sprache muss
      auf dem Sysop-Rechner installiert sein. Weitere Worte sind Argumente.

      //RUN FREE
          Speicher gesamt, belegt und frei.

      Liest das Skript eine Zeile, ist der naechste Text von dieser
      Verbindung diese Zeile. Zweites //RUN: "program is running". Ende
      oder 300 s ohne Eingabe/Ausgabe beendet es; verbunden bleibt
      "closed run program after timeout". Ein Programm pro Kanal; Link
      weg beendet ebenfalls.
""",
)
tr(
    "de",
    "CUT",
    """
  //CUT
      Bricht die Dateiuebertragung auf diesem Kanal ab und kehrt zum Chat
      zurueck. BIN-RX oder 7+-Capture sendet #ABORT# (#NO#, wenn BIN noch
      nicht gestartet). BIN-TX stoppt. Ein Sender, der schon Bytes gesendet
      hat, sendet ebenfalls #ABORT#, damit der Empfaenger die Datei verlaesst.
""",
)
tr(
    "de",
    "READ",
    """
  //Read <filename>
      Sendet eine Textdatei. Liste aller lesbaren Dateien mit "//CATalog".
      Mail ist //MAIL, keine Dateiuebertragung.
""",
)
tr(
    "de",
    "LANGUAGE",
    """
  //Language <language>
      Wechselt UI- und Hilfe-Sprache (ISO-Codes wie en, de, nl).
      Ohne Parameter: Liste aller unterstuetzten Sprachen.
""",
)

# Spanish
tr(
    "es",
    "7DECODE",
    """
  //7DECODE <file>
      Une partes 7Plus ya guardadas en USER. <file> puede ser un nombre de
      parte (README.P01) o el original (README.TXT). La primera parte da el
      numero de partes. Se informan las que faltan; cuando estan todas se
      escribe el fichero original y se borran las partes.
""",
)
tr(
    "es",
    "MAIL",
    """
  //MAIL
      Muestra el correo pendiente para tu indicativo. No es transferencia
      de fichero. //Read envia un fichero. Un SSID numerico es el mismo
      buzon: NL1STK-2 lee NL1STK. Sin correo la respuesta es:
      You have no mail
""",
)
tr(
    "es",
    "SEND",
    """
  //SEND <call>
      La siguiente linea es un mensaje de correo para esa estacion. El SSID
      numerico se ignora (NL1STK-2 = NL1STK). Se guarda como correo de esa
      indicativo. Si ya hay correo, dos lineas en blanco y el mensaje nuevo:
      <from> to <call> at <date> <time>, luego el texto.
      Respuesta: GPNG: Mail send to <call>
""",
)
tr(
    "es",
    "CSTATUS",
    """
  //CSTATUS
      Lista solo canales con conexion. Primera linea: Channel state, luego
      fecha, hora y zona (%D %T %Z). Cada enlace una linea: tu indicativo,
      flecha y la otra estacion. <-- llamaron. --> saliste tu.
             Chan   1: NL1STK     <--  NL0MSK
""",
)
tr(
    "es",
    "N",
    """
  //<n> [text]
      n es un numero. //n solo entra al chat del canal n. Todo lo que
      escribas va a ese canal; el texto de alli (y otros) vuelve por aire.
      La sala ve "<GPNG>: CALL (Name) Joined your channel!". //0 sale y
      anuncia Left. Con texto, //n envia solo una linea sin quedarse en chat.
      El destino debe ser un canal TERM conectado.
""",
)
tr(
    "es",
    "#",
    """
  //# [text]
      # es un numero. //# solo entra al chat del canal #. Todo lo que
      escribas va a ese canal; el texto de alli (y otros) vuelve por aire.
      La sala ve "<GPNG>: CALL (Name) Joined your channel!". //0 sale y
      anuncia Left. Con texto, //# envia solo una linea sin quedarse en chat.
      El destino debe ser un canal TERM conectado.
""",
)
tr(
    "es",
    "PARAMETERS",
    """
  //PARameters
  //P
      Consulta parametros de temporizacion del TNC (P-Persistence, Slottime,
      Frack, T2, T3, TXDelay, MaxUnack). En WA8DED se leen en vivo; si no,
      los ultimos conocidos o de WA8DED.INI / init[].
""",
)
tr(
    "es",
    "RUN",
    """
  //RUN <program>
      Ejecuta un programa de EXTERNAL y envia su salida en este enlace.
      //RUN ? lista los programas. //<program> vale si el nombre no es ya
      un comando.

      Scripts en EXTERNAL: Python, PHP, Perl, Ruby, Node, Lua o Java.
      Java sin ventana. La primera linea puede ser p.ej.
      #! /usr/bin/env python3 (Linux, macOS y Windows). El lenguaje debe
      estar instalado en el ordenador del sysop. Palabras tras el nombre
      son argumentos.

      //RUN FREE
          Memoria total, usada y libre.

      Si el script lee una linea, el siguiente texto de esta conexion es
      esa linea. Un segundo //RUN responde "program is running". Al salir
      o tras 300 s sin entrada/salida se cierra; si sigue conectado:
      "closed run program after timeout". Un programa por canal.
""",
)
tr(
    "es",
    "CUT",
    """
  //CUT
      Aborta la transferencia en este canal y vuelve al chat. BIN-RX o
      captura 7+ envia #ABORT# (#NO# si BIN no ha empezado). BIN-TX para.
      Un emisor que ya envio bytes tambien manda #ABORT# para que el
      receptor deje el fichero.
""",
)
tr(
    "es",
    "READ",
    """
  //Read <filename>
      Envia un fichero de texto. Lista con "//CATalog".
      El correo es //MAIL, no esta transferencia.
""",
)
tr(
    "es",
    "LANGUAGE",
    """
  //Language <language>
      Cambia el idioma de la UI y de la ayuda remota (codigos ISO: en, es, de…).
      Sin parametros lista los idiomas disponibles.
""",
)

# French, Dutch, Portuguese, Norwegian, Polish, Czech, Hungarian, Slovenian,
# Bulgarian, Russian — same keys; keep translations practical.


def copy_en_as(lang: str, *keys: str) -> None:
    for k in keys:
        tr(lang, k, GPNG["en"][k])


for lang in ("fr", "nl", "pt", "no", "pl", "cs", "hu", "sl", "bg", "ru"):
    # Start from EN for structural accuracy; overlay language-specific where set.
    for k in REFRESH:
        if k not in GPNG.get(lang, {}):
            copy_en_as(lang, k)

# French overlays
tr(
    "fr",
    "MAIL",
    """
  //MAIL
      Affiche le courrier en attente pour votre indicatif. Ce n'est pas un
      transfert de fichier. //Read envoie un fichier. Un SSID numerique est
      la meme boite (NL1STK-2 lit NL1STK). Sans courrier:
      You have no mail
""",
)
tr(
    "fr",
    "SEND",
    """
  //SEND <call>
      La ligne suivante est un message pour cette station. Le SSID numerique
      est ignore. Reponse: GPNG: Mail send to <call>
""",
)
tr(
    "fr",
    "LANGUAGE",
    """
  //Language <language>
      Change la langue de l'interface et de l'aide (codes ISO: en, fr, de…).
      Sans parametre: liste des langues disponibles.
""",
)
tr(
    "fr",
    "READ",
    """
  //Read <filename>
      Envoie un fichier texte. Liste via "//CATalog".
      Le courrier est //MAIL, pas ce transfert.
""",
)
tr(
    "fr",
    "CUT",
    """
  //CUT
      Aborte le transfert sur ce canal et revient au chat. BIN-RX ou capture
      7+ envoie #ABORT# (#NO# si BIN n'a pas demarre). BIN-TX s'arrete.
""",
)
tr(
    "fr",
    "7DECODE",
    """
  //7DECODE <file>
      Assemble les parties 7Plus deja dans USER. <file> peut etre une partie
      (README.P01) ou le nom original. Parties manquantes signalees; quand
      tout est la, le fichier original est ecrit et les parties effacees.
""",
)
tr(
    "fr",
    "CSTATUS",
    """
  //CSTATUS
      Liste uniquement les canaux connectes. Premiere ligne: Channel state,
      puis date, heure et fuseau (%D %T %Z). <-- entree. --> sortie.
""",
)
tr(
    "fr",
    "N",
    GPNG["en"]["N"],
)
tr(
    "fr",
    "#",
    GPNG["en"]["#"],
)
tr(
    "fr",
    "PARAMETERS",
    """
  //PARameters
  //P
      Interroge les parametres tempo du TNC (P-Persistence, Slottime, Frack,
      T2, T3, TXDelay, MaxUnack). Sur WA8DED en direct; sinon derniere
      valeur connue ou WA8DED.INI / init[].
""",
)
tr(
    "fr",
    "RUN",
    """
  //RUN <program>
      Lance un programme depuis EXTERNAL et renvoie sa sortie. //RUN ?
      liste. Scripts: Python, PHP, Perl, Ruby, Node, Lua ou Java.
      //RUN FREE = memoire. Timeout 300 s. Un programme par canal.
""",
)

tr(
    "nl",
    "MAIL",
    """
  //MAIL
      Toont de mail voor jouw call. Dit is geen bestandsoverdracht.
      //Read stuurt een bestand. Numerieke SSID hoort bij dezelfde mailbox
      (NL1STK-2 leest NL1STK). Geen mail: You have no mail
""",
)
tr(
    "nl",
    "SEND",
    """
  //SEND <call>
      De volgende regel is mail voor die station. Numerieke SSID wordt
      genegeerd. Antwoord: GPNG: Mail send to <call>
""",
)
tr(
    "nl",
    "LANGUAGE",
    """
  //Language <language>
      Wisselt UI- en help-taal (ISO-codes zoals en, nl, de).
      Zonder parameter: lijst van talen.
""",
)
tr(
    "nl",
    "READ",
    """
  //Read <filename>
      Stuurt een tekstbestand. Lijst met "//CATalog".
      Mail is //MAIL, niet deze overdracht.
""",
)
tr(
    "nl",
    "CUT",
    """
  //CUT
      Stopt de bestandsoverdracht op dit kanaal en keert terug naar chat.
      BIN-RX of 7+-capture stuurt #ABORT# (#NO# als BIN nog niet startte).
""",
)
tr(
    "nl",
    "7DECODE",
    """
  //7DECODE <file>
      Voegt 7Plus-delen in USER samen. <file> mag een deelnaam of de
      originele naam zijn. Ontbrekende delen worden gemeld; bij alle delen
      wordt het origineel geschreven en de delen gewist.
""",
)
tr(
    "nl",
    "CSTATUS",
    """
  //CSTATUS
      Toont alleen kanalen met verbinding. Eerste regel Channel state, dan
      datum/tijd/zone. <-- inkomend. --> uitgaand.
""",
)
tr("nl", "N", GPNG["en"]["N"])
tr("nl", "#", GPNG["en"]["#"])
tr(
    "nl",
    "PARAMETERS",
    """
  //PARameters
  //P
      Vraagt TNC-timingparameters op. Op WA8DED live; anders laatste bekende
      of WA8DED.INI / init[].
""",
)
tr(
    "nl",
    "RUN",
    """
  //RUN <program>
      Start een programma uit EXTERNAL en stuurt de uitvoer terug. //RUN ?
      toont de lijst. Scripts: Python, PHP, Perl, Ruby, Node, Lua of Java.
      //RUN FREE = geheugen. Timeout 300 s. Een programma per kanaal.
""",
)

# For remaining langs, ensure full REFRESH set exists (EN copy already done above).
for lang in ("pt", "no", "pl", "cs", "hu", "sl", "bg", "ru"):
    for k in REFRESH:
        assert k in GPNG[lang], (lang, k)

# Light overlays for pt/no
tr(
    "pt",
    "LANGUAGE",
    """
  //Language <language>
      Muda o idioma da UI e da ajuda (codigos ISO: en, pt, es…).
      Sem parametro lista os idiomas.
""",
)
tr(
    "pt",
    "MAIL",
    """
  //MAIL
      Mostra o correio para o seu indicativo. Nao e transferencia de arquivo.
      //Read envia arquivo. SSID numerico e a mesma caixa. Sem correio:
      You have no mail
""",
)
tr(
    "pt",
    "SEND",
    """
  //SEND <call>
      A proxima linha e mensagem para essa estacao. Resposta:
      GPNG: Mail send to <call>
""",
)
tr(
    "pt",
    "READ",
    """
  //Read <filename>
      Envia um arquivo texto. Lista com "//CATalog".
      Correio e //MAIL, nao esta transferencia.
""",
)
tr(
    "no",
    "LANGUAGE",
    """
  //Language <language>
      Byttar UI- og hjelpesprak (ISO-koder som en, no, de).
      Uten parameter: liste over sprak.
""",
)
tr(
    "no",
    "MAIL",
    """
  //MAIL
      Viser post for ditt kallesignal. Dette er ikke filoverforing.
      //Read sender en fil. Numerisk SSID er samme postkasse. Uten post:
      You have no mail
""",
)
tr(
    "pl",
    "LANGUAGE",
    """
  //Language <language>
      Zmienia jezyk UI i pomocy (kody ISO: en, pl, de…).
      Bez parametru: lista jezykow.
""",
)
tr(
    "cs",
    "LANGUAGE",
    """
  //Language <language>
      Prepne jazyk UI a napovedy (ISO kody: en, cs, de…).
      Bez parametru vypise dostupne jazyky.
""",
)
tr(
    "hu",
    "LANGUAGE",
    """
  //Language <language>
      Valta az UI es a tavoli sugo nyelvet (ISO kodok: en, hu, de…).
      Parameter nelkul listazza a nyelveket.
""",
)
tr(
    "sl",
    "LANGUAGE",
    """
  //Language <language>
      Preklopi jezik vmesnika in pomoci (ISO kode: en, sl, de…).
      Brez parametra izpise seznam jezikov.
""",
)
tr(
    "bg",
    "LANGUAGE",
    GPNG["en"]["LANGUAGE"],
)
tr(
    "ru",
    "LANGUAGE",
    """
  //Language <language>
      Меняет язык интерфейса и справки (ISO: en, ru, de…).
      Без параметра — список языков.
""",
)


def ensure_overview(overview: str) -> str:
    """Keep existing overview prose, but ensure GPNG command lines appear."""
    lines = overview.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    existing_cmds = {
        re.sub(r"\s+", " ", ln.strip().lower())
        for ln in lines
        if ln.strip().startswith("//")
    }

    def present(cmd_line: str) -> bool:
        # Match by command token after //
        token = cmd_line.strip().split()[0].lower()  # //7decode or //# 
        for ex in existing_cmds:
            if ex.startswith(token):
                return True
        # Also bare forms like //mail in descriptions
        bare = token.lstrip("/").lower()
        joined = "\n".join(lines).lower()
        return f"//{bare}" in joined or f"//{bare.replace('a', 'a')}" in joined

    missing = [ln for ln in OVERVIEW_LINES if not present(ln)]
    if not missing:
        return overview if overview.endswith("\n") else overview + "\n"

    # Insert before trailing blank / footer lines after last // line
    insert_at = 0
    for i, ln in enumerate(lines):
        if ln.strip().startswith("//"):
            insert_at = i + 1
    block = missing
    new_lines = lines[:insert_at] + block + lines[insert_at:]
    text = "\n".join(new_lines).rstrip() + "\n"
    return text


def write_locale(
    path: Path,
    name: str,
    setup: dict[str, str],
    overview: str,
    commands: dict[str, str],
    prefixes: dict[str, int] | None = None,
) -> None:
    lines: list[str] = ["[meta]", f'name = "{name}"', ""]
    if setup:
        lines.append("[setup]")
        for k, v in setup.items():
            lines.append(f'{k} = "{v}"')
        lines.append("")
    lines.append("[help]")
    lines.append(f"overview = {toml_multiline(overview)}")
    lines.append("")
    lines.append("[help.commands]")
    # Stable order: REQUIRED first, then any extras (e.g. CD)
    seen = set()
    for tag in REQUIRED:
        if tag in commands:
            lines.append(f"{key_for(tag)} = {toml_multiline(commands[tag])}")
            seen.add(tag)
    for tag in sorted(commands):
        if tag not in seen:
            lines.append(f"{key_for(tag)} = {toml_multiline(commands[tag])}")
    if prefixes:
        lines.append("")
        lines.append("[help.prefix]")
        for tag, mn in sorted(prefixes.items()):
            lines.append(f"{key_for(tag)} = {mn}")
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def load_en_prefixes() -> dict[str, int]:
    text = (OUT / "en.toml").read_text(encoding="utf-8")
    prefs: dict[str, int] = {}
    if "[help.prefix]" not in text:
        return prefs
    block = text.split("[help.prefix]", 1)[1]
    for m in re.finditer(r'(?m)^(?:"([^"]+)"|([A-Za-z0-9+#]+))\s*=\s*(\d+)', block):
        tag = (m.group(1) or m.group(2)).upper()
        prefs[tag] = int(m.group(3))
    return prefs


def main() -> None:
    # Fix Spanish from DOS source first
    if DOS_E.is_file():
        ov, secs, _order = parse_gph(decode(DOS_E.read_bytes()))
        meta, setup, old_cmds, old_ov = parse_locale_toml(
            (OUT / "es.toml").read_text(encoding="utf-8", errors="replace")
        )
        # Prefer re-parsed GPH sections, then old, then GPNG overlays
        commands = dict(secs)
        for k, v in old_cmds.items():
            commands.setdefault(k, v)
        for k, v in GPNG["es"].items():
            commands[k] = v.lstrip("\n") if v.startswith("\n") else v
        overview = ensure_overview(ov)
        write_locale(
            OUT / "es.toml",
            meta.get("name", "Spanish"),
            setup,
            overview,
            commands,
        )
        print(f"rewrote es.toml ({len(commands)} commands)")

    prefixes = load_en_prefixes()
    en_meta, en_setup, en_cmds, en_ov = parse_locale_toml(
        (OUT / "en.toml").read_text(encoding="utf-8")
    )
    # Keep en as-is but ensure REFRESH texts match GPNG["en"]
    for k, v in GPNG["en"].items():
        body = v.lstrip("\n") if v.startswith("\n") else v
        en_cmds[k] = body if body.endswith("\n") else body + "\n"
    write_locale(
        OUT / "en.toml",
        en_meta.get("name", "English"),
        en_setup,
        ensure_overview(en_ov),
        en_cmds,
        prefixes,
    )
    print("refreshed en.toml")

    for path in sorted(OUT.glob("*.toml")):
        iso = path.stem
        if iso in ("en", "es"):
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        meta, setup, commands, overview = parse_locale_toml(text)
        gpng = GPNG.get(iso, GPNG["en"])
        added = []
        for key in REQUIRED:
            if key in REFRESH or key not in commands:
                if key in gpng:
                    body = gpng[key]
                    body = body.lstrip("\n") if body.startswith("\n") else body
                    commands[key] = body if body.endswith("\n") else body + "\n"
                    added.append(key)
                elif key in en_cmds and key not in commands:
                    commands[key] = en_cmds[key]
                    added.append(key)
            elif key not in commands and key in en_cmds:
                commands[key] = en_cmds[key]
                added.append(key)
        overview = ensure_overview(overview)
        write_locale(
            path,
            meta.get("name", iso),
            setup,
            overview,
            commands,
        )
        print(f"{iso}: updated {sorted(set(added))}")


if __name__ == "__main__":
    main()
