# Nimwake 문서

```text
.docs/
├── prd/       ← 원장 (Single Source of Truth)
└── phases/    ← 실행 계획 (Phase 1, 2, 3 …)
```

| 트리                              | 성격                                                                  |
| --------------------------------- | --------------------------------------------------------------------- |
| [**PRD**](prd/README.md)          | **원장.** 무엇을 왜 만드는가. 제품 정의 · 아키텍처 규범 · 품질 기준 · 로드맵 |
| [**Phases**](phases/README.md)    | 실행 계획. 언제 어떤 순서로 만드는가. 페이즈별 마일스톤과 완료 조건    |

---

## 단일 출처 규칙

1. **요구사항은 PRD에만 존재한다.** 상태 머신, invariant, 테스트 기준, CI 정책, 개발 규칙은 PRD가 소유한다.
2. **페이즈 문서는 요구사항을 복사하지 않고 링크한다.** 페이즈 문서가 소유하는 것은 목표 · PRD 참조 맵 · 마일스톤 작업 · 완료 체크리스트뿐이다.
3. **충돌하면 PRD가 이긴다.** 실행 중 요구사항이 바뀌면 PRD를 먼저 고치고, 페이즈 문서의 링크와 체크리스트를 맞춘다.
4. 한 파일은 하나의 주제를 다루며 300라인을 넘기지 않는다.
5. 폴더와 파일 이름은 `NN-kebab-case` 숫자 접두사로 읽는 순서를 고정한다.

---

## 페이즈 현황

| 페이즈                                 | 버전     | 주제                                         | 상태      |
| -------------------------------------- | -------- | -------------------------------------------- | --------- |
| [Phase 1](phases/phase-1/README.md)    | `v0.1.0` | Core Lifecycle · Scale-to-Zero               | 진행 예정 |
| [Phase 2](phases/phase-2/README.md)    | `v0.2.x` | CLI · 상태 저장 · 배포 · 롤백                | 미착수    |
| [Phase 3](phases/phase-3/README.md)    | `v0.3.x` | 리소스 모니터링 · memory pressure · eviction | 미착수    |
| [Phase 4](phases/phase-4/README.md)    | Later    | Android host tooling · 스트리밍 · 컨테이너   | 후보      |

---

## PRD 바로가기

**제품 정의**

- [제품 개요와 문제 정의](prd/01-product/01-overview.md)
- [제품 핵심 원칙](prd/01-product/02-principles.md)
- [Non-Goals와 Core MVP 범위](prd/01-product/03-scope.md)

**아키텍처**

- [Core Architecture와 상태 모델](prd/02-architecture/01-core-architecture.md)
- [Lifecycle Invariants와 Identity](prd/02-architecture/02-lifecycle-invariants.md)
- [Concurrency Model과 Cold Start](prd/02-architecture/03-concurrency.md)
- [Runtime Abstraction과 Process Ownership](prd/02-architecture/04-runtime.md)
- [Application Manifest와 Readiness](prd/02-architecture/05-manifest-and-readiness.md)
- [Gateway Routing과 Request Lease](prd/02-architecture/06-gateway-and-requests.md)
- [Scale-to-Zero와 Graceful Shutdown](prd/02-architecture/07-scale-to-zero.md)
- [Failure Handling](prd/02-architecture/08-failure-handling.md)
- [Repository Architecture와 Technology Stack](prd/02-architecture/09-repository-and-stack.md)

**딜리버리**

- [Core MVP Version Plan](prd/03-delivery/01-version-plan.md)
- [Testing Strategy](prd/03-delivery/02-testing-strategy.md)
- [Load Tests](prd/03-delivery/03-load-tests.md)
- [Lifecycle Stress Test](prd/03-delivery/04-stress-tests.md)
- [Latency Instrumentation](prd/03-delivery/05-instrumentation.md)
- [CI Strategy](prd/03-delivery/06-ci-strategy.md)
- [Performance Goals와 Regression](prd/03-delivery/07-performance-baseline.md)
- [Core MVP Definition of Done](prd/03-delivery/08-definition-of-done.md)

**플랫폼 · 이후**

- [Target Environments와 외부 의존성](prd/04-platform/01-environments.md)
- [Android Deployment와 검증](prd/04-platform/02-android.md)
- [Resource Management](prd/05-beyond-mvp/01-resource-management.md)
- [CLI · Deployment · Storage · Observability · Security](prd/05-beyond-mvp/02-platform-features.md)
- [Long-Term Roadmap](prd/05-beyond-mvp/03-long-term-roadmap.md)

**규칙**

- [Development Rules](prd/06-conventions/01-development-rules.md)
- [제품 포지셔닝과 핵심 성공 조건](prd/06-conventions/02-positioning.md)

---

## Phase 1 바로가기

- [페이즈 개요](phases/phase-1/README.md)
- [목표와 성공 기준](phases/phase-1/01-goal.md)
- [PRD 참조 맵](phases/phase-1/02-prd-map.md)
- 마일스톤:
  [v0.0.1](phases/phase-1/03-milestones/01-v0.0.1-lifecycle-foundation.md) ·
  [v0.0.2](phases/phase-1/03-milestones/02-v0.0.2-process-runtime.md) ·
  [v0.0.3](phases/phase-1/03-milestones/03-v0.0.3-http-scale-to-zero.md) ·
  [v0.0.4](phases/phase-1/03-milestones/04-v0.0.4-e2e-performance-baseline.md) ·
  [v0.0.5](phases/phase-1/03-milestones/05-v0.0.5-stress-regression.md) ·
  [v0.1.0](phases/phase-1/03-milestones/06-v0.1.0-core-mvp.md)
- [완료 체크리스트](phases/phase-1/04-acceptance.md)
- [설계 결정 기록](phases/phase-1/05-design-decisions.md)
