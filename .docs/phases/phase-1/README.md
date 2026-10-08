# Phase 1 — Core MVP

> **목표 버전:** `v0.1.0`
> **주제:** Core Lifecycle · Scale-to-Zero
> **상태:** Core 구현 완료 · `v0.1.0` 태깅 대기
>
> 남은 것은 [완료 체크리스트](04-acceptance.md)의 `release CI passes`(태그 필요)와
> `nimwake-android` 검증 리포트다.

---

## 한 줄 정의

> 트래픽이 오면 애플리케이션을 깨우고, 프록시하고, idle이면 재운다.

이 하나의 lifecycle을 **작고, 결정적이며, 동시성에 안전하고, 신뢰할 수 있게** 만드는 것이 Phase 1의 전부다.

기능 개수는 Phase 1의 완료 기준이 아니다.

---

## 구현 전 확정된 설계

설계 리뷰를 거쳐 다음 네 가지 identity와 `acquire()` 계약을 확정했다.

```text
OperationId       stale start/stop 결과 차단
ProcessId         stale process-exit 차단
IdleGeneration    stale idle timer 차단
LeaseId           request/shutdown race 차단
```

그리고 모든 대기열은 bounded다.

전체 결정 이력과 PRD 반영 위치는 [05. 설계 결정 기록](05-design-decisions.md)에 있다.

---

## 문서

| 문서                                          | 내용                                       |
| --------------------------------------------- | ------------------------------------------ |
| [01. 목표와 성공 기준](01-goal.md)            | 이 페이즈가 증명해야 하는 것               |
| [02. PRD 참조 맵](02-prd-map.md)              | 구현 대상 PRD 섹션 매핑                    |
| [03. 마일스톤](#마일스톤)                     | `v0.0.1` ~ `v0.1.0` 버전별 구현 항목       |
| [04. 완료 체크리스트](04-acceptance.md)       | Definition of Done 확인표                  |
| [05. 설계 결정 기록](05-design-decisions.md)  | 리뷰 지적 → PRD 반영 추적표                |

---

## 마일스톤

| 버전     | 마일스톤                          | 성격            | 문서                                                        |
| -------- | --------------------------------- | --------------- | ------------------------------------------------------------ |
| `v0.0.1` | Lifecycle Foundation              | 상태 머신 확정  | [열기](03-milestones/01-v0.0.1-lifecycle-foundation.md)      |
| `v0.0.2` | Process Runtime                   | 실제 process    | [열기](03-milestones/02-v0.0.2-process-runtime.md)           |
| `v0.0.3` | HTTP + Scale-to-Zero              | 가설 증명       | [열기](03-milestones/03-v0.0.3-http-scale-to-zero.md)        |
| `v0.0.4` | E2E + Performance Baseline        | 측정 가능화     | [열기](03-milestones/04-v0.0.4-e2e-performance-baseline.md)  |
| `v0.0.5` | Stress + Regression Qualification | 비교·판정       | [열기](03-milestones/05-v0.0.5-stress-regression.md)         |
| `v0.1.0` | Core MVP                          | 릴리스          | [열기](03-milestones/06-v0.1.0-core-mvp.md)                  |

---

## 이 페이즈에서 하지 않는 것

Phase 1 제외 항목은 PRD가 소유한다.

- [PRD §6 Non-Goals](../../prd/01-product/03-scope.md)
- [PRD §8 Core MVP에서 제외하는 기능](../../prd/01-product/03-scope.md)

명시적으로 미룬 항목: crash-loop backoff, dynamic port allocation, process adoption,
request body 크기 상한, 상시 metrics exporter → [05. 설계 결정 기록 P2](05-design-decisions.md)

Android 검증은 `v0.1.0` 이후가 아니라 `v0.0.3` 이후 [`nimwake-android` Phase 1](https://github.com/nimwake/nimwake-android/tree/main/.docs/phases/phase-1)에서 **병렬로** 수행한다.
결과는 [완료 체크리스트 — Android Device](04-acceptance.md#android-device)로 들어온다. → [PRD §50](../../prd/04-platform/02-android.md), [D42](05-design-decisions.md)

---

[페이즈 로드맵](../README.md) · [문서 인덱스](../../README.md) · [PRD 인덱스](../../prd/README.md)
