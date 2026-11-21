#!/usr/bin/env python3
"""Extract and audit WinRust translations without modifying Rust sources.

Usage: python scripts/i18n-catalog.py export
       python scripts/i18n-catalog.py audit --source PATH

Keys are the original English text. Conflicting translations are reported for
review; this tool deliberately does not choose one silently. The exported
catalog is a migration aid, not a runtime language pack yet.
"""
import argparse
import json
import pathlib
import re
import sys

if hasattr(sys.stdout, 'reconfigure'):
    sys.stdout.reconfigure(encoding='utf-8', errors='backslashreplace')

ROOT = pathlib.Path(__file__).resolve().parents[1]
LITERAL = r'"(?:\\.|[^"\\])*"'
CALL = re.compile(r'\b(?P<kind>ptf!|pt!|tf!|t!|Tr)\s*\(\s*(?P<ru>' + LITERAL + r')\s*,\s*(?P<en>' + LITERAL + r')(?:\s*,\s*(?P<pt>' + LITERAL + r'))?', re.S)
RUST_UNICODE = re.compile(r'\\u\{([0-9a-fA-F_]+)\}')
FIELDS = re.compile(r'\{\{|\}\}|\{[^{}]*\}')


def decode(literal):
    return json.loads(RUST_UNICODE.sub(lambda m: chr(int(m.group(1).replace('_', ''), 16)), literal))


def fields(value):
    return sorted(x for x in FIELDS.findall(value) if x not in ('{{', '}}'))


def files(root):
    for path in root.rglob('*.rs'):
        if not any(part in {'.git', 'target', 'dist', 'payload'} for part in path.relative_to(root).parts):
            yield path


def extract(root):
    values, needed, invalid = {}, set(), []
    for path in files(root):
        source = path.read_text(encoding='utf-8')
        for m in CALL.finditer(source):
            try:
                en = decode(m.group('en'))
                pt = decode(m.group('pt')) if m.group('pt') else None
            except (ValueError, json.JSONDecodeError):
                invalid.append(f'{path.relative_to(root)}:{source.count(chr(10), 0, m.start()) + 1}')
                continue
            needed.add(en)
            if m.group('kind') in ('pt!', 'ptf!', 'Tr') and pt is not None:
                values.setdefault(en, set()).add(pt)
    return values, needed, invalid


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('action', choices=('export', 'audit'))
    parser.add_argument('--source', type=pathlib.Path, default=ROOT)
    parser.add_argument('--catalog', type=pathlib.Path, default=ROOT / 'locales' / 'pt-BR.json')
    parser.add_argument('--strict', action='store_true', help='return a failure for missing or ambiguous translations')
    args = parser.parse_args()
    found, required, invalid = extract(args.source.resolve())
    if args.action == 'export':
        existing = json.loads(args.catalog.read_text(encoding='utf-8')) if args.catalog.exists() else {}
        messages = existing.get('messages', {})
        conflicts = {}
        for en, candidates in found.items():
            if len(candidates) == 1:
                messages.setdefault(en, next(iter(candidates)))
            elif en not in messages:
                conflicts[en] = sorted(candidates)
        result = {'language': 'pt-BR', 'messages': dict(sorted(messages.items())), 'needs_review': dict(sorted(conflicts.items()))}
        args.catalog.parent.mkdir(parents=True, exist_ok=True)
        args.catalog.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    else:
        result = json.loads(args.catalog.read_text(encoding='utf-8'))
        messages = result['messages']
    missing = sorted(required - messages.keys())
    bad = sorted(en for en, pt in messages.items() if fields(en) != fields(pt))
    print(f'{len(messages)} catalog entries; {len(missing)} untranslated source messages; {len(result.get("needs_review", {}))} ambiguous keys; {len(bad)} placeholder mismatches; {len(invalid)} literals require manual review.')
    for en in missing[:15]: print('MISSING:', repr(en))
    for en in bad[:15]: print('PLACEHOLDERS:', repr(en))
    if invalid: print('UNREADABLE:', ', '.join(invalid[:15]))
    return 1 if bad or invalid or (args.strict and (missing or result.get('needs_review'))) else 0


if __name__ == '__main__':
    sys.exit(main())
