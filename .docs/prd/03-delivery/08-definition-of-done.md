# Core MVP Definition of Done

> **Nimwake PRD** › 딜리버리
>
> 원문 구간: `PRD.md` §65

---

## 65. Core MVP Definition of Done

다음을 모두 만족하면 Core MVP를 완료한 것으로 정의한다.

### 런타임

- Rust daemon 실행
- manifest parsing
- manifest validation (unique host / unique port / port != gateway port)
- Runtime abstraction
- real child process spawn
- `Child` 소유권이 `ProcessController` 한 곳에 있음
- stdout/stderr를 inherit하거나 drain (방치된 pipe 없음)
- process monitoring
- TCP readiness
- startup timeout

### 게이트웨이

- Host routing
- HTTP reverse proxy
- cold start
- concurrent cold-start deduplication
- `acquire()` 단일 API (backend 확보 + active request 예약이 원자적)
- request tracking이 lease 발급/반납으로만 변경됨
- mailbox / pending waiter 상한 초과 시 `503`

### Scale-to-Zero

- idle shutdown
- stale timer protection
- graceful shutdown
- forced shutdown
- process crash detection
- retry after failure

### 동시성 불변식

- no duplicate processes
- no lost startup waiter
- no lifecycle deadlock
- no orphan process after stress test
- stale `StartCompleted` / `StopCompleted`가 state를 변경하지 않음
- 교체된 process의 `ProcessExited`가 state를 변경하지 않음
- daemon 종료 후 managed child process 잔존 0

### 테스트

- unit tests pass
- concurrency tests pass
- E2E tests pass
- cold-start storm passes
- lifecycle stress passes
- release CI passes

### 계측

- cold start가 구간별 breakdown으로 기록됨
- warm path proxy overhead가 direct 대비로 기록됨
- concurrency sweep 결과가 기록됨
- performance report artifact가 release마다 생성됨

### Android Device

[§50](../04-platform/02-android.md)의 병렬 검증 결과. `nimwake-android` 검증 리포트로 판정한다.

- Install
- Boot recovery
- Cold start
- Scale-to-zero
- Daemon recovery
- Network recovery
- 24h soak
- 72h soak

---

이 상태를:

```text
Nimwake v0.1.0 — Core MVP
```

로 정의한다.

---

← [Performance Goals와 Regression](../03-delivery/07-performance-baseline.md) · [문서 인덱스](../README.md) · [Target Environments와 외부 의존성](../04-platform/01-environments.md) →
