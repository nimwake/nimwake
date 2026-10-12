#!/usr/bin/env python3
"""soak CSV에서 PRD §51의 관찰 항목을 계산한다.

사용법:

    python3 scripts/soak-report.py soak.csv

판정하지 않는다. **숫자를 내고, CSV가 답할 수 없는 것을 적는다.**
합격선은 PRD §51과 `nimwake-android`가 정한다.

CSV는 `nimwake-android`의 host script가 쓴다. 열은 이렇다.

    epoch,time,http,seconds,app_pid,apps_before,daemon_pid,
    daemon_rss_kb,state,battery_pct,battery_temp_c,free_mb

열이 늘어나도 동작한다. 이름으로 찾고, 없는 열은 "측정 안 됨"으로 적는다.
"""

from __future__ import annotations

import csv
import sys
from pathlib import Path

# PRD §51이 정한 관찰 항목. CSV가 답할 수 있는 것과 없는 것을 함께 적는다.
OBSERVATIONS = [
    ("daemon survival", "daemon_pid"),
    ("Android background process killing", "daemon_pid"),
    ("memory usage", "daemon_rss_kb"),
    ("battery usage", "battery_pct"),
    ("temperature", "battery_temp_c"),
    ("wake reliability", "http"),
    ("network recovery", None),
    ("orphan process", "apps_before"),
    # `free_mb`는 시스템 전체의 df 여유다. nimwake 몫을 가려내지 못한다.
    ("storage growth (nimwake 몫)", None),
    ("repeated lifecycle stability", "state"),
]


def numbers(rows: list[dict[str, str]], column: str) -> list[float]:
    """한 열에서 숫자만 뽑는다. `NA`와 빈 값은 버린다."""
    out = []
    for row in rows:
        raw = (row.get(column) or "").strip()
        if not raw or raw == "NA":
            continue
        try:
            out.append(float(raw))
        except ValueError:
            continue
    return out


def percentile(values: list[float], fraction: float) -> float:
    """가장 가까운 순위. 표본이 적을 때 보간은 없는 정밀도를 흉내낸다."""
    if not values:
        return float("nan")
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, round(fraction * (len(ordered) - 1))))
    return ordered[index]


def slope_per_hour(rows: list[dict[str, str]], column: str) -> float:
    """최소제곱 기울기(단위/시간).

    처음과 끝만 보면 중간의 봉우리를 놓치고, 한 번의 튐에 결론이 뒤집힌다.
    """
    points = []
    for row in rows:
        raw = (row.get(column) or "").strip()
        epoch = (row.get("epoch") or "").strip()
        if not raw or raw == "NA" or not epoch:
            continue
        try:
            points.append((float(epoch), float(raw)))
        except ValueError:
            continue

    if len(points) < 2:
        return float("nan")

    t0 = points[0][0]
    xs = [(t - t0) / 3600.0 for t, _ in points]
    ys = [v for _, v in points]
    n = len(xs)
    mx = sum(xs) / n
    my = sum(ys) / n
    denominator = sum((x - mx) ** 2 for x in xs)
    if denominator == 0:
        return float("nan")
    return sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / denominator


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2

    path = Path(sys.argv[1])
    with path.open(newline="", encoding="utf-8") as handle:
        rows = list(csv.DictReader(handle))

    if not rows:
        print(f"{path}: 표본이 없다")
        return 1

    columns = set(rows[0].keys())
    epochs = numbers(rows, "epoch")
    hours = (max(epochs) - min(epochs)) / 3600.0 if len(epochs) >= 2 else 0.0

    print(f"# soak 리포트 — {path.name}\n")
    print(f"표본 {len(rows)}개 · 경과 {hours:.1f}h")
    print(f"시작 {rows[0].get('time', '?')} · 끝 {rows[-1].get('time', '?')}\n")

    # ── daemon survival: pid가 하나여야 한다. 둘이면 재시작한 것이다.
    pids = sorted({(r.get("daemon_pid") or "").strip() for r in rows} - {"", "NA"})
    print("## daemon")
    print(f"  daemon_pid 종류   {' '.join(pids) if pids else '측정 안 됨'}")
    print(f"  재시작            {'없다' if len(pids) == 1 else f'{len(pids) - 1}회 (pid가 바뀌었다)'}")

    # ── wake reliability
    codes: dict[str, int] = {}
    for row in rows:
        codes[(row.get("http") or "?").strip()] = codes.get((row.get("http") or "?").strip(), 0) + 1
    non_200 = sum(count for code, count in codes.items() if code != "200")
    print("\n## 요청")
    for code, count in sorted(codes.items()):
        print(f"  HTTP {code:<16} {count}회")
    print(f"  비200             {non_200}회")

    latencies = numbers(rows, "seconds")
    if latencies:
        print(
            f"  응답 p50/p95/max  {percentile(latencies, 0.5) * 1000:.1f} / "
            f"{percentile(latencies, 0.95) * 1000:.1f} / {max(latencies) * 1000:.1f} ms"
        )

    # ── memory: 기울기로 본다
    print("\n## 메모리")
    if "daemon_rss_kb" in columns:
        values = numbers(rows, "daemon_rss_kb")
        if values:
            drift = slope_per_hour(rows, "daemon_rss_kb")
            print(
                f"  daemon RSS        {values[0]:.0f} → {values[-1]:.0f} KB "
                f"(min {min(values):.0f} · max {max(values):.0f} · 기울기 {drift:+.2f} KB/h)"
            )
        else:
            print("  daemon RSS        값이 없다")
    else:
        print("  daemon RSS        측정 안 됨")

    # ── storage: `free_mb`는 **디스크**다. 메모리가 아니다.
    #
    # host script가 `df -Pk $HOME`로 읽는다. 메모리 절에 두면 오해를 부른다 —
    # 실제로 한 번 그렇게 읽고 "메모리가 1.2GB 줄었다"로 잘못 읽었다.
    #
    # 그리고 이 값은 **시스템 전체**다. 줄어든 양을 nimwake에 돌릴 수 없다.
    # nimwake 자신의 발자국은 로그 크기로 따로 봐야 한다.
    print("\n## 저장 공간 (시스템 전체 — nimwake 몫이 아니다)")
    if "free_mb" in columns:
        values = numbers(rows, "free_mb")
        if values:
            drift = slope_per_hour(rows, "free_mb")
            used = values[0] - values[-1]
            print(
                f"  df 여유           {values[0]:.0f} → {values[-1]:.0f} MB "
                f"(기울기 {drift:+.2f} MB/h · 소모 {used:.0f} MB)"
            )
            print("  ※ 다른 앱과 OS가 함께 쓴다. nimwake 몫은 로그 크기로 따로 재야 한다")
        else:
            print("  df 여유           값이 없다")
    else:
        print("  df 여유           측정 안 됨")

    # ── battery / temperature
    print("\n## 배터리와 온도")
    battery = numbers(rows, "battery_pct")
    if battery:
        rate = slope_per_hour(rows, "battery_pct")
        print(f"  battery_pct       {battery[0]:.0f}% → {battery[-1]:.0f}% ({rate:+.3f} %/h)")
    else:
        print("  battery_pct       측정 안 됨")
    temps = numbers(rows, "battery_temp_c")
    if temps:
        print(f"  battery_temp_c    {min(temps):.1f} ~ {max(temps):.1f}°C (p95 {percentile(temps, 0.95):.1f})")
    else:
        print("  battery_temp_c    측정 안 됨")

    # ── repeated lifecycle: 어떤 상태를 거쳤나
    print("\n## 생애주기")
    states: dict[str, int] = {}
    for row in rows:
        states[(row.get("state") or "?").strip()] = states.get((row.get("state") or "?").strip(), 0) + 1
    for state, count in sorted(states.items(), key=lambda pair: -pair[1]):
        print(f"  {state:<17} {count}회")

    app_pids = sorted({(r.get("app_pid") or "").strip() for r in rows} - {"", "NA"})
    print(f"  app process 종류  {len(app_pids)}개 (요청마다 깨우고 재웠다면 많아야 정상이다)")

    orphans = numbers(rows, "apps_before")
    if orphans:
        print(
            f"  요청 전 잔존 app  max {max(orphans):.0f}개 "
            f"(0이 아니면 이전 요청의 process가 남아 있었다는 뜻이다)"
        )

    # ── CSV가 답할 수 없는 것
    print("\n## 이 CSV로 답할 수 없는 것 (PRD §51)")
    for name, column in OBSERVATIONS:
        if column is None:
            print(f"  {name}")
        elif column not in columns:
            print(f"  {name} — `{column}` 열이 없다")

    print("\n판정하지 않는다. 합격선은 PRD §51과 `nimwake-android`가 정한다.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
