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

### RSS는 메모리 여유의 대리값이 아니다

`nimwake-android`가 §78·§80을 기기에서 돌리며 확인했다
([리포트 14](https://github.com/nimwake/nimwake-android/blob/main/.docs/validation/14-memory-admission-eviction.md)).

```text
Next.js 블로그가 깨어 있을 때
  RSS               93–97 MB
  MemAvailable 감소  45–50 MB      약 절반
```

공유 페이지(Node 바이너리 · 라이브러리)와 파일 캐시가 RSS에는 들어가고
MemAvailable 감소에는 덜 들어간다. 기기에서 임계값을 가용 −60MB로 두었을 때
eviction이 일어나지 않은 이유가 이것이다.

**§52가 app별 RSS를 모아 "N개를 동시에 깨워도 되는가"를 판단하면 과하게
비관적인 답을 얻는다.** 봐야 하는 것은 RSS의 합이 아니라 **MemAvailable 감소의
합**이다. 둘은 2배 가까이 다르다.

§78·§80이 `MemAvailable`을 직접 보는 설계였던 것이 결과적으로 맞았다 — 그 값은
이 차이를 이미 반영하고 있다.

## 남은 범위의 착수 조건

- [x] 실기기에서 `--min-free-memory-mb`를 켜고 돌린 데이터 — **충족.** 기기가
      `--min-free-memory-mb 512`로 떠 있고
      [리포트 14](https://github.com/nimwake/nimwake-android/blob/main/.docs/validation/14-memory-admission-eviction.md)가
      §78·§80을 실증했다
- [ ] 그 데이터에서 §78·§80으로 **부족한 것**이 확인됨 ([PRD §64 Rule 8](../../prd/06-conventions/01-development-rules.md))
      — **미충족.** 둘은 동작한다

### 둘째 조건이 왜 미충족인가

리포트 14는 §78과 §80이 **동작한다**는 것을 보였다. 설계 제약 하나를 주었지만
(위의 RSS ↔ MemAvailable) 부족함을 보이지는 않았다.

실사용 수치가 그것을 뒷받침한다. 블로그 + echo를 돌리는 기기에서:

```text
MemAvailable 최소   2472 MB        임계값 512 MB의 4.8배
evicting …          0건
refusing to wake    0건
```

**메모리 압박이 실제로 오지 않는다.** 2 GB 급 여유가 남는 기기에서 §52의 수집을
만들면 쓰이지 않는 계약이 하나 늘어난다.

§54의 `priority`와 `always_on`도 같다 — 지금 §80이 "가장 오래 유휴인 것"을 고르는데,
그 선택이 틀렸던 사례가 아직 없다(고른 적이 없으므로).

**이 절을 열려면 기기에서 무엇이 모자랐는지가 먼저 나와야 한다.** 앱이 늘거나
더 무거워져서 압박이 실제로 오는 때다.

**`priority`와 `always_on`을 먼저 만들지 않는다.** manifest 항목은 한 번
내보내면 계약이 되고, 지금은 "어느 app을 지켜야 하는가"의 실제 사례가 없다.

---

[페이즈 로드맵](../README.md) · [문서 인덱스](../../README.md) · [PRD 인덱스](../../prd/README.md)
