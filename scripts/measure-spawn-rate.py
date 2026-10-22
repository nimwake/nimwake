#!/usr/bin/env python3
"""backoff 없이 실패하는 app의 spawn 속도를 센다. (PRD §77)

§77은 처음에 "측정된 상한은 초당 623회"라고 적었는데, **그 수치의 출처가 그것을
상한이라고 하지 않았다** — client가 그만큼밖에 밀어넣지 못해서 나온 값이다.
이 스크립트가 그것을 다시 재고, 범위와 조건을 같이 낸다.

    WORKERS=1  python3 -I scripts/measure-spawn-rate.py 4
    WORKERS=32 python3 -I scripts/measure-spawn-rate.py 3

**backoff를 끈 빌드가 필요하다.** 지금 코드는 3회 실패하면 멈추므로 상한을 볼 수
없다. `FAILURE_THRESHOLD`를 크게 두고 release로 빌드한 뒤 돌린다.

    # crates/nimwake-core/src/app/backoff.rs
    pub(crate) const FAILURE_THRESHOLD: usize = 1_000_000;

**동시 1에서는 요청 수와 spawn 수가 같아진다** — 그때 재는 것은 Core가 아니라
클라이언트다. 동시 연결을 늘려야 Core가 평평해지는 지점이 보인다. 늘린 만큼
요청이 늘어도 spawn이 늘지 않으면 §20의 기동 중복 제거가 흡수하고 있는 것이다.

기기(전화기)에서 다시 재야 한다. 개발 기계의 수치는 자릿수만 말해 준다.
"""

import http.client
import os
import pathlib
import socket
import subprocess
import sys
import time

TARGET = os.environ.get("CARGO_TARGET_DIR", os.path.expanduser("~/.cache/nimwake-target"))
BIN = pathlib.Path(TARGET) / "release"
WINDOW = 3.0
WORKERS = int(os.environ.get("WORKERS", "8"))


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def one_run(root: pathlib.Path, index: int) -> float:
    app_port = free_port()
    apps = root / f"run{index}"
    app = apps / "broken"
    app.mkdir(parents=True, exist_ok=True)
    (app / "nimwake.toml").write_text(
        f'name = "broken{index}"\n\n'
        f"[server]\n"
        f'host = "broken.localhost"\n'
        f"port = {app_port}\n\n"
        f"[runtime]\n"
        # 즉시 실패한다 — 없는 파일이다.
        f'command = ["/nonexistent/never-runs"]\n'
        f'working_dir = "."\n\n'
        f"[lifecycle]\n"
        f"startup_timeout_seconds = 5\n",
        encoding="utf-8",
    )

    log = root / f"out{index}.log"
    with log.open("w") as sink:
        daemon = subprocess.Popen(
            [str(BIN / "nimwaked"), "--apps", str(apps), "--port", "0"],
            stdout=sink,
            stderr=subprocess.STDOUT,
        )

    gateway = None
    for _ in range(100):
        text = log.read_text(errors="replace")
        if "listening on" in text:
            gateway = int(text.split("listening on http://127.0.0.1:")[1].split()[0])
            break
        time.sleep(0.1)
    if gateway is None:
        daemon.terminate()
        return -1.0

    # 창이 열린 동안 끊임없이 요청한다. 매 요청이 기동을 재시도한다.
    #
    # **여러 연결로 보낸다.** 한 연결로 순차 요청하면 왕복 지연이 먼저 막아서
    # 요청 수와 spawn 수가 1:1이 되고, 그러면 재는 것이 Core의 spawn 상한이
    # 아니라 클라이언트의 속도다.
    import threading

    deadline = time.monotonic() + WINDOW
    counts = [0] * WORKERS

    def hammer(slot: int) -> None:
        while time.monotonic() < deadline:
            try:
                conn = http.client.HTTPConnection("127.0.0.1", gateway, timeout=5)
                conn.request("GET", "/", headers={"Host": "broken.localhost"})
                conn.getresponse().read()
                conn.close()
                counts[slot] += 1
            except OSError:
                return

    threads = [threading.Thread(target=hammer, args=(i,)) for i in range(WORKERS)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    attempts = sum(counts)

    daemon.terminate()
    try:
        daemon.wait(timeout=30)
    except subprocess.TimeoutExpired:
        daemon.kill()

    # spawn 시도를 로그에서 센다.
    text = log.read_text(errors="replace")
    # 시도당 한 줄이다. `failed to spawn`은 transition과 gateway 양쪽에 나오므로
    # 그것으로 세면 두 배가 된다.
    spawns = text.count("startup failed")
    print(f"    {index}회차: 요청 {attempts}개 · spawn 시도 {spawns}개 · {spawns / WINDOW:.0f}/s")
    return spawns / WINDOW


def main() -> int:
    runs = int(sys.argv[1]) if len(sys.argv) > 1 else 4
    root = pathlib.Path("/tmp/nwspawn")
    root.mkdir(parents=True, exist_ok=True)
    rates = [r for r in (one_run(root, i + 1) for i in range(runs)) if r > 0]
    if not rates:
        print("  측정 실패")
        return 1
    print(f"  ==== {len(rates)}회 ====")
    print(f"    범위  {min(rates):.0f} ~ {max(rates):.0f} /s")
    print(f"    중간값 {sorted(rates)[len(rates) // 2]:.0f} /s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
