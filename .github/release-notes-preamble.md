<!--
릴리스 노트의 머리말. `release.yml`의 publish job이 성능 리포트 앞에 붙인다.

링크는 `../` 없이 저장소 루트 기준으로 쓴다. 릴리스 페이지에서 그렇게 풀린다.
제약 목록의 출처는 v0.1.0 마일스톤과 PRD다. 여기서 새로 정하지 않는다.
-->

# Nimwake Core

Scale-to-Zero application lifecycle. 요청이 오면 깨우고, 쓰지 않으면 재운다.

## 보장하지 않는 것

릴리스마다 **먼저** 적는다. 쓰는 쪽이 성능 표보다 이것을 먼저 읽어야 한다.

| | |
| --- | --- |
| process adoption | daemon을 재시작하면 기존 application process를 재발견하거나 인수하지 않는다. port가 이미 쓰이고 있으면 시작을 거부한다(502) |
| WebSocket / SSE | 보장하지 않는다. 대용량 스트리밍 업로드도 마찬가지다 |
| 성능 수치 | baseline이며 SLA가 아니다. runner 편차가 크다 |
| dynamic port allocation | 없다. manifest의 static port를 쓴다 |

근거: [PRD §24](.docs/prd/02-architecture/06-gateway-and-requests.md) ·
[§71](.docs/prd/02-architecture/08-failure-handling.md) ·
[§73](.docs/prd/03-delivery/07-performance-baseline.md)

## 측정되지 않은 것

자동 테스트를 통과한 것은 **Core lifecycle의 정확성**이다. 제품의 가용성이 아니다.

| | |
| --- | --- |
| 실제 서비스의 cold start | Next.js 기동 206ms · 첫 응답 396ms. 다만 한 페이지짜리 앱이라 하한이다 |
| readiness의 의미 | TCP connect가 되는 시점을 쓴다. Next.js는 port를 열고도 첫 응답까지 190ms 더 걸린다 |
| 동시 기동의 자원 한계 | Next.js 앱 하나가 118MB. 2GB 기기면 10~14개가 한계인데 제한 장치가 없다 |
| 장시간 운용 | CI 측정 구간은 분 단위다. 실기기에서 50.4시간을 측정했다(daemon 재시작 0회 · 요청 실패 0/604). 72h 판정은 `nimwake-android` 소관 |
| manifest 재적재 | `SIGHUP`으로 된다. 돌고 있는 application은 건드리지 않는다 (PRD §76) |
| crash-loop backoff | 60초 안에 실패 3회면 재시도를 미룬다. 1초부터 2배씩, 상한 30초 (PRD §77) |
| 운영 도구 | `nimwake status`도 admin IPC도 없다 |

[완료 체크리스트](.docs/phases/phase-1/04-acceptance.md)에 항목별 근거가 있다.

## 지원 환경

```text
Tier 1   Linux x86_64 · Linux ARM64
Tier 2   Android + Termux + proot · Raspberry Pi · ARM SBC
```

daemon은 Unix 전용이다. 종료 계약(SIGTERM → timeout → SIGKILL)과 process group
정리가 POSIX 신호 위에 서 있다.

Gateway는 기본적으로 `127.0.0.1:8080`에 bind한다. 외부 노출은 opt-in이며
Core는 TLS도 인증도 제공하지 않는다. → [PRD §74](.docs/prd/04-platform/03-host-interface.md)

---
