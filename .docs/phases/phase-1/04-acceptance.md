# 완료 체크리스트

> **Phase 1 — Core MVP** › 완료 판단
>
> 판정 기준 원장: [PRD §65 Core MVP Definition of Done](../../prd/03-delivery/08-definition-of-done.md)

---

이 문서는 PRD §65의 항목을 **추적용 체크리스트**로 옮긴 것이다.

항목을 추가하거나 바꿔야 한다면 PRD §65를 먼저 고치고 여기를 맞춘다.

---

## 런타임

- [ ] Rust daemon 실행
- [ ] manifest parsing
- [ ] manifest validation (unique host / unique port / `port != gateway port`)
- [ ] Runtime abstraction
- [ ] real child process spawn
- [ ] `Child` 소유권이 `ProcessController` 한 곳에 있음
- [ ] stdout/stderr를 inherit하거나 drain (방치된 pipe 없음)
- [ ] process monitoring
- [ ] TCP readiness
- [ ] startup timeout

## 게이트웨이

- [ ] Host routing
- [ ] HTTP reverse proxy
- [ ] cold start
- [ ] concurrent cold-start deduplication
- [ ] `acquire()` 단일 API — backend 확보와 request 예약이 원자적
- [ ] request tracking이 lease 발급/반납으로만 변경됨
- [ ] mailbox / pending waiter 상한 초과 시 `503`

## Scale-to-Zero

- [ ] idle shutdown
- [ ] stale timer protection
- [ ] graceful shutdown
- [ ] forced shutdown
- [ ] process crash detection
- [ ] retry after failure

## 동시성 불변식

- [ ] no duplicate processes
- [ ] no lost startup waiter
- [ ] no lifecycle deadlock
- [ ] no orphan process after stress test
- [ ] stale `StartCompleted` / `StopCompleted`가 state를 변경하지 않음
- [ ] 교체된 process의 `ProcessExited`가 state를 변경하지 않음
- [ ] daemon 종료 후 managed child process 잔존 0

## 테스트

- [ ] unit tests pass
- [ ] concurrency tests pass
- [ ] E2E tests pass
- [ ] cold-start storm passes
- [ ] lifecycle stress passes
- [ ] release CI passes

## 계측

- [ ] cold start가 구간별 breakdown으로 기록됨
- [ ] warm path proxy overhead가 direct 대비로 기록됨
- [ ] concurrency sweep 결과가 기록됨
- [ ] performance report artifact가 release마다 생성됨

## Android Device

[`nimwake-android` Phase 1](https://github.com/nimwake/nimwake-android/tree/main/.docs/phases/phase-1)의 검증 리포트로 판정한다.

```text
Install ✓   Boot recovery ✓   Cold start ✓   Scale-to-zero ✓
Daemon recovery ✓   Network recovery ✓   24h soak ✓   72h soak ✓
```

- [ ] Install
- [ ] Boot recovery
- [ ] Cold start
- [ ] Scale-to-zero
- [ ] Daemon recovery
- [ ] Network recovery
- [ ] 24h soak
- [ ] 72h soak

---

전 항목이 충족되면 다음으로 태깅한다.

```text
Nimwake v0.1.0 — Core MVP
```

---

← [v0.1.0 Core MVP](03-milestones/06-v0.1.0-core-mvp.md) · [페이즈 개요](README.md) · [설계 결정 기록](05-design-decisions.md) →
