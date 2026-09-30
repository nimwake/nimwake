# Phase 4 — 플랫폼 확장

> **목표 버전:** 미정 (Later)
> **주제:** Android host daemon · 스트리밍 · 컨테이너 런타임 · 관측성
> **상태:** 후보 — 범위 미확정

---

## 한 줄 정의

> Core가 안정된 뒤에 붙이는 것들.

Phase 4의 항목은 아직 커밋된 계획이 아니라 **후보 목록**이다.
실제 필요성이 확인되기 전에는 착수하지 않는다. → [PRD §64 Rule 8](../../prd/06-conventions/01-development-rules.md)

---

## 후보 영역

| 영역                       | PRD 근거                                                                  |
| -------------------------- | ------------------------------------------------------------------------- |
| Logging & Observability    | [§60](../../prd/05-beyond-mvp/02-platform-features.md)                     |
| Security                   | [§61](../../prd/05-beyond-mvp/02-platform-features.md)                     |
| Android host daemon (`nimwake-hostd`) | [§49–§51](../../prd/04-platform/02-android.md). shell 기반 host tooling MVP는 [`nimwake-android` Phase 1](https://github.com/nimwake/nimwake-android/tree/main/.docs/phases/phase-1)로 옮겼다 ([D42](../phase-1/05-design-decisions.md)) |
| WebSocket · SSE · 스트리밍 | [§24](../../prd/02-architecture/06-gateway-and-requests.md), [§63](../../prd/05-beyond-mvp/03-long-term-roadmap.md) |
| Container / MicroVM Runtime | [§18](../../prd/02-architecture/04-runtime.md), [§63](../../prd/05-beyond-mvp/03-long-term-roadmap.md) |
| Dashboard · 통합           | [§63](../../prd/05-beyond-mvp/03-long-term-roadmap.md)                     |

전체 후보 목록은 [PRD §63 Post-MVP 후보](../../prd/05-beyond-mvp/03-long-term-roadmap.md)가 소유한다.

---

## 제약

이 페이즈의 어떤 항목도 다음을 위반할 수 없다.

- Core는 provider independent를 유지한다 ([Rule 6](../../prd/06-conventions/01-development-rules.md))
- Android 전용 동작은 Core에 들어가지 않는다 ([Rule 7](../../prd/06-conventions/01-development-rules.md))
- Runtime adapter가 추가되어도 lifecycle semantics는 바뀌지 않는다 ([PRD §18](../../prd/02-architecture/04-runtime.md))

---

## 착수 조건

- [ ] Phase 3 완료 또는 해당 항목의 필요성이 운영 데이터로 확인됨
- [ ] 착수 항목이 PRD의 정식 요구사항 섹션으로 승격됨

---

[페이즈 로드맵](../README.md) · [문서 인덱스](../../README.md) · [PRD 인덱스](../../prd/README.md)
