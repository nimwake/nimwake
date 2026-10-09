# 완료 체크리스트

> **Phase 1 — Core MVP** › 완료 판단
>
> 판정 기준 원장: [PRD §65 Core MVP Definition of Done](../../prd/03-delivery/08-definition-of-done.md)

---

이 문서는 PRD §65의 항목을 **추적용 체크리스트**로 옮긴 것이다.

항목을 추가하거나 바꿔야 한다면 PRD §65를 먼저 고치고 여기를 맞춘다.

---

## 읽는 방법

각 항목에 **무엇이 그것을 보장하는지**를 함께 적는다.
체크만 있으면 "누가 언제 무슨 근거로 체크했는가"가 남지 않고,
남지 않으면 회귀가 생겼을 때 어디를 보아야 하는지 알 수 없다.

근거는 셋 중 하나다.

| 표기 | 의미 |
| --- | --- |
| `test` | 자동 테스트. 이름으로 찾을 수 있다 |
| `gate` | CI workflow 단계 |
| `report` | release artifact에 기록되는 측정값 |

> 수동 테스트에서 동작하는 것은 `v0.1.0`의 근거가 되지 않는다.
> → [v0.1.0 마일스톤](03-milestones/06-v0.1.0-core-mvp.md)

---

## 런타임

- [x] Rust daemon 실행 — `test` `the_primary_regression_scenario` (실제 바이너리를 띄운다)
- [x] manifest parsing — `test` `registry::tests::*`, `spec::tests::*`
- [x] manifest validation (unique host / unique port / `port != gateway port`) — `test` `registry_rejects_conflicting_manifests`, `a_port_conflict_refuses_to_start`
- [x] Runtime abstraction — `test` 전이 테스트 26개가 `Runtime` 없이 돈다. adapter는 `FakeRuntime` / `ProcessRuntime` 둘이고 lifecycle semantics는 같다
- [x] real child process spawn — `test` `start_waits_for_readiness_and_stop_removes_the_process`
- [x] `Child` 소유권이 `ProcessController` 한 곳에 있음 — `gate` `ci.yml`의 "Forbid Child ownership outside ProcessController"
- [x] stdout/stderr를 inherit하거나 drain (방치된 pipe 없음) — `test` `a_noisy_process_keeps_serving`, `a_noisy_app_keeps_serving`
- [x] process monitoring — `test` `a_process_that_dies_while_running_fails_then_restarts`, `wait_reports_the_exit_status_of_a_process_that_dies_on_its_own`
- [x] TCP readiness — `test` `start_fails_when_the_process_exits_before_readiness` (readiness가 성립하지 않으면 기동이 실패한다)
- [x] startup timeout — `test` `startup_timeout_moves_to_failed_and_leaves_no_process`, `an_app_that_never_becomes_ready_returns_502`

---

## 게이트웨이

- [x] Host routing — `test` `routing_is_driven_by_the_host_header`, `an_unknown_host_wakes_nothing`
- [x] HTTP reverse proxy — `test` `a_post_body_survives_a_cold_start` (body가 왕복한다)
- [x] cold start — `test` `request_wakes_the_app_serves_it_sleeps_and_wakes_again`
- [x] concurrent cold-start deduplication — `test` `thousand_concurrent_acquires_start_exactly_one_process`, `the_primary_regression_scenario` (100 동시 → unique pid 1)
- [x] `acquire()` 단일 API — backend 확보와 request 예약이 원자적 — `test` `sleeping_plus_acquire_enters_starting`, `running_plus_acquire_reuses_the_process` (한 전이 안에서 일어난다)
- [x] request tracking이 lease 발급/반납으로만 변경됨 — `test` `a_duplicate_request_finished_does_not_change_the_counter`, `a_lease_outlives_the_response_body`
- [x] mailbox / pending waiter 상한 초과 시 `503` — `test` `the_pending_waiter_limit_rejects_with_overloaded`, `requests_beyond_the_pending_limit_get_503`

---

## Scale-to-Zero

- [x] idle shutdown — `test` `an_idle_application_stops_its_process`, `idle_with_no_active_lease_enters_stopping`
- [x] stale timer protection — `test` `a_stale_idle_timer_is_ignored`, `an_early_idle_timer_does_not_stop_the_app`, `a_request_before_the_idle_deadline_keeps_the_app_alive`
- [x] graceful shutdown — `test` `graceful_shutdown_waits_for_the_shutdown_timeout`, `the_node_example_shuts_down_gracefully` (실제 앱이 `SIGTERM`에 스스로 나간다)
- [x] forced shutdown — `test` `sigterm_is_escalated_to_sigkill`, `stop_terminates_the_whole_process_group`
- [x] process crash detection — `test` `running_plus_process_exit_enters_failed`, `a_process_that_dies_while_running_fails_then_restarts`
- [x] retry after failure — `test` `failed_plus_acquire_enters_starting`, `an_app_that_crashes_is_restarted_by_the_next_request`

---

## 동시성 불변식

- [x] no duplicate processes — `test` `thousand_concurrent_acquires_start_exactly_one_process` · `report` cold-start storm의 `spawn count`
- [x] no lost startup waiter — `test` `startup_failure_rejects_every_waiter`, `stopping_plus_acquire_restarts_after_the_stop_completes`, `teardown_rejects_pending_waiters`
- [x] no lifecycle deadlock — `gate` 긴 단계마다 `timeout-minutes`를 두어 멈추면 **어느 단계인지와 함께** 실패한다 (release gate: Build 20 · Tests 20 · Stress 25분) · `report` lifecycle stress의 `failed requests`
- [x] no orphan process after stress test — `report` lifecycle stress의 `orphan processes` / `zombie processes` (release 실측: 둘 다 0)
- [x] stale `StartCompleted` / `StopCompleted`가 state를 변경하지 않음 — `test` `a_stale_start_completed_is_ignored`, `a_stale_stop_completed_is_ignored`, `a_stale_start_completed_stops_the_orphan_process`
- [x] 교체된 process의 `ProcessExited`가 state를 변경하지 않음 — `test` `a_stale_process_exit_is_ignored`
- [x] daemon 종료 후 managed child process 잔존 0 — `test` `no_child_survives_the_daemon`, `registry_shutdown_leaves_no_managed_child` · `gate` `release.yml`의 "Assert nothing survived"

---

## 테스트

- [x] unit tests pass — `gate` `ci.yml` / `release.yml`의 `cargo test --workspace`
- [x] concurrency tests pass — `gate` 위와 같음 (`tests/concurrency.rs`)
- [x] E2E tests pass — `gate` 위와 같음 (`crates/nimwaked/tests/e2e.rs`, 실제 바이너리)
- [x] cold-start storm passes — `gate` `release.yml`의 "Stress and leak checks" · `report` `spawn count == 1`
- [x] lifecycle stress passes — `gate` 위와 같음 · `report` 1000 cycles / orphan 0 / zombie 0
- [x] release CI passes — `gate` `v0.1.0-rc1` 태그에서 세 job이 모두 통과했다
      (`qualify` / `aarch64` / `publish`). `publish`는 이 rc에서 처음 실행됐다

---

## 계측

- [x] cold start가 구간별 breakdown으로 기록됨 — `report` actor queue / process spawn / app readiness / total · `test` `benchmark_mode_emits_span_measurements`
- [x] warm path proxy overhead가 direct 대비로 기록됨 — `report` Warm path 표의 `overhead` 열
- [x] concurrency sweep 결과가 기록됨 — `report` Concurrency sweep 표 + knee point
- [x] performance report artifact가 release마다 생성됨 — `gate` `release.yml`의 "Upload the report" (`performance-report.md`, `baseline.txt`)

---

## Android Device

[`nimwake-android` Phase 1](https://github.com/nimwake/nimwake-android/tree/main/.docs/phases/phase-1)의 검증 리포트로 판정한다.

```text
Install ✓   Boot recovery ✓   Cold start ✓   Scale-to-zero ✓
Daemon recovery ✓   Network recovery ✓   24h soak ✓   72h soak ✓
```

**이 절은 Core가 체크할 수 없다.** 입력이 다른 저장소의 리포트다.
Core가 할 일은 aarch64 바이너리를 공급하는 것까지이고, 그것은 `release.yml`에 있다.

- [ ] Install
- [ ] Boot recovery
- [ ] Cold start
- [ ] Scale-to-zero
- [ ] Daemon recovery
- [ ] Network recovery
- [ ] 24h soak
- [ ] 72h soak

---

## 남은 것

```text
Android        8항목        nimwake-android 리포트 대기
```

**Core 쪽은 전부 끝났다.** 구현도, release gate도 실제 태그로 검증됐다.

`v0.1.0-rc1`에서 확인한 것:

```text
release        prerelease로 표시됨. /releases/latest가 404 →
               다음 정식 release의 baseline 비교가 rc를 집어 가지 않는다
assets         nimwake-aarch64-linux-musl.tar.gz (2.67 MB)
               performance-report.md · baseline.txt
release notes  제약이 먼저(1~56줄), 성능 리포트가 뒤(57줄부터)
baseline       label=v0.1.0-rc1 + 지표 6개. 다음 release가 이것과 비교한다
```

남은 것은 다른 저장소의 입력이다. `v0.1.0` 태깅은 그 리포트를 기다린다.

---

## 이 체크리스트가 보장하지 않는 것

전 항목이 체크되어도 아래는 **측정되지 않은 상태**다.
`v0.1.0`은 "Core lifecycle이 자동 테스트를 통과한 상태"이고 제품의 가용성이 아니다.

| 공백 | 지금 아는 것 |
| --- | --- |
| 실제 서비스의 cold start | Next.js **기동 206ms**, 첫 응답 396ms. 목표 5초에 여유가 크다. 다만 한 페이지짜리 앱이므로 하한이다 |
| readiness의 의미 | TCP connect는 application readiness가 아니다. Next.js는 port를 열고도 190ms 더 걸린다 |
| 동시 기동의 자원 한계 | Next.js 앱 8개를 동시에 깨우면 직렬화 없이 전부 0.87초에 뜬다. 앱당 96MB로 합계 768MB. 2GB 기기면 10개 남짓이 한계인데 제한 장치가 없다 (자원 관리는 Post-MVP) |
| 장시간 운용 | 최장 측정이 105초(stress) / 120초(sustained). 24h·72h는 Android 소관 |
| crash-loop | backoff가 없다 (PRD §33, Post-MVP). **누수는 없음을 확인했다** — 동시 자식 ≤ app 수, 정착 후 zombie 0, RSS +1.8MB, FD 증가 없음. 다만 spawn 속도를 막는 것이 없다. 623 spawn/초를 쟀고 `startup_timeout`도 waiter 상한도 관여하지 않는다 |
| manifest 재적재 | 없다. 앱 추가에 daemon 재시작이 필요하고 그러면 모든 앱이 내려간다 |
| 운영 도구 | `nimwake status`도 admin IPC도 없다 (Post-MVP) |

실측 근거는 [05. 설계 결정 기록](05-design-decisions.md)의 "실측: 실제 애플리케이션"에 있다.

---

전 항목이 충족되면 다음으로 태깅한다.

```text
Nimwake v0.1.0 — Core MVP
```

---

← [v0.1.0 Core MVP](03-milestones/06-v0.1.0-core-mvp.md) · [페이즈 개요](README.md) · [설계 결정 기록](05-design-decisions.md) →
