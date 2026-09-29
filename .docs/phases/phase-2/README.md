# Phase 2 — 운영 인터페이스

> **목표 버전:** `v0.2.x`
> **주제:** CLI · 상태 저장 · 배포 · 롤백
> **상태:** 미착수 — [Phase 1](../phase-1/README.md)의 Definition of Done 통과 후 착수

---

## 한 줄 정의

> 배포된 애플리케이션을 사람이 운영할 수 있게 만든다.

Phase 1이 lifecycle의 **정확성**을 증명했다면, Phase 2는 그 lifecycle을 **조작하고 관측할 수 있는 표면**을 만든다.

---

## 구현 대상 PRD 섹션

| PRD 섹션              | 문서                                                                      |
| --------------------- | ------------------------------------------------------------------------- |
| §56 CLI               | [CLI · Deployment · Storage · Observability · Security](../../prd/05-beyond-mvp/02-platform-features.md) |
| §57 Deployment        | [동일](../../prd/05-beyond-mvp/02-platform-features.md)                    |
| §58 Filesystem        | [동일](../../prd/05-beyond-mvp/02-platform-features.md)                    |
| §59 Database Strategy | [동일](../../prd/05-beyond-mvp/02-platform-features.md)                    |

관련 후보 항목: [PRD §63 Post-MVP 후보](../../prd/05-beyond-mvp/03-long-term-roadmap.md)

---

## 예상 범위

- `nimwake` CLI와 daemon 간 Unix Domain Socket IPC
- SQLite 기반 상태 저장
- artifact 배포와 rollback
- release 관리

세부 요구사항은 PRD에 있으며, 착수 시점에 이 문서를 Phase 1과 같은 구조로 확장한다.

```text
phases/phase-2/
├── README.md
├── 01-goal.md
├── 02-prd-map.md
├── 03-milestones/
├── 04-acceptance.md
└── 05-design-decisions.md
```

---

## 착수 조건

- [ ] [Phase 1 완료 체크리스트](../phase-1/04-acceptance.md) 전 항목 충족
- [ ] `v0.1.0` 태깅 완료
- [ ] Phase 1에서 드러난 요구사항 변경이 PRD에 반영됨

---

[페이즈 로드맵](../README.md) · [문서 인덱스](../../README.md) · [PRD 인덱스](../../prd/README.md)
