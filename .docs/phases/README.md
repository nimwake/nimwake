# 페이즈 로드맵

Nimwake는 [PRD](../prd/README.md)에 정의된 요구사항을 페이즈 단위로 실행한다.

PRD가 **무엇을 왜 만드는가**를 정의하고, 각 페이즈 문서는 **언제 어떤 순서로 만드는가**를 정의한다.

---

## 페이즈 목록

| 페이즈                          | 버전     | 주제                                         | 상태        | PRD 근거                                                                      |
| ------------------------------- | -------- | -------------------------------------------- | ----------- | ----------------------------------------------------------------------------- |
| [Phase 1](phase-1/README.md)    | `v0.1.0` | Core Lifecycle · Scale-to-Zero               | 구현 완료   | [§7](../prd/01-product/03-scope.md), [§9–§36](../prd/02-architecture/01-core-architecture.md) |
| [Phase 2](phase-2/README.md)    | `v0.2.x` | CLI · 상태 저장 · 배포 · 롤백                | 미착수      | [§56–§59](../prd/05-beyond-mvp/02-platform-features.md)                       |
| [Phase 3](phase-3/README.md)    | `v0.3.x` | 리소스 모니터링 · memory pressure · eviction | 미착수      | [§52–§55](../prd/05-beyond-mvp/01-resource-management.md)                      |
| [Phase 4](phase-4/README.md)    | Later    | Android host tooling · 스트리밍 · 컨테이너   | 후보        | [§60–§63](../prd/05-beyond-mvp/03-long-term-roadmap.md)                        |

각 페이즈는 이전 페이즈의 Definition of Done을 통과한 뒤에 시작한다.

---

## 페이즈 문서 작성 규칙

1. **요구사항을 다시 쓰지 않는다.** 상태 머신, invariant, 품질 기준, 개발 규칙 같은 규범은 PRD에만 존재한다. 페이즈 문서는 해당 PRD 섹션을 링크한다.
2. **페이즈 문서가 소유하는 것**은 다음 네 가지다.
   - 이 페이즈의 목표와 완료 판단 기준
   - 구현 대상 PRD 섹션 매핑(PRD 참조 맵)
   - 마일스톤별 구현 항목과 작업 순서
   - 실행 중에만 의미 있는 결정 기록(fixture, 도구 선택, 측정값)
3. **충돌 시 PRD가 이긴다.** 페이즈 진행 중 요구사항이 바뀌면 PRD를 먼저 고치고 페이즈 문서의 링크를 갱신한다.
4. 한 파일은 300라인을 넘기지 않는다.

---

## 페이즈 문서 구조

```text
phases/phase-N/
├── README.md               # 페이즈 개요와 인덱스
├── 01-goal.md              # 목표와 성공 기준
├── 02-prd-map.md           # 구현 대상 PRD 섹션 매핑
├── 03-milestones/          # 버전별 구현 항목
├── 04-acceptance.md        # 완료 체크리스트
└── 05-design-decisions.md  # 설계 결정 이력과 PRD 반영 추적
```

`05-design-decisions.md`는 결정의 **내용**이 아니라 **이력**을 담는다.
결정 내용은 PRD로 들어가고, 여기에는 "무엇을 왜 바꿨고 어디에 반영했는가"만 남긴다.

---

[문서 인덱스](../README.md) · [PRD 인덱스](../prd/README.md)
