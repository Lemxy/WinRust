#!/usr/bin/env python3
"""
给 WinRust 的 t!/tf! 调用批量补中文（zh: "..."）。

策略：
- 只处理带 `ja:` 和 `pt:` 的调用（4 参数完整形式）
- 在 pt: "..." 之后插入 zh: "<翻译>"
- 翻译来自 zh-map.json（英文原文 -> 中文），缺失的回落到英文原文
- 用括号配对解析，避免正则误伤字符串里的括号

用法：
  python add-zh.py --dry-run     # 只报告会改多少处
  python add-zh.py               # 实际修改
"""

import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MAP_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "zh-map.json")

# 匹配 t!( 或 tf!( 的位置
CALL_RE = re.compile(r"\b(tf?!)\s*\(")


def find_call_end(src: str, open_paren: int) -> int:
    """从 ( 的位置开始，找到配对的 )，跳过字符串和字符字面量。"""
    depth = 0
    i = open_paren
    n = len(src)
    while i < n:
        c = src[i]
        if c == '"':
            # 跳过字符串（处理转义）
            i += 1
            while i < n:
                if src[i] == "\\":
                    i += 2
                    continue
                if src[i] == '"':
                    break
                i += 1
        elif c == "'":
            # 字符字面量或生命周期，粗略跳过
            j = i + 1
            if j < n and src[j] == "\\":
                j += 2
            else:
                j += 1
            if j < n and src[j] == "'":
                i = j
        elif c == "/" and i + 1 < n and src[i + 1] == "/":
            # 行注释
            while i < n and src[i] != "\n":
                i += 1
            continue
        elif c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return -1


def parse_args(arg_text: str):
    """把 t! 的参数拆成 (positional, named)。named 形如 {'ja': '...', 'pt': '...'}"""
    parts = []
    depth = 0
    cur = []
    i = 0
    n = len(arg_text)
    in_str = False
    while i < n:
        c = arg_text[i]
        if in_str:
            cur.append(c)
            if c == "\\":
                if i + 1 < n:
                    cur.append(arg_text[i + 1])
                    i += 2
                    continue
            elif c == '"':
                in_str = False
            i += 1
            continue
        if c == '"':
            in_str = True
            cur.append(c)
            i += 1
            continue
        if c in "([{":
            depth += 1
            cur.append(c)
            i += 1
            continue
        if c in ")]}":
            depth -= 1
            cur.append(c)
            i += 1
            continue
        if c == "," and depth == 0:
            parts.append("".join(cur).strip())
            cur = []
            i += 1
            continue
        cur.append(c)
        i += 1
    tail = "".join(cur).strip()
    if tail:
        parts.append(tail)

    positional = []
    named = {}
    for p in parts:
        m = re.match(r"^(ja|pt|zh)\s*:\s*(.+)$", p, re.S)
        if m:
            named[m.group(1)] = m.group(2).strip()
        else:
            positional.append(p)
    return positional, named


def unquote(s: str) -> str:
    """把 "..." 字面量解成内容（只处理常见转义）。"""
    s = s.strip()
    if len(s) >= 2 and s[0] == '"' and s[-1] == '"':
        body = s[1:-1]
        return body.encode("utf-8").decode("unicode_escape").encode("latin-1", "ignore").decode("utf-8", "ignore") or body
    return s


def load_map():
    if not os.path.exists(MAP_FILE):
        return {}
    with open(MAP_FILE, encoding="utf-8") as f:
        return json.load(f)


def process_file(path: str, zh_map: dict, dry: bool):
    with open(path, encoding="utf-8") as f:
        src = f.read()

    out = []
    last = 0
    changed = 0
    missing = []

    for m in CALL_RE.finditer(src):
        name = m.group(1)
        open_paren = m.end() - 1
        if open_paren < last:
            continue
        close = find_call_end(src, open_paren)
        if close < 0:
            continue
        inner = src[open_paren + 1 : close]
        positional, named = parse_args(inner)

        # 只处理已经有 ja: 和 pt: 的完整调用
        if "ja" not in named or "pt" not in named or "zh" in named:
            continue
        # 需要至少两个位置参数(ru, en)
        if len(positional) < 2:
            continue

        en_text = unquote(positional[1])
        zh = zh_map.get(en_text)
        if not zh:
            zh = en_text  # 回落英文
            missing.append(en_text)

        # 重建参数：zh: 必须插在【最外层】pt: 之后、额外参数之前
        # 宏签名: ($ru, $en, ja: $ja, pt: $pt, zh: $zh $(, $a:expr)*)
        # 注意：位置参数里可能嵌着另一个 t!/tf!（也含 pt:），必须跳过嵌套的
        pt_pos = -1
        depth_p = 0
        in_sp = False
        k = 0
        while k < len(inner):
            ch = inner[k]
            if in_sp:
                if ch == "\\":
                    k += 2
                    continue
                if ch == '"':
                    in_sp = False
            elif ch == '"':
                in_sp = True
            elif ch in "([{":
                depth_p += 1
            elif ch in ")]}":
                depth_p -= 1
            elif depth_p == 0 and inner.startswith("pt:", k):
                pt_pos = k
            k += 1
        if pt_pos < 0:
            continue
        # 从 pt: 开始找那个参数的结束位置（到下一个顶层逗号）
        i = pt_pos
        depth2 = 0
        in_s = False
        while i < len(inner):
            ch = inner[i]
            if in_s:
                if ch == "\\":
                    i += 2
                    continue
                if ch == '"':
                    in_s = False
            elif ch == '"':
                in_s = True
            elif ch in "([{":
                depth2 += 1
            elif ch in ")]}":
                depth2 -= 1
            elif ch == "," and depth2 == 0:
                break
            i += 1
        # i 现在指向 pt 参数后的逗号（或结尾）
        before = inner[:i].rstrip()
        after = inner[i:]  # 以 "," 开头
        new_inner = before + f', zh: "{zh}"' + after

        out.append(src[last : open_paren + 1])
        out.append(new_inner)
        out.append(")")
        last = close + 1
        changed += 1

    out.append(src[last:])
    new_src = "".join(out)

    if changed and not dry:
        with open(path, "w", encoding="utf-8", newline="") as f:
            f.write(new_src)

    return changed, missing


def main():
    dry = "--dry-run" in sys.argv
    zh_map = load_map()

    rs_files = []
    for dirpath, dirnames, filenames in os.walk(ROOT):
        if "target" in dirpath or ".git" in dirpath:
            continue
        for fn in filenames:
            if fn.endswith(".rs"):
                rs_files.append(os.path.join(dirpath, fn))

    total = 0
    all_missing = []
    for path in rs_files:
        try:
            n, missing = process_file(path, zh_map, dry)
        except Exception as e:
            print(f"  ERROR {path}: {e}")
            continue
        if n:
            rel = os.path.relpath(path, ROOT)
            print(f"  {rel}: {n}")
            total += n
            all_missing.extend(missing)

    print(f"\n{'[dry-run] ' if dry else ''}共处理 {total} 处")
    if all_missing:
        uniq = sorted(set(all_missing))
        print(f"未命中翻译 {len(uniq)} 条（回落到英文）")
        print("\n前 30 条：")
        for t in uniq[:30]:
            print(f"  {t}")


if __name__ == "__main__":
    main()
