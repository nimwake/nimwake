# PRD 참조 맵

> **Phase 1 — Core MVP** › 범위
>
> 이 페이즈가 구현하는 PRD 섹션과 담당 마일스톤의 대응표.

---

Phase 1은 아래 PRD 섹션을 구현 대상으로 삼는다.

요구사항 본문은 모두 PRD에 있으며, 이 표는 **어느 마일스톤에서 구현하는가**만 정의한다.

---

## 아키텍처

| PRD 섹션                    | 문서                                                                           | 마일스톤 |
| --------------------------- | ------------------------------------------------------------------------------ | -------- |
| §9 Core Architecture        | [Core Architecture와 상태 모델](../../prd/02-architecture/01-core-architecture.md) | `v0.0.1` |
| §10 Application Lifecycle   | [Core Architecture와 상태 모델](../../prd/02-architecture/01-core-architecture.md) | `v0.0.1` |
| §11 상태 모델               | [Core Architecture와 상태 모델](../../prd/02-architecture/01-core-architecture.md) | `v0.0.1` |
| §12 Lifecycle Invariants    | [Invariants와 Identity](../../prd/02-architecture/02-lifecycle-invariants.md)   | 전 구간  |
| §68 Lifecycle Identity      | [Invariants와 Identity](../../prd/02-architecture/02-lifecycle-invariants.md)   | `v0.0.1` |
| §13 Concurrency Model       | [Concurrency Model과 Cold Start](../../prd/02-architecture/03-concurrency.md)    | `v0.0.1` |
| §14 AppManager              | [Concurrency Model과 Cold Start](../../prd/02-architecture/03-concurrency.md)    | `v0.0.1` |
| §15 Command / Event Model   | [Concurrency Model과 Cold Start](../../prd/02-architecture/03-concurrency.md)    | `v0.0.1` |
| §16 Concurrent Cold Start   | [Concurrency Model과 Cold Start](../../prd/02-architecture/03-concurrency.md)    | `v0.0.1` |
| §17 Non-Blocking Startup    | [Concurrency Model과 Cold Start](../../prd/02-architecture/03-concurrency.md)    | `v0.0.1` |
| §18 Runtime Abstraction     | [Runtime과 Process Ownership](../../prd/02-architecture/04-runtime.md)          | `v0.0.1` |
| §19 Process Runtime         | [Runtime과 Process Ownership](../../prd/02-architecture/04-runtime.md)          | `v0.0.2` |
| §20 Process Isolation       | [Runtime과 Process Ownership](../../prd/02-architecture/04-runtime.md)          | `v0.0.2` |
| §70 Process Ownership       | [Runtime과 Process Ownership](../../prd/02-architecture/04-runtime.md)          | `v0.0.2` |
| §21 Application Manifest    | [Manifest와 Readiness](../../prd/02-architecture/05-manifest-and-readiness.md)  | `v0.0.1` |
| §22 Readiness               | [Manifest와 Readiness](../../prd/02-architecture/05-manifest-and-readiness.md)  | `v0.0.2` |
| §23 Gateway Routing         | [Gateway와 Request Lease](../../prd/02-architecture/06-gateway-and-requests.md) | `v0.0.3` |
| §24 HTTP Contract           | [Gateway와 Request Lease](../../prd/02-architecture/06-gateway-and-requests.md) | `v0.0.3` |
| §25 Request Tracking        | [Gateway와 Request Lease](../../prd/02-architecture/06-gateway-and-requests.md) | `v0.0.1` |
| §26 Acquire와 RequestLease  | [Gateway와 Request Lease](../../prd/02-architecture/06-gateway-and-requests.md) | `v0.0.1` |
| §69 Backpressure와 Overload | [Gateway와 Request Lease](../../prd/02-architecture/06-gateway-and-requests.md) | `v0.0.3` |
| §27 Scale-to-Zero           | [Scale-to-Zero와 Graceful Shutdown](../../prd/02-architecture/07-scale-to-zero.md) | `v0.0.3` |
| §28 Idle Timer              | [Scale-to-Zero와 Graceful Shutdown](../../prd/02-architecture/07-scale-to-zero.md) | `v0.0.3` |
| §29 Stale Timer Protection  | [Scale-to-Zero와 Graceful Shutdown](../../prd/02-architecture/07-scale-to-zero.md) | `v0.0.3` |
| §30 Graceful Shutdown       | [Scale-to-Zero와 Graceful Shutdown](../../prd/02-architecture/07-scale-to-zero.md) | `v0.0.2` |
| §31 Request During STOPPING | [Failure Handling](../../prd/02-architecture/08-failure-handling.md)             | `v0.0.3` |
| §32 Process Crash           | [Failure Handling](../../prd/02-architecture/08-failure-handling.md)             | `v0.0.2` |
| §33 Failure Recovery        | [Failure Handling](../../prd/02-architecture/08-failure-handling.md)             | `v0.0.2` |
| §71 Daemon–Child Lifetime   | [Failure Handling](../../prd/02-architecture/08-failure-handling.md)             | `v0.0.2` |
| §34 Repository Architecture | [Repository와 Technology Stack](../../prd/02-architecture/09-repository-and-stack.md) | `v0.0.1` |
| §35 Technology Stack        | [Repository와 Technology Stack](../../prd/02-architecture/09-repository-and-stack.md) | `v0.0.1` |
| §36 Nimwake Daemon          | [Repository와 Technology Stack](../../prd/02-architecture/09-repository-and-stack.md) | `v0.0.3` |

---

## 품질

| PRD 섹션                      | 문서                                                                        | 마일스톤 |
| ----------------------------- | --------------------------------------------------------------------------- | -------- |
| §38 Testing Strategy          | [Testing Strategy](../../prd/03-delivery/02-testing-strategy.md)             | `v0.0.1` |
| §39 Unit Tests                | [Testing Strategy](../../prd/03-delivery/02-testing-strategy.md)             | `v0.0.1` |
| §40 Concurrency Tests         | [Testing Strategy](../../prd/03-delivery/02-testing-strategy.md)             | `v0.0.1` |
| §41 E2E Tests                 | [Testing Strategy](../../prd/03-delivery/02-testing-strategy.md)             | `v0.0.4` |
| §42 Load Tests                | [Load Tests](../../prd/03-delivery/03-load-tests.md)                         | `v0.0.4` |
| §43 Lifecycle Stress Test     | [Lifecycle Stress Test](../../prd/03-delivery/04-stress-tests.md)            | `v0.0.5` |
| §72 Latency Instrumentation   | [Latency Instrumentation](../../prd/03-delivery/05-instrumentation.md)       | `v0.0.4` |
| §44 CI Strategy               | [CI Strategy](../../prd/03-delivery/06-ci-strategy.md)                       | `v0.0.4` |
| §45 Performance Goals         | [Performance Goals와 Regression](../../prd/03-delivery/07-performance-baseline.md) | `v0.0.4` |
| §73 Performance Report와 Regression | [Performance Goals와 Regression](../../prd/03-delivery/07-performance-baseline.md) | `v0.0.5` |
| §64 Development Rules         | [Development Rules](../../prd/06-conventions/01-development-rules.md)        | 전 구간  |
| §65 Definition of Done        | [Core MVP Definition of Done](../../prd/03-delivery/08-definition-of-done.md) | `v0.1.0` |

---

## Phase 1 범위 밖

| PRD 섹션                 | 담당 페이즈                        |
| ------------------------ | ---------------------------------- |
| §49–§51 Android          | `v0.1.0` 이후 검증 / [Phase 4](../phase-4/README.md) |
| §52–§55 Resource Management | [Phase 3](../phase-3/README.md)  |
| §56–§59 CLI · 배포 · 저장소 | [Phase 2](../phase-2/README.md)  |
| §60–§61 Observability · Security | [Phase 4](../phase-4/README.md) |
| §62–§63 Long-Term Roadmap | [페이즈 로드맵](../README.md)      |

---

← [목표와 성공 기준](01-goal.md) · [페이즈 개요](README.md) · [v0.0.1 Lifecycle Foundation](03-milestones/01-v0.0.1-lifecycle-foundation.md) →
