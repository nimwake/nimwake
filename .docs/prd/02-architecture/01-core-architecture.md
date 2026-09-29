# Core Architecture와 상태 모델

> **Nimwake PRD** › 아키텍처
>
> 원문 구간: `PRD.md` §9–§11

---

## 9. Core Architecture

```text
                   HTTP Request
                        │
                        ▼
                ┌───────────────┐
                │    Gateway    │
                │               │
                │ Host Router   │
                │ Reverse Proxy │
                └───────┬───────┘
                        │
                        ▼
                   AppRegistry
                        │
                        ▼
                    AppHandle
                        │
                        │ acquire()
                        ▼
                ┌──────────────┐
                │  AppManager  │
                │              │
                │ StateMachine │
                └──────┬───────┘
                       │
                       ▼
                    Runtime
                       │
                       ▼
              Application Process
```

Nimwake의 architecture에서 중요한 dependency direction은 다음과 같다.

```text
Gateway
   ↓
AppHandle
   ↓
AppManager
   ↓
Runtime
```

Gateway가 `ProcessRuntime`에 직접 접근하는 구조는 허용하지 않는다.

Gateway가 AppManager에 요청하는 연산은 `acquire()` 하나다.
→ [§26 Acquire와 RequestLease](06-gateway-and-requests.md)

---

## 10. Application Lifecycle

Core에서 사용하는 기본 lifecycle은 다음과 같다.

```text
                         acquire
                            │
                            ▼
                    ┌──────────────┐
              ┌────▶│   STARTING   │──── failure ───▶ FAILED
              │     └──────┬───────┘                    │
              │            │                            │
              │          ready                       acquire
              │            │                            │
              │            ▼                            │
       ┌──────────┐  ┌───────────┐                      │
       │ SLEEPING │  │  RUNNING  │◀─────────────────────┘
       └────▲─────┘  └─────┬─────┘
            │              │
            │    idle && active_requests == 0
            │              │
            │              ▼
            │       ┌───────────┐
            └───────│ STOPPING  │
                    └─────┬─────┘
                          │
                       acquire
                          │
                          ▼
                    queue waiter
                          │
                     stop complete
                          │
                          ▼
                       STARTING
```

FAILED로의 전이 조건:

```text
STARTING
   │
   ├── spawn failure
   ├── startup timeout
   └── readiness failure
          │
          ▼
        FAILED


RUNNING
   │
   └── unexpected process exit
          │
          ▼
        FAILED
```

`FAILED`는 terminal state가 아니다. 다음 `acquire`에서 다시 `STARTING`으로 전이한다.

---

## 11. 상태 모델

application state는 다음 형태를 가진다.

```rust
enum AppState {
    Sleeping,

    Starting {
        operation_id: OperationId,
        started_at: Instant,
        waiters: Vec<Waiter>,
    },

    Running {
        process: ProcessHandle,
        started_at: Instant,
        active_requests: usize,
        last_request_at: Instant,
        idle_generation: IdleGeneration,
    },

    Stopping {
        operation_id: OperationId,
        process_id: ProcessId,
        waiters: Vec<Waiter>,
    },

    Failed {
        error: AppError,
        failed_at: Instant,
    },
}
```

각 상태에 필요한 데이터는 반드시 해당 state 안에 위치시킨다.

특히 **startup을 기다리는 waiter는 state 바깥의 별도 구조가 아니라 state가 직접 소유한다.**

waiter를 state 밖에 두면 `FAILED`, `STOPPING`, shutdown 전이에서 waiter를 유실할 수 있다.

```text
STARTING  ─ owns ─▶ waiters
STOPPING  ─ owns ─▶ waiters
```

상태 전이 시 waiter는 반드시 다음 중 하나로 처리된다.

```text
resolve   (backend 전달)
   or
reject    (error 전달)
   or
carry     (다음 state로 이전)
```

이 모델을 통해 다음과 같은 논리적으로 잘못된 상태를 피한다.

```text
Sleeping + running ProcessHandle
Failed   + pending waiters
```

`OperationId` / `ProcessId` / `IdleGeneration`의 의미는
[§68 Lifecycle Identity](02-lifecycle-invariants.md)에서 정의한다.

---

← [Non-Goals와 Core MVP 범위](../01-product/03-scope.md) · [문서 인덱스](../README.md) · [Lifecycle Invariants와 Identity](../02-architecture/02-lifecycle-invariants.md) →
