# Core MVP Version Plan

> **Nimwake PRD** › 딜리버리
>
> 원문 구간: `PRD.md` §37

---

## 37. Core MVP Version Plan

Core MVP는 다섯 개의 마일스톤을 거쳐 `v0.1.0`에 도달한다.

각 버전은 작업량이 아니라 **아키텍처 마일스톤**을 의미한다.

```text
v0.0.1   Lifecycle Foundation
   ↓
v0.0.2   Process Runtime
   ↓
v0.0.3   HTTP + Scale-to-Zero
   ↓
v0.0.4   E2E + Performance Baseline
   ↓
v0.0.5   Stress + Regression Qualification
   ↓
v0.1.0   Core MVP
```

| 버전     | 아키텍처 마일스톤                 | 핵심 acceptance                                        | 실행 계획                                                                                 |
| -------- | --------------------------------- | ------------------------------------------------------ | ----------------------------------------------------------------------------------------- |
| `v0.0.1` | Lifecycle Foundation              | `FakeRuntime.start_count == 1`                         | [Phase 1 › v0.0.1](../../phases/phase-1/03-milestones/01-v0.0.1-lifecycle-foundation.md)    |
| `v0.0.2` | Process Runtime                   | 실제 process spawn → ready → stop, 소유권 단일화       | [Phase 1 › v0.0.2](../../phases/phase-1/03-milestones/02-v0.0.2-process-runtime.md)         |
| `v0.0.3` | HTTP + Scale-to-Zero              | cold start → 200 → idle → 재기동, overload 시 `503`    | [Phase 1 › v0.0.3](../../phases/phase-1/03-milestones/03-v0.0.3-http-scale-to-zero.md)      |
| `v0.0.4` | E2E + Performance Baseline        | E2E 자동화 + 구간별 latency baseline 확보              | [Phase 1 › v0.0.4](../../phases/phase-1/03-milestones/04-v0.0.4-e2e-performance-baseline.md) |
| `v0.0.5` | Stress + Regression Qualification | storm / stress / leak 통과 + 이전 release 대비 비교    | [Phase 1 › v0.0.5](../../phases/phase-1/03-milestones/05-v0.0.5-stress-regression.md)       |
| `v0.1.0` | Core MVP                          | [Definition of Done](08-definition-of-done.md) 전 항목 | [Phase 1 › v0.1.0](../../phases/phase-1/03-milestones/06-v0.1.0-core-mvp.md)                 |

`v0.1.0`은 다음 의미를 가진다.

> Nimwake의 Scale-to-Zero Core lifecycle이 automated correctness, concurrency, E2E, stress test를 통과한 상태.

단순히 수동 테스트에서 동작하는 것을 의미하지 않는다.

---

### v0.0.4 / v0.0.5의 역할 분리

`v0.0.4`는 **측정 가능하게 만드는 단계**이고, `v0.0.5`는 **측정값을 비교해 판정하는 단계**다.

```text
v0.0.4 — E2E + Performance Baseline

Correctness
├── E2E
├── cold-start concurrency
├── crash recovery
└── lifecycle

Performance
├── direct-backend baseline
├── warm proxy benchmark
├── cold-start breakdown
├── concurrency sweep
├── CPU / RSS / FD
└── benchmark report


v0.0.5 — Stress + Regression Qualification

├── lifecycle stress
├── cold-start storm
├── sustained warm load
├── memory leak detection
├── FD leak detection
├── orphan detection
├── previous-release comparison
└── performance regression report
```

성능 수치는 초기에 release hard gate로 사용하지 않는다.
→ [§45 Performance Goals](07-performance-baseline.md)

---

### 이 문서의 범위

PRD는 각 마일스톤이 **무엇을 달성해야 하는지**만 정의한다.

마일스톤별 구현 항목, 작업 순서, 테스트 fixture 같은 실행 세부사항은
[Phase 1 실행 계획](../../phases/phase-1/README.md)에서 관리한다.

---

← [Repository Architecture와 Technology Stack](../02-architecture/09-repository-and-stack.md) · [문서 인덱스](../README.md) · [Testing Strategy](../03-delivery/02-testing-strategy.md) →
