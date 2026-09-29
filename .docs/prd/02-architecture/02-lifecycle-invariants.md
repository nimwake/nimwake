# Lifecycle Invariants와 Identity

> **Nimwake PRD** › 아키텍처
>
> 원문 구간: `PRD.md` §12, §68

---

## 12. Lifecycle Invariants

Nimwake Core는 다음 invariant를 반드시 만족해야 한다.

### Invariant 1 — Single Process

하나의 application에 대해 동시에 존재하는 runtime process는 최대 하나다.

```text
application_process_count <= 1
```

---

### Invariant 2 — Single Lifecycle Authority

application lifecycle state를 변경할 수 있는 주체는 `AppManager` 하나뿐이다.

---

### Invariant 3 — Gateway Isolation

Gateway는 application process를 직접 시작하거나 종료하지 않는다.

Gateway는 다음 interface만 사용한다.

```rust
let lease = app.acquire().await?;
```

---

### Invariant 4 — Cold Start Deduplication

동시에 수백~수천 개의 요청이 발생해도 한 번의 startup만 수행해야 한다.

```text
1000 requests
     ↓
AppManager
     ↓
Runtime.start()
     ↓
exactly once
```

---

### Invariant 5 — Active Request Protection

다음 조건에서는 application을 idle shutdown하지 않는다.

```text
active_request_count > 0
```

---

### Invariant 6 — Non-Blocking Actor

다음과 같이 오래 걸릴 수 있는 작업은 AppManager의 event loop 밖에서 수행한다.

- process startup
- readiness polling
- graceful shutdown
- process exit waiting

이 작업들의 완료는 event로 AppManager에 보고한다.

---

### Invariant 7 — Acquire Atomicity

**backend 확보와 active request 예약은 하나의 state transition에서 수행한다.**

```text
acquire
   │
   ├── ensure RUNNING
   │
   └── active_requests += 1
             │
             ▼
        RequestLease
```

둘을 분리하면 다음 race가 발생한다.

```text
ensure_running()
      ↓
   RUNNING
             ← idle timer 발화
   STOPPING
      ↓
RequestStarted   ← 이미 종료 중인 process에 요청
```

`acquire()`가 반환한 `RequestLease`가 살아 있는 동안 해당 application은 idle shutdown 대상이 될 수 없다.

---

### Invariant 8 — Stale Event Rejection

비동기 lifecycle event는 도착 시점에 이미 무효일 수 있다.

identity가 현재 state와 일치하지 않는 event는 **무시하고 state를 변경하지 않는다.**

```text
event.identity != state.identity
        ↓
     ignore
```

---

### Invariant 9 — Bounded Queues

AppManager로 향하는 모든 대기열은 상한을 가진다.

```text
mailbox capacity   : bounded
pending waiters    : bounded
```

상한 초과 시 요청을 무한정 적재하지 않고 거절한다.
→ [§69 Backpressure와 Overload](06-gateway-and-requests.md)

---

### Invariant 10 — Daemon-Child Lifetime

Nimwake가 생성한 child process의 수명은 daemon의 수명을 넘지 않는다.

```text
nimwaked 종료
      ↓
managed child process 종료
```

→ [§71 Daemon–Child Lifetime](08-failure-handling.md)

---

## 68. Lifecycle Identity

Nimwake의 동시성 안전성은 네 가지 identity로 성립한다.

| Identity          | 방지하는 race                              | 소유 위치                         |
| ----------------- | ------------------------------------------ | --------------------------------- |
| `OperationId`     | stale start / stop 결과가 새 state를 덮어씀 | `Starting` / `Stopping` state     |
| `ProcessId`       | 이미 교체된 process의 exit event 오적용     | `Running` / `Stopping` state      |
| `IdleGeneration`  | stale idle timer가 활성 app을 종료          | `Running` state                   |
| `LeaseId`         | request/shutdown race, 중복 counter 감소    | `RequestLease`                    |

정의:

```rust
type OperationId = u64;
type ProcessId = u64;
type IdleGeneration = u64;
type LeaseId = u64;
```

---

### OperationId

startup / shutdown처럼 actor 밖에서 수행하는 모든 비동기 operation에 부여한다.

```text
start operation #10
      │
      ├── slow readiness
      │
      ▼
   timeout → FAILED

   retry acquire

start operation #11
      │
      ▼
   RUNNING

   ... 뒤늦게 StartCompleted(#10) 도착
```

처리:

```rust
if operation_id != current_operation_id {
    // stale operation
    return;
}
```

---

### ProcessId

readiness 성공과 process exit는 동시에 발생할 수 있다.

```text
spawn
 ↓
port open
 ↓
readiness success      ┐
                       ├── 순서 비결정적
process exits          ┘
```

따라서 process 관련 event는 반드시 process identity를 동반한다.

```rust
ProcessExited {
    process_id: ProcessId,
    status: ExitStatus,
}
```

처리:

```rust
if running.process_id != process_id {
    // stale process event
    return;
}
```

---

### IdleGeneration

```text
idle_generation = 41

idle timer(41) 예약

new acquire

idle_generation = 42

timer(41) 발화

41 != 42

→ ignore
```

→ [§29 Stale Timer Protection](07-scale-to-zero.md)

---

### LeaseId

`RequestLease` drop은 한 번만 counter를 감소시켜야 한다.

```rust
RequestFinished {
    lease_id: LeaseId,
}
```

중복 `RequestFinished`나 이미 교체된 process에 대한 lease 반납은 무시한다.

---

← [Core Architecture와 상태 모델](../02-architecture/01-core-architecture.md) · [문서 인덱스](../README.md) · [Concurrency Model과 Cold Start](../02-architecture/03-concurrency.md) →
