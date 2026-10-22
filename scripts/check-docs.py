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



# `[lifecycle]` 블록의 `키 = 값`. 뒤에 `# §75` 같은 주석이 붙을 수 있다.
MANIFEST_DEFAULT = re.compile(r"^([a-z_]+) = (\d+)", re.M)
# `const DEFAULT_IDLE_TIMEOUT_SECONDS: u64 = 60;`
RUST_DEFAULT = re.compile(r"const DEFAULT_([A-Z_]+)\s*:\s*\w+\s*=\s*(\d+)")

MANIFEST_DOC = PRD / "02-architecture" / "05-manifest-and-readiness.md"
SPEC_SOURCE = pathlib.Path("crates/nimwake-core/src/app/spec.rs")


def check_documented_defaults(problems: list[str]) -> int:
    """PRD §21의 manifest 예시와 코드의 기본값이 같은가.

    **테스트로는 이것을 지킬 수 없다.** 코드 상수를 테스트의 숫자와 비교하는
    것은 코드를 코드에 묶는 것이고, 문서를 바꿔도 통과한다. 운영자는 문서를
    베껴 쓰므로 둘이 어긋나면 문서가 거짓말을 한다.

    비교하는 쌍은 이름으로 찾는다 — `idle_timeout_seconds`는
    `DEFAULT_IDLE_TIMEOUT_SECONDS`다.
    """
    if not MANIFEST_DOC.is_file() or not SPEC_SOURCE.is_file():
        problems.append(
            f"기본값을 비교할 수 없다: {MANIFEST_DOC} 또는 {SPEC_SOURCE}가 없다"
        )
        return 0

    text = MANIFEST_DOC.read_text(encoding="utf-8")
    start = text.find("[lifecycle]")
    if start == -1:
        problems.append(f"{MANIFEST_DOC.as_posix()}: `[lifecycle]` 예시가 없다")
        return 0
    end = text.find("```", start)
    documented = dict(MANIFEST_DEFAULT.findall(text[start:end if end != -1 else len(text)]))

    code = {
        name: value
        for name, value in RUST_DEFAULT.findall(SPEC_SOURCE.read_text(encoding="utf-8"))
    }

    compared = 0
    for key, doc_value in sorted(documented.items()):
        constant = f"{key.upper()}"
        if constant not in code:
            problems.append(
                f"§21이 `{key} = {doc_value}`를 적는데 "
                f"`DEFAULT_{constant}` 상수가 없다 — 문서가 없는 기본값을 약속한다"
            )
            continue
        compared += 1
        if code[constant] != doc_value:
            problems.append(
                f"`{key}`의 기본값이 어긋난다: §21은 {doc_value}, "
                f"코드는 {code[constant]} — 운영자는 문서를 베껴 쓴다"
            )
    return compared

# 프로그램 출력을 흉내 낸 버전. `nimwaked 0.1.4`, `"version":"0.1.4"`, `Version: `v0.1.4``
OUTPUT_VERSION = re.compile(
    r"nimwaked? (\d+\.\d+\.\d+)"
    r'|"version"\s*:\s*"(\d+\.\d+\.\d+)"'
    r"|Version: `v(\d+\.\d+\.\d+)`"
)
# `[workspace.package]`의 version.
CARGO_VERSION = re.compile(r'^version = "(\d+\.\d+\.\d+)"', re.M)

# 역사 기록이다. 그때 그 버전이 맞으므로 **고치면 거짓이 된다.**
HISTORICAL = PRD.parent / "phases"


def current_versions(problems: list[str]) -> int:
    """지금 버전이라고 적은 곳이 Cargo.toml과 같은가.

    릴리스마다 손으로 챙기던 곳이 다섯이었고 **둘이 뒤처져 있었다** — §74의
    `nimwaked 0.1.0`과 §85의 `"version":"0.1.1"`. 둘 다 "지금 daemon이 이렇게
    답한다"고 적은 곳이라, 운영자가 보고 자기 설치가 낡았다고 오해한다.

    `.docs/phases/`는 보지 않는다. 그쪽의 `nimwaked 0.0.1`은 그때의 기록이고
    **고치면 거짓이 된다.**

    `--version`의 출력 자체는 `CARGO_PKG_VERSION`에서 오므로 테스트가 지킨다.
    여기서 보는 것은 **문서가 베껴 적은 값**이다.
    """
    cargo = pathlib.Path("Cargo.toml")
    if not cargo.is_file():
        problems.append("Cargo.toml이 없다 — 버전을 대조할 수 없다")
        return 0
    match = CARGO_VERSION.search(cargo.read_text(encoding="utf-8"))
    if not match:
        problems.append("Cargo.toml에서 version을 찾지 못했다")
        return 0
    current = match.group(1)

    targets = [pathlib.Path("README.md")]
    targets.extend(sorted(PRD.rglob("*.md")))

    checked = 0
    for path in targets:
        if not path.is_file() or HISTORICAL in path.parents:
            continue
        for found in OUTPUT_VERSION.finditer(path.read_text(encoding="utf-8")):
            written = next(group for group in found.groups() if group)
            checked += 1
            if written != current:
                problems.append(
                    f"{path.as_posix()}: `{found.group(0)}`이 지금 버전과 어긋난다 — "
                    f"Cargo.toml은 {current}다. 운영자가 보고 자기 설치가 낡았다고 "
                    "오해한다 (역사 기록이면 .docs/phases/로 옮긴다)"
                )
    return checked


# README의 저장소 구조 트리에서 `crates/` 아래의 이름들.
TREE_CRATE = re.compile(r"^│\s+[├└]── (nimwake[a-z-]*)/", re.M)


def crate_list(problems: list[str]) -> int:
    """README가 적은 crate 목록이 실제와 같은가.

    **README가 `nimwake-cli/`를 적어 두었는데 그런 crate가 없었다.** CLI는
    `nimwaked`의 두 번째 바이너리다. 대신 `nimwake-bench`와 `nimwake-fixtures`가
    빠져 있었다 — 세 개가 틀렸다.

    처음 보는 사람이 가장 먼저 읽는 지도다. 없는 디렉터리를 찾게 만들면 안 된다.
    """
    readme = pathlib.Path("README.md")
    crates = pathlib.Path("crates")
    if not readme.is_file() or not crates.is_dir():
        problems.append("README.md 또는 crates/가 없다 — crate 목록을 대조할 수 없다")
        return 0

    actual = {path.name for path in crates.iterdir() if (path / "Cargo.toml").is_file()}
    written = set(TREE_CRATE.findall(readme.read_text(encoding="utf-8")))

    if not written:
        problems.append(
            "README의 저장소 구조에서 crate 목록을 찾지 못했다 — "
            "트리 모양이 바뀌었으면 이 검사도 같이 고친다"
        )
        return 0

    for name in sorted(written - actual):
        problems.append(
            f"README가 `crates/{name}/`을 적는데 그런 crate가 없다 — "
            "처음 보는 사람이 없는 디렉터리를 찾는다"
        )
    for name in sorted(actual - written):
        problems.append(
            f"`crates/{name}/`이 있는데 README의 구조에 없다 — "
            "지도에 없는 것은 읽는 사람에게 없는 것이다"
        )
    return len(actual)


def main() -> int:
    if not PRD.is_dir():
        print(f"{PRD}가 없다 — 저장소 뿌리에서 실행해야 한다", file=sys.stderr)
        return 2

    problems: list[str] = []
    sections = check_headings(problems)
    check_links(problems)
    check_references(problems, sections)
    defaults = check_documented_defaults(problems)
    versions = current_versions(problems)
    crates = crate_list(problems)

    if problems:
        for problem in problems:
            print(f"::error::{problem}")
        print(f"\n문서에서 {len(problems)}가지를 찾았다", file=sys.stderr)
        return 1

    print(
        f"docs are consistent: {len(sections)} PRD sections, "
        f"{len(markdown_files())} markdown files, links and §N references resolve, "
        f"{defaults} documented defaults match the code, "
        f"{versions} written-out versions match Cargo.toml, "
        f"README lists all {crates} crates"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
