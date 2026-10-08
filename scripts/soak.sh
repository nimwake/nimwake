#!/usr/bin/env bash
#
# 장시간 soak. 요구사항: .docs/prd/03-delivery/04-stress-tests.md (PRD §43)
#
# **Actions에서 돌리지 않는다.** 저장소가 private이므로 runner 시간이 과금되고,
# GitHub-hosted runner의 job 상한도 6시간이라 PRD §65의 24h · 72h는 애초에
# 담기지 않는다. release gate의 부하는 148초이고 soak은 그 수십 배다.
#
# 긴 구간은 수명이 긴 호스트의 일이다. Android 실기기 검증이 그 역할을 맡는다.
#
#   사용법:  scripts/soak.sh [초] [cycle 수]
#   예:      scripts/soak.sh 7200          # 2시간
#            scripts/soak.sh 86400 1000    # 24시간
#
# 중간에 멈출 때는 Ctrl-C를 쓴다. 아래 trap이 process group을 정리한다.

set -euo pipefail

SECONDS_TO_RUN="${1:-3600}"
CYCLES="${2:-1000}"

cd "$(dirname "$0")/.."

# /mnt/c에 빌드하면 느리다. WSL에서는 홈 밑을 쓴다.
if [[ -z "${CARGO_TARGET_DIR:-}" && "$(pwd)" == /mnt/* ]]; then
  export CARGO_TARGET_DIR="${HOME}/.cache/nimwake-target"
  echo "CARGO_TARGET_DIR=${CARGO_TARGET_DIR}"
fi

cargo build --release --workspace

BIN="${CARGO_TARGET_DIR:-target}/release/nimwake-bench"
OUT="soak-$(date -u +%Y%m%dT%H%M%SZ).md"

# bench는 SIGTERM에 destructor를 돌리지 않는다. 신호로 죽으면 그것이 띄운
# daemon과 application이 남는다. soak은 중간에 멈추는 일이 잦으니 정리가 필요하다.
#
# bench를 `setsid`로 띄워 자기 process group의 leader로 만든다.
# 그래야 group 단위로 정리할 수 있다. `kill -- -$$`로는 안 된다.
# 비대화형 실행에서 이 스크립트는 group leader가 아니고, 그때 `-$$`는 존재하지
# 않는 pgid여서 kill이 조용히 실패한다. 그러면 daemon이 그대로 남는다.
BENCH_PGID=""

cleanup() {
  local code=$?
  trap - INT TERM EXIT

  if [[ -n "${BENCH_PGID}" ]]; then
    echo
    echo "정리 중 (pgid ${BENCH_PGID})..."
    kill -TERM -- "-${BENCH_PGID}" 2>/dev/null || true
    sleep 2
    kill -KILL -- "-${BENCH_PGID}" 2>/dev/null || true
    sleep 1
  fi

  local left
  left=$(ps -eo comm= | grep -cE '^(nimwaked|echo-server|crash-server|group-server|slow-start-server)$' || true)
  echo "남은 프로세스: ${left}"
  [[ "${left}" == "0" ]] || echo "경고: 직접 확인이 필요하다"
  exit "${code}"
}
trap cleanup INT TERM EXIT

echo "지속 부하 ${SECONDS_TO_RUN}초 · cycle ${CYCLES}회 → ${OUT}"

setsid "${BIN}" \
  --commit "soak-$(git rev-parse --short HEAD)" \
  --stress \
  --stress-cycles "${CYCLES}" \
  --sustained-seconds "${SECONDS_TO_RUN}" \
  --max-concurrency 1 \
  --out "${OUT}" &

# setsid는 자식을 새 session의 leader로 만든다. 그 pid가 곧 pgid다.
BENCH_PGID=$!
wait "${BENCH_PGID}"

trap - INT TERM EXIT
echo
echo "리포트: ${OUT}"

# grep -c는 센 값이 0이면 종료 코드 1이다. pipefail 아래서 그대로 두면
# 아무것도 남지 않은 정상 상태가 스크립트 실패로 보인다.
left=$(ps -eo comm= | grep -cE '^nimwaked$' || true)
echo "남은 daemon: ${left}"
[[ "${left}" == "0" ]] || exit 1
