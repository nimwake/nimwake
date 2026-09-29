# Concurrency Model과 Cold Start

> **Nimwake PRD** › 아키텍처
>
> 원문 구간: `PRD.md` §13–§17

---

## 13. Concurrency Model

각 application은 하나의 독립적인 lifecycle actor로 관리한다.

```text
                  AppRegistry
                      │
          ┌───────────┼───────────┐
          ▼           ▼           ▼

      AppManager   AppManager   AppManager
         blog         api        portfolio
```

각 AppManager는 application lifecycle event를 순차적으로 처리한다.

```text
Acquire ────────────────┐
StartCompleted ─────────┤
StopCompleted ──────────┤
ProcessExited ──────────┼──▶ AppManager ──▶ AppState
RequestFinished ────────┤
IdleTimeout ────────────┤
CLI Event ──────────────┤
Memory Event (Future) ──┤
Shutdown ───────────────┘
```

이를 통해 여러 subsystem이 shared mutable state를 직접 수정하는 구조를 피한다.

state transition이 직렬화되므로 codebase 전반에 lock을 분산시키지 않아도 된다.

---

## 14. AppManager

개념적인 구조:

```rust
struct AppManager {
    spec: Arc<AppSpec>,
    tx: mpsc::Sender<AppCommand>,
}
```

mailbox는 반드시 bounded channel이다.
→ [§69 Backpressure와 Overload](06-gateway-and-requests.md)

```rust
let (tx, rx) = mpsc::channel(DEFAULT_MAILBOX_CAPACITY);
```

실제 lifecycle state는 actor task 내부에서 소유한다.

```rust
async fn app_loop(
    spec: AppSpec,
    mut rx: mpsc::Receiver<AppCommand>,
) {
    let mut state = AppState::Sleeping;

    while let Some(command) = rx.recv().await {
        // state transition
    }
}
```

---

## 15. Command / Event Model

command model:

```rust
enum AppCommand {
    Acquire {
        reply: oneshot::Sender<Result<RequestLease>>,
    },

    RequestFinished {
        lease_id: LeaseId,
    },

    StartCompleted {
        operation_id: OperationId,
        result: Result<StartedProcess>,
    },

    StopCompleted {
        operation_id: OperationId,
        result: Result<()>,
    },

    ProcessExited {
        process_id: ProcessId,
        status: ExitStatus,
    },

    IdleTimeout {
        generation: IdleGeneration,
    },

    Stop {
        reason: StopReason,
    },

    Shutdown,
}
```

설계 규칙:

1. 외부 요청과 내부 lifecycle event가 모두 AppManager를 통해 처리된다.
2. **actor 밖에서 발생한 모든 event는 identity를 동반한다.**
   identity가 현재 state와 불일치하면 무시한다. → [Invariant 8](02-lifecycle-invariants.md)
3. `EnsureRunning`과 `RequestStarted`를 분리하지 않는다.
   두 연산은 `Acquire` 하나로 합쳐 원자적으로 처리한다. → [Invariant 7](02-lifecycle-invariants.md)

---

## 16. Concurrent Cold Start

sleeping application에 동시에 여러 요청이 들어올 수 있다.

```text
Request A ─┐
Request B ─┼──▶ AppManager
Request C ─┤
...        │
Request N ─┘
```

첫 `Acquire`가 도착하면:

```text
SLEEPING
   ↓
STARTING { operation_id, waiters }
```

으로 전환하고 `Runtime.start()`를 한 번만 실행한다.

STARTING 중 들어오는 `Acquire`는 현재 state가 소유한 waiter 목록에 등록한다.

```text
STARTING
   │
   └── waiters: [w1, w2, ... wN]
```

startup 성공 후:

```text
STARTING
   ↓
RUNNING
```

으로 전환하면서 **모든 waiter에게 동일한 backend를 전달하고, 동시에 `active_requests`를 waiter 수만큼 증가시킨다.**

```text
waiters.len() == N
        ↓
active_requests += N
        ↓
N개의 RequestLease 발급
```

startup 실패 시 모든 waiter에게 error를 전달하고 `FAILED`로 전이한다.
waiter를 유실한 채 상태만 바꾸는 전이는 허용하지 않는다.

pending waiter 수에는 상한이 있다. → [§69 Backpressure와 Overload](06-gateway-and-requests.md)

---

## 17. Non-Blocking Startup

다음과 같이 actor 내부에서 직접 readiness를 기다리지 않는다.

```rust
runtime.start().await;
wait_until_ready().await;
```

startup은 background task에서 수행하고, 결과는 `operation_id`와 함께 event로 돌아온다.

```text
AppManager
   │
   ├── operation_id = N
   ├── state = STARTING { operation_id: N, waiters }
   │
   └── spawn lifecycle task
             │
             ├── Runtime.start()
             ├── readiness()
             │
             ▼
       StartCompleted { operation_id: N, .. }
             │
             ▼
        AppManager
             │
             └── N != current → ignore
```

shutdown도 동일한 방식으로 처리한다.

---

← [Lifecycle Invariants와 Identity](../02-architecture/02-lifecycle-invariants.md) · [문서 인덱스](../README.md) · [Runtime Abstraction과 Process Ownership](../02-architecture/04-runtime.md) →
