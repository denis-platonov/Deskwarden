#!/usr/bin/env python3
"""Proves en.txt still says what the code said before it was converted.

    python scripts/i18n/verify_english.py [BASE_REV]      # default: v1

For every key in deskwarden/locales/en.txt it looks for the English text among
the string literals of the Rust sources at BASE_REV (`git show`). A key whose
text is found there verbatim is **byte-identical to the string it replaced**. A
key with `{placeholders}` is a rewritten `format!`, so its template is compared
with the original after turning every `{name}`/`{}`/`{:?}`-style hole into `{}`.
Anything neither of those finds is listed, for a human to look at -- a new
string, or a deliberate wording change recorded in the plan's Rulings.

Exit status 1 if anything is unexplained.
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(Path(__file__).resolve().parent))
from extract import parse_catalog, LOCALES  # noqa: E402

BS = chr(92)


def decode(lit):
    out, i = [], 0
    while i < len(lit):
        c = lit[i]
        if c != BS:
            out.append(c)
            i += 1
            continue
        n = lit[i + 1]
        if n == "u":
            j = lit.index("}", i)
            out.append(chr(int(lit[i + 3:j], 16)))
            i = j + 1
        elif n in "nt\"'" + BS:
            out.append({"n": "\n", "t": "\t"}.get(n, n))
            i += 2
        elif n == "\n":
            i += 2
            while i < len(lit) and lit[i] in " \t\n":
                i += 1
        else:
            out.append(lit[i:i + 2])
            i += 2
    return "".join(out)


def literals(text):
    out, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if text.startswith("//", i):
            j = text.find("\n", i)
            i = n if j < 0 else j
        elif text.startswith("/*", i):
            j = text.find("*/", i)
            i = n if j < 0 else j + 2
        elif c == "r" and re.match(r'r#*"', text[i:i + 8]) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            m = re.match(r'r(#*)"', text[i:])
            close = '"' + m.group(1)
            j = text.find(close, i + len(m.group(0)))
            i = n if j < 0 else j + len(close)
        elif c == "'":
            m = re.match(r"'(?:" + BS + BS + r"(?:u\{[0-9a-fA-F]+\}|.)|[^" + BS + BS + r"'])'", text[i:], re.S)
            i += len(m.group(0)) if m else 1
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == BS else 1
            out.append(decode(text[i + 1:j]))
            i = j + 1
        else:
            i += 1
    return out


def shape(template):
    template = template.replace("{{", "\0").replace("}}", "\1")
    return re.sub(r"\{[^{}]*\}", "{}", template).replace("\0", "{{").replace("\1", "}}")


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    base = sys.argv[1] if len(sys.argv) > 1 else "v1"
    files = subprocess.run(["git", "ls-tree", "-r", "--name-only", base, "deskwarden/src", "deskwarden/examples"],
                           cwd=ROOT, capture_output=True, text=True, check=True).stdout.split()
    exact, shapes = set(), set()
    for f in files:
        if not f.endswith(".rs"):
            continue
        text = subprocess.run(["git", "show", f"{base}:{f}"], cwd=ROOT, capture_output=True, check=True).stdout.decode("utf-8").replace("\r\n", "\n")
        for lit in literals(text):
            exact.add(lit)
            shapes.add(shape(lit))
    en, problems = parse_catalog(LOCALES / "en.txt")
    verbatim = templated = 0
    unexplained = []
    for (key, plural), raw in sorted(en.items(), key=lambda kv: kv[0][0]):
        value = raw.replace(BS + "n", "\n").replace(BS + "t", "\t").replace(BS + "s", " ").replace(BS + BS, BS)
        if value in exact:
            verbatim += 1
        elif "{" in value and shape(value) in shapes:
            templated += 1
        else:
            unexplained.append((key + (f"[{plural}]" if plural else ""), value))
    print(f"{verbatim} keys verbatim from {base}, {templated} templates match a format string there, {len(unexplained)} to look at")
    for key, value in unexplained:
        print(f"  {key} = {value[:100]}")
    return 1 if unexplained or problems else 0


if __name__ == "__main__":
    sys.exit(main())
