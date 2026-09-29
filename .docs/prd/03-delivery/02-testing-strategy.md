# Testing Strategy

> **Nimwake PRD** › 딜리버리
>
> 원문 구간: `PRD.md` §38–§41

---

## 38. Testing Strategy

Nimwake는 다음 네 계층으로 테스트한다.

```text
Unit
  ↓
Concurrency
  ↓
E2E
  ↓
Stress / Soak
```

---

## 39. Unit Tests

FakeRuntime을 사용한다.

필수 테스트:

| Scenario                           | Expected           |
| ---------------------------------- | ------------------ |
| Sleeping + acquire                 | Starting           |
| Starting + success                 | Running            |
| Starting + failure                 | Failed             |
| Running + acquire                  | process reused     |
| Running + crash                    | Failed             |
| Failed + acquire                   | Starting           |
| Running + idle + no active lease   | Stopping           |
| Running + idle + active lease      | Running            |
| stale idle timer                   | ignored            |
| Stopping + acquire                 | restart after stop |

stale event 처리도 단위 테스트 대상이다. → [§68 Lifecycle Identity](../02-architecture/02-lifecycle-invariants.md)

| Scenario                                   | Expected             |
| ------------------------------------------ | -------------------- |
| 이전 `operation_id`의 `StartCompleted`     | ignored              |
| 이전 `operation_id`의 `StopCompleted`      | ignored              |
| 교체된 process의 `ProcessExited`           | ignored              |
| 중복 `RequestFinished(lease_id)`           | counter 변화 없음    |
| startup 실패 시 대기 중이던 waiter         | 전원 error 수신      |
| Stopping → Starting 전이 시 waiter         | 유실 없이 이전       |

state machine correctness는 HTTP layer와 독립적으로 테스트할 수 있어야 한다.

---

## 40. Concurrency Tests

Nimwake에서 concurrency test는 first-class correctness test다.

핵심 테스트:

```text
1000 concurrent callers
          ↓
       acquire()
          ↓
      AppManager
          ↓
    FakeRuntime.start

expected:

start_count == 1
```

추가 검증:

```text
all waiters completed
duplicate process == 0
deadlock == 0
lost waiter == 0
active_requests == 발급된 lease 수
lease drop 후 active_requests == 0
```

bounded queue 동작도 검증한다.

```text
pending > max_pending_requests
          ↓
      503 응답
          ↓
이미 대기 중인 waiter는 영향 없음
```

---

## 41. E2E Tests

실제 `nimwaked`와 실제 child process를 사용한다.

Fixture:

```text
tests/fixtures/

├── echo-server/
├── slow-start-server/
└── crash-server/
```

핵심 E2E:

```text
start nimwaked

↓

assert backend process == 0

↓

100 concurrent requests

↓

HTTP 200

unique backend PID == 1

↓

idle timeout

↓

backend process == 0

↓

new request

↓

HTTP 200

new PID != old PID
```

이 시나리오를 Nimwake의 primary regression test로 삼는다.

추가 E2E 시나리오:

```text
daemon 종료 후 child process 잔존 == 0
port 충돌 manifest → daemon 기동 거부
stdout을 대량 출력하는 fixture → child가 멈추지 않음
```

---

← [Core MVP Version Plan](../03-delivery/01-version-plan.md) · [문서 인덱스](../README.md) · [Load Tests](../03-delivery/03-load-tests.md) →
