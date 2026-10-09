# Nimwake PRD

Nimwake 제품 요구사항 정의서. **이 트리가 원장(Single Source of Truth)이다.**

제품 정의부터 아키텍처 규범, 품질 기준, 장기 로드맵까지 **무엇을 왜 만드는가**를 정의한다.

실행 순서와 마일스톤 작업은 [페이즈 문서](../phases/README.md)가 담당한다.
두 문서가 충돌하면 PRD가 이긴다.

---

## 제품 정의 — `01-product/`

- [제품 개요와 문제 정의](01-product/01-overview.md) — `§1–§4`
- [제품 핵심 원칙](01-product/02-principles.md) — `§5`
- [Non-Goals와 Core MVP 범위](01-product/03-scope.md) — `§6–§8`

## 아키텍처 — `02-architecture/`

- [Core Architecture와 상태 모델](02-architecture/01-core-architecture.md) — `§9–§11`
- [Lifecycle Invariants와 Identity](02-architecture/02-lifecycle-invariants.md) — `§12`, `§68`
- [Concurrency Model과 Cold Start](02-architecture/03-concurrency.md) — `§13–§17`
- [Runtime Abstraction과 Process Ownership](02-architecture/04-runtime.md) — `§18–§20`, `§70`
- [Application Manifest와 Readiness](02-architecture/05-manifest-and-readiness.md) — `§21–§22`
- [Gateway Routing과 Request Lease](02-architecture/06-gateway-and-requests.md) — `§23–§26`, `§69`, `§75`
- [Scale-to-Zero와 Graceful Shutdown](02-architecture/07-scale-to-zero.md) — `§27–§30`
- [Failure Handling](02-architecture/08-failure-handling.md) — `§31–§33`, `§71`
- [Repository Architecture와 Technology Stack](02-architecture/09-repository-and-stack.md) — `§34–§36`

## 딜리버리 — `03-delivery/`

- [Core MVP Version Plan](03-delivery/01-version-plan.md) — `§37`
- [Testing Strategy](03-delivery/02-testing-strategy.md) — `§38–§41`
- [Load Tests](03-delivery/03-load-tests.md) — `§42`
- [Lifecycle Stress Test](03-delivery/04-stress-tests.md) — `§43`
- [Latency Instrumentation](03-delivery/05-instrumentation.md) — `§72`
- [CI Strategy](03-delivery/06-ci-strategy.md) — `§44`
- [Performance Goals와 Regression](03-delivery/07-performance-baseline.md) — `§45`, `§73`
- [Core MVP Definition of Done](03-delivery/08-definition-of-done.md) — `§65`

## 플랫폼 — `04-platform/`

- [Target Environments와 외부 의존성](04-platform/01-environments.md) — `§46–§48`
- [Android Deployment와 검증](04-platform/02-android.md) — `§49–§51`
- [Host Interface](04-platform/03-host-interface.md) — `§74`

## Post Core MVP — `05-beyond-mvp/`

- [Resource Management](05-beyond-mvp/01-resource-management.md) — `§52–§55`
- [CLI · Deployment · Storage · Observability · Security](05-beyond-mvp/02-platform-features.md) — `§56–§61`
- [Long-Term Roadmap](05-beyond-mvp/03-long-term-roadmap.md) — `§62–§63`

## 규칙과 포지셔닝 — `06-conventions/`

- [Development Rules](06-conventions/01-development-rules.md) — `§64`
- [제품 포지셔닝과 핵심 성공 조건](06-conventions/02-positioning.md) — `§66–§67`

---

## 섹션 번호 규칙

`§N`은 문서 간 참조를 위한 **안정 식별자**다.

- 섹션을 다른 파일로 옮기더라도 번호는 유지한다.
- 새 섹션은 기존 번호 사이에 끼워 넣지 않고 **다음 빈 번호를 사용한다.** (현재 최대: `§75`)
- 따라서 파일 안에서 번호가 연속하지 않을 수 있다. 읽는 순서는 파일 이름의 `NN` 접두사가 결정한다.

### 설계 리뷰로 추가된 섹션

| 섹션  | 주제                    | 위치                                                          |
| ----- | ----------------------- | ------------------------------------------------------------- |
| `§68` | Lifecycle Identity      | [Invariants와 Identity](02-architecture/02-lifecycle-invariants.md) |
| `§69` | Backpressure와 Overload | [Gateway와 Request Lease](02-architecture/06-gateway-and-requests.md) |
| `§70` | Process Ownership       | [Runtime과 Process Ownership](02-architecture/04-runtime.md)   |
| `§71` | Daemon–Child Lifetime   | [Failure Handling](02-architecture/08-failure-handling.md)     |
| `§72` | Latency Instrumentation | [Latency Instrumentation](03-delivery/05-instrumentation.md)   |
| `§73` | Performance Report와 Regression | [Performance Goals와 Regression](03-delivery/07-performance-baseline.md) |
| `§74` | Host Interface           | [Host Interface](04-platform/03-host-interface.md)             |
| `§75` | Response Header Timeout  | [Gateway와 Request Lease](02-architecture/06-gateway-and-requests.md) |

---

[문서 인덱스](../README.md) · [페이즈 로드맵](../phases/README.md)
