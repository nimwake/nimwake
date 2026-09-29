# 설계 결정 기록

> **Phase 1 — Core MVP** › 설계 결정
>
> 구현 착수 전 설계 리뷰에서 나온 지적과, 각 항목이 PRD 어디에 반영되었는지의 추적표.

---

이 문서는 **결정의 이력**을 남기기 위한 것이다.
결정의 내용 자체는 PRD가 소유하며, 여기서는 "무엇을 왜 바꿨고 어디에 반영했는가"만 기록한다.

---

## P0 — 구현 시작 전 반드시 반영

| #   | 결정                                                     | 반영 위치                                                                 | 마일스톤 |
| --- | -------------------------------------------------------- | ------------------------------------------------------------------------- | -------- |
| D1  | `ensure_running()` + `RequestStarted` → 원자적 `acquire()` / `RequestLease` | [§26](../../prd/02-architecture/06-gateway-and-requests.md), [Invariant 7](../../prd/02-architecture/02-lifecycle-invariants.md) | `v0.0.1` |
| D2  | startup / shutdown에 `OperationId` 부여                  | [§68](../../prd/02-architecture/02-lifecycle-invariants.md), [§15](../../prd/02-architecture/03-concurrency.md) | `v0.0.1` |
| D3  | waiter를 `AppState`가 직접 소유                          | [§11](../../prd/02-architecture/01-core-architecture.md)                   | `v0.0.1` |
| D4  | `ProcessExited`에 `ProcessId` 포함                       | [§32](../../prd/02-architecture/08-failure-handling.md), [§68](../../prd/02-architecture/02-lifecycle-invariants.md) | `v0.0.2` |
| D5  | `tokio::process::Child` 소유권을 `ProcessController` 단일화 | [§70](../../prd/02-architecture/04-runtime.md)                            | `v0.0.2` |

### D1이 가장 중요한 이유

`ensure_running()`은 "프로세스가 살아 있다"까지만 보장하고,
그 직후 idle shutdown되지 않는다는 보장은 하지 못한다.

```text
ensure_running()
      ↓
   RUNNING
             ← idle timer 발화
   STOPPING
      ↓
RequestStarted   ← 종료 중인 process로 proxy
```

`acquire()`가 RUNNING 확보와 active request 예약을 **하나의 actor transition**으로 처리하면
이 race가 구조적으로 사라진다.

---

## P1 — `v0.0.1`~`v0.0.3` 구간에 반영

| #   | 결정                                        | 반영 위치                                                          | 마일스톤 |
| --- | ------------------------------------------- | ------------------------------------------------------------------ | -------- |
| D6  | AppManager mailbox를 bounded channel로      | [§14](../../prd/02-architecture/03-concurrency.md), [§69](../../prd/02-architecture/06-gateway-and-requests.md) | `v0.0.1` |
| D7  | pending waiter 상한 (`max_pending_requests`) → `503` | [§69](../../prd/02-architecture/06-gateway-and-requests.md)  | `v0.0.3` |
| D8  | daemon–child lifetime 정책 (process group)  | [§71](../../prd/02-architecture/08-failure-handling.md), [Invariant 10](../../prd/02-architecture/02-lifecycle-invariants.md) | `v0.0.2` |
| D9  | manifest validation (host/port 충돌 거부)   | [§21](../../prd/02-architecture/05-manifest-and-readiness.md)       | `v0.0.1` |
| D10 | stdout/stderr는 inherit 또는 drain          | [§19](../../prd/02-architecture/04-runtime.md)                      | `v0.0.2` |
| D11 | 구간별 latency instrumentation              | [§72](../../prd/03-delivery/05-instrumentation.md)                  | `v0.0.4` |

### D6 + D7이 함께 필요한 이유

mailbox만 제한해도 충분하지 않다.
AppManager가 command를 빠르게 소비하면서 waiter를 계속 누적시킬 수 있다.

```text
10,000 cold-start requests

mailbox capacity = 128

AppManager가 계속 consume

        ↓

waiters = 10,000      ← mailbox 제한만으로는 막지 못한다
```

---

## P2 — Core MVP 이후

| #   | 항목                                    | 처리                                                            |
| --- | --------------------------------------- | --------------------------------------------------------------- |
| D12 | HTTP body 크기 상한 / long-lived 정책   | [§24](../../prd/02-architecture/06-gateway-and-requests.md)에 계약만 명시, 상세는 Post-MVP |
| D13 | crash-loop backoff                      | [§33](../../prd/02-architecture/08-failure-handling.md) · [§63](../../prd/05-beyond-mvp/03-long-term-roadmap.md) |
| D14 | dynamic port allocation                 | [§21](../../prd/02-architecture/05-manifest-and-readiness.md) · [§63](../../prd/05-beyond-mvp/03-long-term-roadmap.md) |
| D15 | process adoption (daemon restart 후 인수) | [§71](../../prd/02-architecture/08-failure-handling.md)에서 명시적으로 제외, Phase 2 재검토 |

---

## 유지하기로 한 결정

| 항목                          | 판단                                                              |
| ----------------------------- | ----------------------------------------------------------------- |
| per-app actor 모델            | 유지. 이 프로젝트 규모에 적합                                      |
| Runtime abstraction           | 유지. adapter 교체 시 lifecycle semantics 불변                     |
| raw process runtime           | 유지                                                               |
| Gateway와 lifecycle 분리      | 유지. `acquire()` 하나로 표면을 더 좁힘                            |
| STOPPING 취소 불가 정책       | 유지. SIGTERM 이후 되돌리기는 application마다 의미가 달라진다      |
| static port                   | 유지하되 load 시점 validation 필수                                 |
| 성능 수치를 release gate로 안 씀 | 유지. 공유 runner 편차 때문 ([§73](../../prd/03-delivery/07-performance-baseline.md)) |

구조적 재설계가 필요한 결함은 발견되지 않았다.

---

← [완료 체크리스트](04-acceptance.md) · [페이즈 개요](README.md) · [페이즈 로드맵](../README.md)
