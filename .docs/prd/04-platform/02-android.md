# Android Deployment와 검증

> **Nimwake PRD** › 플랫폼
>
> 원문 구간: `PRD.md` §49–§51

---

## 49. Android Deployment Profile

Android-specific functionality는 별도 project/layer에서 관리한다.

```text
Android
   │
Termux
   │
Host Supervisor
   │
Ubuntu / proot
   │
nimwaked
   │
applications
```

Host Layer 책임:

- boot recovery
- Termux:Boot
- wake lock
- battery optimization guidance
- Ubuntu proot lifecycle
- nimwaked startup/recovery
- network recovery
- storage monitoring
- log rotation
- restart policy

Host Layer의 기동 순서:

```text
Termux:Boot
     ↓
wake lock
     ↓
start Ubuntu proot
     ↓
start nimwaked
     ↓
health check
```

이 로직은 Nimwake Core 안에 넣지 않는다.

Android-specific behavior는 `nimwake-core`가 아니라 별도 layer에 둔다.

```text
nimwake-android
```

---

## 50. Android Validation

Core `v0.1.0` 이후 실제 Android device에서 검증한다.

필수 시나리오:

```text
Android reboot
   ↓
Termux boot
   ↓
nimwaked recovery
```

boot recovery 직후 application은 sleeping 상태를 유지해야 한다.

그리고:

```text
HTTP request
   ↓
cold start
   ↓
response
   ↓
idle
   ↓
sleep
```

실제 application으로 Next.js standalone artifact 등을 사용할 수 있다.

application build는 phone 외부에서 수행하는 것을 기본 정책으로 한다.

### CI Linux와 동일 지표 비교

Android 검증은 별도 metric 체계를 만들지 않는다.

[§72 Latency Instrumentation](../03-delivery/05-instrumentation.md)에서 정의한 지표를
**CI Linux와 Android 실기기에서 같은 workload로 실행해 비교한다.**

```text
동일 workload
      │
      ├── CI Linux runner
      │
      └── 4GB Android + Termux + proot

비교 항목

proxy overhead p50 / p95
cold start breakdown (spawn / readiness / proxy connect)
daemon RSS
CPU
concurrency knee point
```

저사양 환경에서 **어느 구간이 먼저 무너지는지**가 Nimwake의 실제 최적화 대상이다.

---

## 51. Android Soak Test

초기:

```text
24 hour
   ↓
72 hour
```

관찰:

- daemon survival
- Android background process killing
- memory usage
- battery usage
- temperature
- wake reliability
- network recovery
- orphan process
- storage growth
- repeated lifecycle stability

이 검증을 통해 Android home-server use case의 현실성을 확인한다.

---

← [Target Environments와 외부 의존성](../04-platform/01-environments.md) · [문서 인덱스](../README.md) · [Resource Management](../05-beyond-mvp/01-resource-management.md) →
