#!/usr/bin/env python3
"""Check that every relative link between the repository's Markdown files works.

The documentation cross-references heavily — the gap register points at specs,
specs point back at gaps, RFCs point at both — and a register full of dead links
is worse than no register, because it reads as maintained.

Only local links are checked. External URLs are not fetched: a network check in
CI fails for reasons that have nothing to do with the commit.

    python3 scripts/check-links.py
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

SKIP_DIRS = {".git", "target", "node_modules"}

INLINE_LINK = re.compile(r"\]\(([^)\s]+)\)")
REFERENCE_LINK = re.compile(r"^\[[^\]]+\]:\s*(\S+)\s*$", re.MULTILINE)
HEADING = re.compile(r"^#{1,6}\s+(.*)$", re.MULTILINE)
FENCE = re.compile(r"^```.*?^```", re.MULTILINE | re.DOTALL)


def slugs(heading: str) -> set[str]:
    """The anchors a heading might plausibly have.

    Renderers disagree about punctuation — GitHub's slugger keeps dash
    characters and turns each space into a hyphen, so `A — B` becomes
    `a-—-b`, while a stricter reading gives `a-b`. Rather than pick a side and
    be wrong somewhere, accept either, and prefer headings that do not make the
    question arise.
    """
    text = heading.strip().lower()
    text = re.sub(r"`([^`]*)`", r"\1", text)
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)

    strict = re.sub(r"\s+", "-", re.sub(r"[^\w\s-]", "", text))
    # GitHub: drop other punctuation, keep dashes, one hyphen per space.
    github = re.sub(r"\s", "-", re.sub(r"[^\w\s‐-―-]", "", text))
    return {strict, github}


def markdown_files(root: Path) -> list[Path]:
    return sorted(
        path
        for path in root.rglob("*.md")
        if not SKIP_DIRS.intersection(path.parts)
    )


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    files = markdown_files(root)
    anchors = {}
    for path in files:
        body = path.read_text(encoding="utf-8")
        anchors[path] = {
            anchor for match in HEADING.findall(body) for anchor in slugs(match)
        }

    problems: list[str] = []
    checked = 0

    for path in files:
        body = path.read_text(encoding="utf-8")
        # Links inside fenced code are examples, not references.
        body = FENCE.sub("", body)
        links = INLINE_LINK.findall(body) + REFERENCE_LINK.findall(body)

        for link in links:
            if link.startswith(("http://", "https://", "mailto:", "#!")):
                continue
            checked += 1
            where, _, anchor = link.partition("#")

            if where:
                target = (path.parent / where).resolve()
                if not target.exists():
                    problems.append(f"{path.relative_to(root)} -> {link} (no such file)")
                    continue
            else:
                target = path

            if anchor and target.suffix == ".md" and target in anchors:
                if anchor not in anchors[target]:
                    problems.append(f"{path.relative_to(root)} -> {link} (no such heading)")

    for problem in problems:
        print(problem, file=sys.stderr)

    if problems:
        print(f"\n{len(problems)} broken link(s) in {len(files)} file(s)", file=sys.stderr)
        return 1

    print(f"{checked} local link(s) in {len(files)} Markdown file(s): all resolve")
    return 0


if __name__ == "__main__":
    sys.exit(main())
