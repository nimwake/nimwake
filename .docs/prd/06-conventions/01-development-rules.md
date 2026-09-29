# Development Rules

> **Nimwake PRD** › 규칙과 포지셔닝
>
> 원문 구간: `PRD.md` §64

---

## 64. Development Rules

### Rule 1

Lifecycle state는 `AppManager`만 변경한다.

### Rule 2

Gateway는 process를 직접 제어하지 않는다.

### Rule 3

긴 startup/shutdown 작업으로 actor event loop를 block하지 않는다.

### Rule 4

발견된 race condition에는 반드시 regression test를 추가한다.

### Rule 5

lifecycle/concurrency 변경 PR에는 concurrency test가 포함되어야 한다.

### Rule 6

Core는 provider independent 상태를 유지한다.

### Rule 7

Android-specific behavior를 Core에 추가하지 않는다.

### Rule 8

실제 필요성이 확인되지 않은 infrastructure는 미리 추가하지 않는다.

### Rule 9

actor 밖에서 발생하는 모든 lifecycle event에는 identity를 붙인다.

`OperationId` / `ProcessId` / `IdleGeneration` / `LeaseId` 없이 state를 변경하는 event를 추가하지 않는다.
→ [§68 Lifecycle Identity](../02-architecture/02-lifecycle-invariants.md)

### Rule 10

AppManager로 향하는 대기열에는 반드시 상한을 둔다.

unbounded channel이나 unbounded waiter 목록을 도입하지 않는다.
→ [§69 Backpressure와 Overload](../02-architecture/06-gateway-and-requests.md)

### Rule 11

성능 문제는 추측이 아니라 구간별 계측으로 판단한다.

총 latency만 보고 최적화 대상을 정하지 않는다.
→ [§72 Latency Instrumentation](../03-delivery/05-instrumentation.md)

### Rule 12

Correctness를 performance보다 우선한다.

특히:

```text
duplicate process = 0
lost request = 0
deadlock = 0
orphan process = 0
```

은 Core의 주요 요구사항이다.

---

← [Long-Term Roadmap](../05-beyond-mvp/03-long-term-roadmap.md) · [문서 인덱스](../README.md) · [제품 포지셔닝과 핵심 성공 조건](../06-conventions/02-positioning.md) →
