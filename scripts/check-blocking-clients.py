#!/usr/bin/env python3
"""단일 스레드 `#[tokio::test]`가 blocking HTTP client를 직접 부르는 자리를 찾는다.

왜 막는가. 테스트가 쓰는 HTTP client는 blocking이다. 단일 스레드 runtime에서
직접 부르면 같은 runtime에 spawn된 task가 **굶는다.** gateway task가 그렇고,
`ProcessRuntime`이 띄우는 stdout/stderr drain task가 그렇다. (PRD §19)

굶으면 pipe가 차고 child가 멈춘다. 그런데 증상은 "client 읽기 시간 초과"로
나타나므로 원인을 찾기 어렵다. 실제로 §19를 지킨다던 테스트가 이 함정 때문에
drain을 제거해도 통과하고 있었다.

`gateway.rs`의 모듈 doc이 이미 "항상 `spawn_blocking`으로 부른다"고 적었는데
`process_runtime.rs`가 밟고 있었다. 한 파일의 경고는 옆 파일에 닿지 않는다.
그래서 검사로 옮긴다.

통과 조건은 둘 중 하나다.

    flavor = "multi_thread"       runtime에 여유 thread가 있다
    spawn_blocking으로 감싼다      blocking 호출이 worker를 막지 않는다
"""

import pathlib
import re
import sys

# blocking client. `registry.get(...)` 같은 메서드 호출은 제외해야 하므로
# 앞에 `.`이나 식별자 문자가 없는 것만 본다.
CLIENT = re.compile(r"(?<![.\w])(http_get|request_within|request|get)\s*\(")

# flavor가 없는 `#[tokio::test]`만 고른다.
SINGLE_THREADED = re.compile(r"#\[tokio::test\]\s*\n\s*async fn (\w+)")

# 주석은 걷어낸다. 설명에 적힌 `spawn_blocking`에 속지 않기 위해서다.
#
# 처음에는 본문에 그 단어가 있는지만 보았다. 그런데 "blocking client는
# spawn_blocking으로 부른다"는 **주석** 때문에 위반을 통과시켰다. 일부러
# 깨뜨려 보지 않았으면 몰랐을 것이다.
LINE_COMMENT = re.compile(r"//[^\n]*")

# 호출만 센다. 단어가 아니라 `spawn_blocking(` 형태여야 한다.
SPAWN_BLOCKING = re.compile(r"spawn_blocking\s*\(")


def violations(root):
    found = []
    for path in sorted(root.rglob("*.rs")):
        source = path.read_text(encoding="utf-8", errors="replace")
        for match in SINGLE_THREADED.finditer(source):
            name = match.group(1)
            start = match.end()
            end = source.find("\n#[", start)
            raw = source[start : end if end != -1 else len(source)]
            body = LINE_COMMENT.sub("", raw)

            if SPAWN_BLOCKING.search(body):
                continue

            calls = sorted({m.group(1) for m in CLIENT.finditer(body)})
            if calls:
                found.append((str(path).replace("\\", "/"), name, calls))
    return found


def main():
    found = violations(pathlib.Path("crates"))
    if not found:
        print("no single-threaded test calls a blocking client directly")
        return 0

    print("blocking client called on a single-threaded runtime:", file=sys.stderr)
    for path, name, calls in found:
        print("  %s" % path, file=sys.stderr)
        print("      %s  calls %s" % (name, ", ".join(calls)), file=sys.stderr)
    print(file=sys.stderr)
    print(
        'fix: add `flavor = "multi_thread"` or wrap the call in'
        " `spawn_blocking` — see PRD §19",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
