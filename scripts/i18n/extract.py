#!/usr/bin/env python3
"""Deskwarden i18n tooling: what the code asks for, what the catalogs hold, and
which user-visible strings are still hard-coded.

    python scripts/i18n/extract.py                 # coverage report; exit 1 if code and en.txt disagree
    python scripts/i18n/extract.py --candidates    # hard-coded user-visible strings still to convert
    python scripts/i18n/extract.py --candidates --all   # ... including vault_window/ and prefs_ui*
    python scripts/i18n/extract.py --todo de       # en.txt entries `de` has not translated, as key = value lines
    python scripts/i18n/extract.py --ci            # as the default, quiet unless something is wrong

The catalog format is documented at the top of deskwarden/src/i18n.rs; the
reader here accepts exactly what the Rust one does (keys, `key[category]`,
trailing-backslash continuations, `#` comments) and reports what it cannot.
The Rust test suite (`cargo test --lib i18n`) is the gate -- every key used in
code exists in en.txt, every locale only has keys English has, with the same
placeholders. This script is the human-facing view of the same facts plus the
one thing a test cannot know: which strings nobody has converted yet.
"""

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2] / "deskwarden"
SRC = ROOT / "src"
LOCALES = ROOT / "locales"

# What a phase-1 conversion leaves for later (see docs/superpowers/plans/2026-11-19-i18n.md).
PHASE_2_PREFIXES = ("vault_window/", "prefs_ui")

CALLS = [("tk", re.compile(r'(?<![A-Za-z0-9_])tk!\("([^"]*)"')),
         ("tf", re.compile(r'i18n::tf\("([^"]*)"')),
         ("tn", re.compile(r'i18n::tn\("([^"]*)"')),
         ("t", re.compile(r'i18n::t\("([^"]*)"'))]

PLURAL_CATEGORIES = {
    "en": ["one", "other"], "bg": ["one", "other"], "ca": ["one", "many", "other"],
    "cs": ["one", "few", "other"], "da": ["one", "other"], "de": ["one", "other"],
    "el": ["one", "other"], "es": ["one", "many", "other"], "et": ["one", "other"],
    "fi": ["one", "other"], "fr": ["one", "many", "other"], "hr": ["one", "few", "other"],
    "hu": ["one", "other"], "id": ["other"], "it": ["one", "many", "other"], "ja": ["other"],
    "ko": ["other"], "lt": ["one", "few", "other"], "lv": ["zero", "one", "other"],
    "ms": ["other"], "nb": ["one", "other"], "nl": ["one", "other"],
    "pl": ["one", "few", "many", "other"], "pt-BR": ["one", "many", "other"],
    "pt-PT": ["one", "many", "other"], "ro": ["one", "few", "other"],
    "ru": ["one", "few", "many", "other"], "sk": ["one", "few", "other"],
    "sl": ["one", "two", "few", "other"], "sr": ["one", "few", "other"], "sv": ["one", "other"],
    "tr": ["one", "other"], "uk": ["one", "few", "many", "other"], "vi": ["other"],
    "zh-Hans": ["other"], "zh-Hant": ["other"],
}


def parse_catalog(path):
    """Returns ({(key, plural_or_None): value}, [problems])."""
    entries, problems = {}, []
    lines = path.read_text(encoding="utf-8").splitlines()
    i = 0
    while i < len(lines):
        number, line = i + 1, lines[i].strip()
        i += 1
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            problems.append(f"{path.name}:{number}: no `=`")
            continue
        raw_key, rest = line.split("=", 1)
        raw_key = raw_key.strip()
        m = re.fullmatch(r"([a-z][a-z0-9_.]*)(?:\[(zero|one|two|few|many|other)\])?", raw_key)
        if not m:
            problems.append(f"{path.name}:{number}: `{raw_key}` is not a key")
            continue
        value = rest.lstrip()
        while re.search(r"(?<!\\)(?:\\\\)*\\$", value) and i < len(lines):
            value = value[:-1] + lines[i].lstrip()
            i += 1
        value = value.rstrip(" \t")
        key = (m.group(1), m.group(2))
        if key in entries:
            problems.append(f"{path.name}:{number}: `{raw_key}` defined twice")
        entries[key] = value
    return entries, problems


def placeholders(value):
    value = value.replace("{{", "").replace("}}", "")
    return set(re.findall(r"\{([a-z][a-z0-9_]*)\}", value))


def source_files(include_phase_2=True):
    for path in sorted(SRC.rglob("*.rs")):
        rel = path.relative_to(SRC).as_posix()
        if rel == "i18n.rs" or rel.startswith("i18n/"):
            continue
        if not include_phase_2 and rel.startswith(PHASE_2_PREFIXES):
            continue
        yield rel, path.read_text(encoding="utf-8").replace("\r\n", "\n")


def used_keys():
    uses = []
    for rel, text in source_files():
        for number, line in enumerate(text.split("\n"), 1):
            if line.lstrip().startswith("//"):
                continue
            for kind, pattern in CALLS:
                for m in pattern.finditer(line):
                    uses.append((rel, number, kind, m.group(1)))
    return uses


def coverage(quiet=False):
    en, problems = parse_catalog(LOCALES / "en.txt")
    en_keys = {k for k, _ in en}
    plural_keys = {k for k, p in en if p}
    bad = list(problems)

    uses = used_keys()
    used = {key for _, _, _, key in uses}
    for rel, number, kind, key in uses:
        if key not in en_keys:
            bad.append(f"{rel}:{number}: `{key}` is used but not in en.txt")
        elif (kind == "tn") != (key in plural_keys):
            bad.append(f"{rel}:{number}: `{key}` is {'plural' if key in plural_keys else 'plain'} in en.txt but read with {kind}")
    quoted = "\n".join(text for _, text in source_files())
    unused = sorted(k for k in en_keys if f'"{k}"' not in quoted)
    for key in unused:
        bad.append(f"en.txt: `{key}` is not mentioned in any source file")

    rows = []
    for path in sorted(LOCALES.glob("*.txt")):
        tag = path.stem
        if tag == "en":
            continue
        entries, problems = parse_catalog(path)
        bad.extend(problems)
        have = {k for k, _ in entries}
        for (key, plural), value in entries.items():
            if key not in en_keys:
                bad.append(f"{path.name}: `{key}` does not exist in en.txt")
                continue
            source = en.get((key, plural)) if (key, plural) in en else en.get((key, "other"))
            if source is not None and placeholders(value) - ({"n"} if plural else set()) != placeholders(source) - ({"n"} if plural else set()):
                bad.append(f"{path.name}: `{key}` uses different placeholders from English")
            if plural and plural not in PLURAL_CATEGORIES.get(tag, []):
                bad.append(f"{path.name}: `{key}[{plural}]` is not a form {tag} uses")
        missing_forms = 0
        for key in plural_keys & have:
            for form in PLURAL_CATEGORIES.get(tag, []):
                if (key, form) not in entries:
                    missing_forms += 1
        rows.append((tag, len(have & en_keys), missing_forms))

    if not quiet:
        print(f"en.txt: {len(en_keys)} keys ({len(plural_keys)} plural), {len(used)} distinct keys used in code")
        print(f"{'locale':8} {'keys':>6} {'of':>5} {'%':>5}  incomplete plurals")
        for tag, count, forms in rows:
            pct = 100.0 * count / len(en_keys) if en_keys else 100.0
            print(f"{tag:8} {count:6} {len(en_keys):5} {pct:5.1f}  {forms or ''}")
    for line in bad:
        print("PROBLEM:", line, file=sys.stderr)
    return 1 if bad else 0


# ---- candidates: user-visible literals still hard-coded ----------------------

# Where a literal is certainly not user-visible.
NOT_VISIBLE = re.compile(
    r"(log::(?:trace|debug|info|warn|error)!|(?:debug_)?assert(?:_eq|_ne)?!|panic!|unreachable!|"
    r"\.expect\(|todo!|unimplemented!|eprintln!|#\[|include_str!|concat!|env!|\.context\()")
# Where one very likely is.
VISIBLE = re.compile(
    r"(\.label\(|RichText::new|\.button\(|button\(|button_width\(|on_hover_text|\.heading\(|"
    r"semibold\(|regular\(|bold\(|extrabold\(|tokens::job\(|toast::show|\.hint_text\(|placeholder|"
    r"message_box|draw_text|\.text\(|ui\.link\(|selectable_label|checkbox\(|Dialog|title\(|"
    r"\.paint_text|painter\(\)\.text|\.add\(egui::Label|egui::Label::new|WidgetText)")
TEXTLIKE = re.compile(r"[A-Za-z]{2,}")


def test_cut(text):
    m = re.search(r"^#\[cfg\(test\)\]\n(?:#\[.*\]\n)*(?:pub )?mod \w+", text, re.M)
    return m.start() if m else len(text)


def candidates(include_phase_2):
    total = 0
    for rel, text in source_files(include_phase_2):
        production = text[:test_cut(text)]
        for number, line in enumerate(production.split("\n"), 1):
            stripped = line.strip()
            if stripped.startswith("//") or NOT_VISIBLE.search(line):
                continue
            for lit in re.findall(r'"((?:[^"\\]|\\.)*)"', line):
                if not TEXTLIKE.search(lit) or lit.startswith(("http", "C:", "\\\\")):
                    continue
                prose = re.search(r"[A-Za-z]{3,}[^A-Za-z]+[A-Za-z]{2,}", lit) or re.fullmatch(r"[A-Z][a-z]{3,}.*", lit)
                if VISIBLE.search(line) or (prose and re.match(r"[A-Z{]", lit)):
                    print(f"{rel}:{number}: {lit[:110]}")
                    total += 1
    print(f"-- {total} candidate literal(s); a candidate is a prompt to look, not a verdict.", file=sys.stderr)
    return 0


def todo(tag):
    en, _ = parse_catalog(LOCALES / "en.txt")
    path = LOCALES / f"{tag}.txt"
    if not path.exists():
        print(f"no such locale: {tag}", file=sys.stderr)
        return 2
    have, _ = parse_catalog(path)
    wanted = PLURAL_CATEGORIES.get(tag, ["other"])
    for (key, plural), value in en.items():
        if plural is None:
            if (key, None) not in have:
                print(f"{key} = {value}")
        elif plural in ("one", "other"):
            # English supplies the pattern; the language needs ITS forms.
            for form in wanted:
                if (key, form) not in have:
                    src = en.get((key, form)) or en.get((key, "other"))
                    print(f"{key}[{form}] = {src}")
    return 0


def main():
    # Catalog text is Unicode; a Windows console is not, by default.
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--candidates", action="store_true")
    ap.add_argument("--all", action="store_true", help="with --candidates: include vault_window/ and prefs_ui*")
    ap.add_argument("--todo", metavar="LOCALE")
    ap.add_argument("--ci", action="store_true")
    args = ap.parse_args()
    if args.candidates:
        return candidates(args.all)
    if args.todo:
        return todo(args.todo)
    return coverage(quiet=args.ci)


if __name__ == "__main__":
    sys.exit(main())
