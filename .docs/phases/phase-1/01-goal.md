# 목표와 성공 기준

> **Phase 1 — Core MVP** › 목표
>
> 원장: [PRD §7 Core MVP 범위](../../prd/01-product/03-scope.md) · [PRD §67 핵심 성공 조건](../../prd/06-conventions/02-positioning.md)

---

## 1. 증명해야 하는 lifecycle

Phase 1은 다음 한 사이클이 반복적으로 안정 동작함을 증명한다.

```text
Application Process
        │
        ▼
     SLEEPING
        │
     HTTP request
        │
        ▼
     STARTING
        │
   process spawn
        │
   readiness check
        │
        ▼
      RUNNING
        │
    HTTP proxy
        │
    idle timeout
        │
        ▼
     STOPPING
        │
   graceful stop
        │
        ▼
     SLEEPING
```

상태 정의와 전이 규칙은 PRD가 소유한다.
→ [PRD §10 Application Lifecycle](../../prd/02-architecture/01-core-architecture.md)

---

## 2. 반드시 보장해야 하는 것

| 보장 항목                                        | 근거 invariant                                                        |
| ------------------------------------------------ | --------------------------------------------------------------------- |
| 동시 cold start에서도 process는 정확히 하나       | [Invariant 1 · 4](../../prd/02-architecture/02-lifecycle-invariants.md) |
| STARTING 중 도착한 요청은 같은 startup을 기다린다 | [Invariant 4](../../prd/02-architecture/02-lifecycle-invariants.md)     |
| backend 확보와 request 예약이 원자적이다          | [Invariant 7](../../prd/02-architecture/02-lifecycle-invariants.md)     |
| active lease가 있으면 idle shutdown하지 않는다    | [Invariant 5](../../prd/02-architecture/02-lifecycle-invariants.md)     |
| 지나간 비동기 event가 새 state를 덮지 않는다      | [Invariant 8](../../prd/02-architecture/02-lifecycle-invariants.md)     |
| 대기열이 무한히 늘어나지 않는다                   | [Invariant 9](../../prd/02-architecture/02-lifecycle-invariants.md)     |
| daemon이 죽으면 child도 함께 정리된다             | [Invariant 10](../../prd/02-architecture/02-lifecycle-invariants.md)    |
| process crash를 감지한다                          | [PRD §32](../../prd/02-architecture/08-failure-handling.md)             |
| FAILED 앱을 다시 시작할 수 있다                   | [PRD §33](../../prd/02-architecture/08-failure-handling.md)             |
| 반복 lifecycle 후 orphan/zombie가 남지 않는다     | [PRD §43](../../prd/03-delivery/04-stress-tests.md)                     |
| Core 동작이 언어/프레임워크에 독립적이다          | [PRD §5.2](../../prd/01-product/02-principles.md)                       |

---

## 3. 네 개의 Identity

Phase 1의 동시성 안전성은 다음 네 가지로 성립한다.
구현 중 새 비동기 event를 추가할 때마다 어느 identity에 속하는지 먼저 정해야 한다.

```text
OperationId       stale start/stop 결과 차단
ProcessId         stale process-exit 차단
IdleGeneration    stale idle timer 차단
LeaseId           request/shutdown race 차단
```

→ [PRD §68 Lifecycle Identity](../../prd/02-architecture/02-lifecycle-invariants.md)

---

## 4. 측정 가능해야 한다

Phase 1은 "동작한다"에서 멈추지 않는다.

`v0.0.4`부터 cold start를 구간별로 분해해 기록한다.

```text
actor queue       3ms
spawn             8ms
app readiness   821ms  ← bottleneck
proxy connect      2ms
other             66ms
```

총합만 기록하면 Nimwake를 고쳐야 할지 application을 고쳐야 할지 구분할 수 없다.

→ [PRD §72 Latency Instrumentation](../../prd/03-delivery/05-instrumentation.md)

---

## 5. 완료 판단

Phase 1의 완료는 [PRD §65 Core MVP Definition of Done](../../prd/03-delivery/08-definition-of-done.md)으로 판단한다.

진행 중 확인표는 [04. 완료 체크리스트](04-acceptance.md)에서 관리한다.

구현 착수 전에 확정해야 할 설계 결정은 [05. 설계 결정 기록](05-design-decisions.md)에 있다.

---

[페이즈 개요](README.md) · [PRD 참조 맵](02-prd-map.md) →
