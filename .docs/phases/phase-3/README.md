# Phase 3 — 리소스 인식 스케줄링

> **목표 버전:** `v0.3.x`
> **주제:** 리소스 모니터링 · memory pressure · eviction
> **상태:** 미착수 — [Phase 2](../phase-2/README.md) 이후

---

## 한 줄 정의

> 남은 메모리를 보고 어떤 애플리케이션을 재울지 판단한다.

Scale-to-Zero가 **시간 기준**(idle timeout)이라면, Phase 3은 **자원 기준**(memory pressure)의 종료 판단을 추가한다.

---

## 구현 대상 PRD 섹션

| PRD 섹션                        | 문서                                                              |
| ------------------------------- | ----------------------------------------------------------------- |
| §52 Resource Management         | [Resource Management](../../prd/05-beyond-mvp/01-resource-management.md) |
| §53 Memory Pressure             | [동일](../../prd/05-beyond-mvp/01-resource-management.md)          |
| §54 Eviction Policy             | [동일](../../prd/05-beyond-mvp/01-resource-management.md)          |
| §55 Application Resource Limits | [동일](../../prd/05-beyond-mvp/01-resource-management.md)          |

---

## 예상 범위

- 시스템 / 프로세스 리소스 수집 (`sysinfo` + `/proc` fallback)
- `minimum_free_memory_mb` 기반 startup 차단과 eviction 트리거
- eviction 후보 우선순위 정책
- application별 resource limit

이 페이즈는 [Invariant 5 — Active Request Protection](../../prd/02-architecture/02-lifecycle-invariants.md)을 깨지 않아야 한다.
active request가 있는 애플리케이션은 eviction 대상이 될 수 없다.

---

## 착수 조건

- [ ] Phase 2 완료
- [ ] 장시간 운영 데이터에서 memory pressure가 실제 문제로 확인됨 ([PRD §64 Rule 8](../../prd/06-conventions/01-development-rules.md))

---

[페이즈 로드맵](../README.md) · [문서 인덱스](../../README.md) · [PRD 인덱스](../../prd/README.md)
