# CI Strategy

> **Nimwake PRD** › 딜리버리
>
> 원문 구간: `PRD.md` §44

---

## 44. CI Strategy

CI를 세 단계로 나눈다.

### Pull Request CI

모든 PR에서:

```bash
cargo fmt --all -- --check

cargo clippy \
  --workspace \
  --all-targets \
  --all-features \
  -- \
  -D warnings

cargo test --workspace
```

추가로 FakeRuntime concurrency test를 실행한다.

목적:

```text
fast feedback
+
lifecycle correctness
```

---

### Main Branch Integration CI

main merge 이후:

```text
build release binary
       ↓
start nimwaked
       ↓
start fixtures
       ↓
HTTP E2E
       ↓
cold start
       ↓
scale-to-zero
       ↓
crash recovery
       ↓
performance baseline 기록
```

실제 OS process behavior를 검증하고, 성능 지표를 **판정 없이 기록**한다.

---

### Release Gate

release tag 생성 시:

```text
Build
 ↓
Unit Tests
 ↓
Concurrency Tests
 ↓
E2E
 ↓
Cold-start Storm
 ↓
Warm Load
 ↓
Lifecycle Stress
 ↓
Leak Checks
 ↓
Performance Report 생성
 ↓
PASS
 ↓
Release
```

required test가 하나라도 실패하면 release artifact를 publish하지 않는다.

---

### Gate 판정 기준

초기 release gate는 **correctness만** 차단 사유로 삼는다.

```text
duplicate process == 0
orphan process == 0
deadlock == 0
lost waiter == 0
HTTP errors == 0
```

성능 수치는 gate가 아니라 리포트로 누적한다.

```text
baseline → measurement → trend → regression warning
```

GitHub-hosted runner는 성능 편차가 크기 때문에,
충분한 baseline이 쌓인 뒤에 regression threshold를 도입한다.
→ [§73 Performance Report와 Regression](07-performance-baseline.md)

---

← [Latency Instrumentation](../03-delivery/05-instrumentation.md) · [문서 인덱스](../README.md) · [Performance Goals와 Regression](../03-delivery/07-performance-baseline.md) →
