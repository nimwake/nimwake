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

## 구현 중 확정 (`v0.0.1`)

문서만으로는 결정되지 않았고, 코드를 쓰면서 확정한 것들. PRD에 반영을 끝냈다.

| #   | 결정                                                        | 이유                                                                                     | 반영 위치                                                   |
| --- | ----------------------------------------------------------- | ---------------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| D16 | `active_requests: usize` → `leases: HashSet<LeaseId>`        | 정수 카운터는 같은 lease가 두 번 반납돼도 그대로 감소한다. 중복 반납 무시가 불가능하다.   | [§11](../../prd/02-architecture/01-core-architecture.md)     |
| D17 | `Runtime` trait에 `wait()` 추가                              | `Child`를 AppManager로 넘기지 않고 exit를 감시하려면 runtime 쪽에 대기 지점이 필요하다.   | [§18](../../prd/02-architecture/04-runtime.md)               |
| D18 | mailbox가 가득 차도 lease 반납은 반드시 전달                 | 반납을 버리면 `active_requests`가 0이 되지 못해 app이 영원히 잠들지 않는다.               | [§69](../../prd/02-architecture/06-gateway-and-requests.md)  |
| D19 | `AppCommand::Status` 추가                                    | 상태 관측 경로가 없으면 테스트가 내부 필드를 들여다봐야 한다. 나중에 `nimwake status`로 이어진다. | [§15](../../prd/02-architecture/03-concurrency.md)      |

## 구현 중 확정 (`v0.0.2`)

| #   | 결정                                                              | 이유                                                                                                       | 반영 위치                                                    |
| --- | ----------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| D20 | `ProcessHandle`의 생존이 process의 생존 조건                       | startup timeout은 `Runtime::start`를 **취소**하는 방식으로 적용된다. 취소된 future가 child를 남기면 cold start마다 orphan이 쌓인다. handle이 모두 drop되면 controller가 정리하게 해서 구조적으로 막았다. | [§70](../../prd/02-architecture/04-runtime.md)               |
| D21 | `control`을 `Arc<dyn ProcessControl>`로 handle에 넣음              | runtime이 `ProcessId → Controller` 맵을 들면 죽은 entry를 걷어낼 책임이 새로 생긴다. 그 맵 자체가 누수 경로다. handle의 동일성은 `ProcessId`가 결정하도록 `PartialEq`를 직접 구현했다. | [§70](../../prd/02-architecture/04-runtime.md)               |
| D22 | `ProcessRuntime`은 Unix 전용 (`#[cfg(unix)]`)                      | SIGTERM / SIGKILL / process group은 Windows에 대응물이 없다. Tier 1/2가 모두 Linux 계열이므로 이식 비용을 지불할 이유가 없다. | [§19](../../prd/02-architecture/04-runtime.md), [§48](../../prd/04-platform/01-environments.md) |
| D23 | stdout/stderr는 inherit이 아니라 **drain**                          | daemon의 stdout이 어디로 가는지는 운영자가 정한다. daemon이 직접 읽어 app 이름과 함께 `tracing`으로 흘리면 그 선택과 무관하게 child가 막히지 않는다. | [§19](../../prd/02-architecture/04-runtime.md)               |

### 코드 수준 주의사항

- lifecycle 시간 판단은 **`tokio::time::Instant`** 를 쓴다.
  `std::time::Instant`는 `tokio::time::pause()`의 가상 시계를 따르지 않아 idle timeout 테스트가 실시간에 묶인다.
- `stop`이 끝나면 `StopCompleted`와 `ProcessExited`가 **둘 다** 도착한다.
  먼저 온 것이 전이를 수행하고 나중 것은 stale로 무시된다. 테스트는 event 개수를 세지 말고 목표 상태까지 진행시켜야 한다.
- process group에 `SIGKILL`을 보내는 것은 **reap보다 먼저**여야 한다.
  직접 child가 zombie로 남아 있는 동안에만 그 pid(= pgid)가 예약되어 있다. 거둔 뒤에 보내면 재사용된 pid의 엉뚱한 group을 죽일 수 있다.
- 통합 테스트의 port를 `bind(:0)`으로 고르면 안 된다.
  돌려준 직후 커널이 같은 번호를 다른 연결의 ephemeral port로 내주고, readiness probe가 엉뚱한 listener에 붙는다.
  실제로 이 경로에서 테스트가 간헐적으로 깨졌다. ephemeral range 밖에서 원자 counter로 고른다.

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
