#!/usr/bin/env python3
"""문서가 스스로 모순되지 않는지 기계로 본다.

이 세션에 손으로 찾은 오류 네 가지가 모두 **읽어 보면 알 수 있는 것**이었다.

    수용 체크리스트가 자기모순      텍스트는 ✓ 8개인데 체크박스는 전부 비어 있었다
    "운영 도구"가 뒤처짐            nimwake status를 만들고도 "Post-MVP"로 적혀 있었다
    "장시간 운용"이 뒤처짐          d7c5590 기준에 멈춰 있었다
    §75·§79·§81 링크 경로가 틀림     다른 파일의 절을 같은 문서 앵커로 적었다

읽어 보면 아는 것은 **읽지 않으면 모른다.** 공개 저장소라 틀린 링크는 바로
보인다. 기계가 보게 한다.

사람의 판단이 필요한 것(내용이 사실인지)은 보지 않는다. 구조만 본다.
"""

from __future__ import annotations

import pathlib
import re
import sys
from collections import Counter, defaultdict

DOCS = pathlib.Path(".docs")
PRD = DOCS / "prd"

# `## 85. Control Socket` — PRD의 절. 하위 절(`### 5.1`)은 번호가 아니다.
SECTION = re.compile(r"^(#+) (\d+)\. ", re.M)
# `[텍스트](경로)` — 외부 링크(http)와 순수 앵커(#)는 따로 다룬다.
LINK = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
# 본문에서 절을 가리키는 `§85`.
REFERENCE = re.compile(r"§(\d+)")


def slug(heading: str) -> str:
    """GitHub이 제목에서 만드는 앵커.

    소문자로 바꾸고, 공백을 하이픈으로, 그 밖의 기호를 버린다. 한글은 남는다.
    완전히 같지는 않지만 이 저장소의 제목에는 충분하다.
    """
    text = heading.strip().lower()
    text = re.sub(r"[^\w\s-]", "", text, flags=re.UNICODE)
    return re.sub(r"\s+", "-", text).strip("-")


def markdown_files() -> list[pathlib.Path]:
    return sorted(p for p in DOCS.rglob("*.md") if p.is_file())


def collect_sections() -> tuple[dict[int, list[str]], list[str]]:
    """PRD의 절 번호 → 그것이 있는 파일들. 그리고 수준이 틀린 것들."""
    sections: dict[int, list[str]] = defaultdict(list)
    wrong_level: list[str] = []

    for path in sorted(PRD.rglob("*.md")):
        text = path.read_text(encoding="utf-8")
        for match in SECTION.finditer(text):
            level, number = match.group(1), int(match.group(2))
            # `### 5.1`은 하위 절이다. 번호 뒤에 또 숫자가 오면 절이 아니다.
            after = text[match.end() : match.end() + 1]
            if level == "##":
                sections[number].append(str(path).replace("\\", "/"))
            elif len(level) == 1:
                wrong_level.append(
                    f"{path.as_posix()}: `{level} {number}.` — 절은 `##`로 적는다"
                )
            # `###`은 하위 절이므로 그냥 둔다.
            del after
    return sections, wrong_level


def check_headings(problems: list[str]) -> dict[int, list[str]]:
    sections, wrong_level = collect_sections()
    problems.extend(wrong_level)

    for number, files in sorted(sections.items()):
        if len(files) > 1:
            problems.append(
                f"§{number}이 여러 곳에 있다: {', '.join(files)} — 한 절은 한 곳에만"
            )

    if sections:
        highest = max(sections)
        gaps = [n for n in range(1, highest + 1) if n not in sections]
        if gaps:
            problems.append(
                f"절 번호에 빈 칸이 있다: {gaps} (가장 높은 번호는 §{highest}) — "
                "번호를 건너뛰면 '§N이 어디 있나'를 찾을 수 없다"
            )
    return sections


def check_links(problems: list[str]) -> None:
    for path in markdown_files():
        text = path.read_text(encoding="utf-8")
        anchors = {slug(line.lstrip("#").strip()) for line in text.split("\n") if line.startswith("#")}

        for match in LINK.finditer(text):
            target = match.group(1).strip()
            if target.startswith(("http://", "https://", "mailto:")):
                continue

            if target.startswith("#"):
                # 같은 문서의 앵커.
                if target[1:] and target[1:] not in anchors:
                    problems.append(
                        f"{path.as_posix()}: `{target}`에 해당하는 제목이 이 문서에 없다 — "
                        "다른 파일의 절이면 경로를 적어야 한다"
                    )
                continue

            file_part = target.split("#", 1)[0]
            if not file_part:
                continue
            resolved = (path.parent / file_part).resolve()
            if not resolved.exists():
                problems.append(f"{path.as_posix()}: `{target}` — 그런 파일이 없다")


def check_references(problems: list[str], sections: dict[int, list[str]]) -> None:
    seen: Counter[int] = Counter()
    for path in markdown_files():
        for match in REFERENCE.finditer(path.read_text(encoding="utf-8")):
            seen[int(match.group(1))] += 1

    dangling = sorted(n for n in seen if n not in sections)
    for number in dangling:
        problems.append(
            f"§{number}을 {seen[number]}곳에서 가리키는데 그런 절이 없다"
        )


def main() -> int:
    if not PRD.is_dir():
        print(f"{PRD}가 없다 — 저장소 뿌리에서 실행해야 한다", file=sys.stderr)
        return 2

    problems: list[str] = []
    sections = check_headings(problems)
    check_links(problems)
    check_references(problems, sections)

    if problems:
        for problem in problems:
            print(f"::error::{problem}")
        print(f"\n문서에서 {len(problems)}가지를 찾았다", file=sys.stderr)
        return 1

    print(
        f"docs are consistent: {len(sections)} PRD sections, "
        f"{len(markdown_files())} markdown files, links and §N references resolve"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
