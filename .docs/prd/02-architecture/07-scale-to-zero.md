# Scale-to-Zero와 Graceful Shutdown

> **Nimwake PRD** › 아키텍처
>
> 원문 구간: `PRD.md` §27–§30

---

## 27. Scale-to-Zero

application shutdown 조건:

```text
active_request_count == 0

AND

now - last_request_at >= idle_timeout
```

조건이 만족되면:

```text
RUNNING
   ↓
STOPPING
```

으로 전환한다.

---

## 28. Idle Timer

모든 application을 주기적으로 scan하는 global polling보다는 application별 timer를 기본 방식으로 사용한다.

```text
RequestFinished { lease_id }
      │
      ▼
active_requests = 0
      │
      ▼
idle_generation += 1
      │
      ▼
schedule idle timer(idle_generation)
      │
      ▼
IdleTimeout { generation }
```

---

## 29. Stale Timer Protection

새로운 request가 발생했는데 이전 idle timer가 나중에 실행될 수 있다.

이를 방지하기 위해 `IdleGeneration`을 사용한다. → [§68 Lifecycle Identity](02-lifecycle-invariants.md)

```text
idle_generation = 41

idle timer(41)

new acquire

idle_generation = 42

timer(41) fires

41 != 42

→ ignore
```

`IdleGeneration`은 `Running` state가 소유하므로, `SLEEPING`이나 `FAILED`를 거쳐 다시 올라온
application의 timer는 자동으로 무효가 된다.

---

## 30. Graceful Shutdown

shutdown sequence:

```text
RUNNING
   ↓
STOPPING
   ↓
SIGTERM
   ↓
shutdown timeout
   │
   ├── exited
   │      ↓
   │   SLEEPING
   │
   └── alive
          ↓
       SIGKILL
          ↓
       SLEEPING
```

shutdown은 `operation_id`를 가진 비동기 operation이다.
`StopCompleted`가 도착했을 때 `operation_id`가 현재 state와 다르면 무시한다.
→ [Invariant 8](02-lifecycle-invariants.md)

SIGTERM / SIGKILL은 개별 process가 아니라 **process group 단위로 전달**한다.
→ [§71 Daemon–Child Lifetime](08-failure-handling.md)

실제 process tree handling은 OS별 특성을 고려해 이후 보강할 수 있다.

---

← [Gateway Routing과 Request Tracking](../02-architecture/06-gateway-and-requests.md) · [문서 인덱스](../README.md) · [Failure Handling](../02-architecture/08-failure-handling.md) →
