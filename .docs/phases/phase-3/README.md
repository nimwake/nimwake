# Phase 3 — 리소스 인식 스케줄링

> **목표 버전:** `v0.3.x`
> **주제:** 리소스 모니터링 · memory pressure · eviction
> **상태:** 일부 선행 구현 — 아래 "이미 들어간 것" 참고

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

## 이미 들어간 것

Phase 2보다 먼저 들어왔다. `v0.1.0-rc4`·`rc5`에서 실기기 측정이 "제한 장치가
없다"를 반복해서 가리켰기 때문이다 ([Rule 8](../../prd/06-conventions/01-development-rules.md)의
"실제 문제로 확인됨"을 만족했다).

| 들어간 것 | 계약 | 근거 |
| --------- | ---- | ---- |
| `minimum_free_memory_mb` 기반 startup 차단 | [§78 Memory Admission Control](../../prd/02-architecture/06-gateway-and-requests.md) | Next.js 앱 하나가 96~118MB, 제한 장치가 없었다 |
| eviction 트리거와 후보 선택 | [§80 Idle Eviction](../../prd/02-architecture/06-gateway-and-requests.md) | §78만으로는 아무도 쓰지 않는 메모리를 쥔 채 거절한다 |

둘 다 `--min-free-memory-mb` 하나로 켜지고 **기본값 `0`에서는 `/proc`을 읽지도
않는다.** §54의 우선순위 다섯 단계 중 두 개(활성 요청 없음 · 오래된 last request)만
쓴다.

## 남은 범위

- 시스템 / 프로세스 리소스 수집 (`sysinfo` + `/proc` fallback) — §52
- eviction 후보 우선순위 — `priority` · `always_on` manifest 항목 (§54의 나머지)
- application별 resource limit — §55
- **동시에 깨어나는 경우의 과다 commit** — app마다의 예상 RSS를 먼저 알아야 한다.
  §52의 수집이 선행 조건이다

이 페이즈는 [Invariant 5 — Active Request Protection](../../prd/02-architecture/02-lifecycle-invariants.md)을 깨지 않아야 한다.
active request가 있는 애플리케이션은 eviction 대상이 될 수 없다.

---

## 기기에서 확인된 제약

`nimwake-android`가 §52의 수집 설계에 걸리는 것을 먼저 확인했다
([nimwake-android#43](https://github.com/nimwake/nimwake-android/pull/43)).

```text
/proc/meminfo            Termux · proot 모두 읽힌다 (MemTotal 5452048 kB)
/proc/pressure/memory    막혀 있다 — PSI를 쓸 수 없다
```

**PSI(Pressure Stall Information)를 전제로 설계하면 안 된다.** Linux 4.20+의
표준이지만 Android가 앱에게 열어 주지 않는다. `MemAvailable` 폴링이 기기에서
쓸 수 있는 유일한 길이다.

§78·§80이 `MemAvailable`을 쓰는 것은 그래서 맞는 선택이었다. §52의 수집도
같은 제약 아래 설계해야 한다.

## 남은 범위의 착수 조건

- [ ] 실기기에서 `--min-free-memory-mb`를 켜고 돌린 데이터 — 지금은 꺼져 있다
- [ ] 그 데이터에서 §78·§80으로 부족한 것이 확인됨 ([PRD §64 Rule 8](../../prd/06-conventions/01-development-rules.md))

**`priority`와 `always_on`을 먼저 만들지 않는다.** manifest 항목은 한 번
내보내면 계약이 되고, 지금은 "어느 app을 지켜야 하는가"의 실제 사례가 없다.

---

[페이즈 로드맵](../README.md) · [문서 인덱스](../../README.md) · [PRD 인덱스](../../prd/README.md)
