# 제품 개요와 문제 정의

> **Nimwake PRD** › 제품 정의
>
> 원문 구간: `PRD.md` §1–§4

---

## 1. 문서 개요

- **프로젝트명:** Nimwake
- **한 줄 설명:** 자원이 제한된 Linux 환경을 위한 Rust 기반 초경량 Scale-to-Zero PaaS
- **핵심 목적:** 다수의 저빈도 애플리케이션을 제한된 메모리 환경에서 효율적으로 호스팅
- **초기 주요 타깃:** Linux x86_64 / ARM64, Android + Termux + Ubuntu proot, Raspberry Pi, ARM SBC, Mini PC, Old Laptop, Cheap VPS
- **핵심 철학:** 배포된 애플리케이션 수와 실행 중인 애플리케이션 수를 분리한다.
- **외부 인프라 필수 의존성:** 없음
- **Cloudflare / Tailscale / GitHub Actions:** Optional Integration

---

## 2. 제품 정의

> **Nimwake is a lightweight, runtime-agnostic, scale-to-zero PaaS for resource-constrained Linux machines.**

Nimwake는 배포된 모든 애플리케이션을 항상 실행하지 않는다.

애플리케이션은 서버에 배포된 상태로 존재하다가 실제 HTTP 요청이 발생하면 시작되고, 일정 시간 동안 요청이 없으면 다시 종료된다.

```text
SLEEPING
   │
   │ request
   ▼
STARTING
   │
   │ ready
   ▼
RUNNING
   │
   │ idle timeout
   ▼
STOPPING
   │
   ▼
SLEEPING
```

이를 통해 제한된 RAM에서도 많은 애플리케이션을 배포해 두고 실제로 필요한 애플리케이션만 메모리에 유지할 수 있다.

Nimwake가 제공하려는 핵심 경험은 다음과 같다.

```text
request
   ↓
wake
   ↓
serve
   ↓
idle
   ↓
sleep
   ↓
repeat
```

---

## 3. 문제 정의

일반적인 self-hosted PaaS는 다음과 같은 구성을 사용하는 경우가 많다.

```text
Linux Server
    ↓
Container Runtime
    ↓
Reverse Proxy
    ↓
Always-running Applications
```

하지만 1~4GB RAM 수준의 저사양 장비에서는 다음 문제가 발생할 수 있다.

- 여러 애플리케이션 runtime의 상주 메모리
- 여러 Node.js / Next.js / Python 프로세스의 중복 메모리
- Docker 및 orchestration overhead
- Redis / PostgreSQL / reverse proxy 등의 추가 상주 프로세스
- build 과정에서 발생하는 높은 CPU / RAM 사용량
- Android 환경에서 Docker / systemd / cgroup 사용의 제약
- 대부분의 시간을 사용하지 않는 개인 서비스의 자원 낭비

Nimwake는 다음 조합을 통해 이 문제를 해결하는 것을 목표로 한다.

```text
Raw Process Runtime
        +
Scale-to-Zero
        +
Low Runtime Overhead
        +
Single Machine First
```

---

## 4. 핵심 제품 목표

1. 다수의 애플리케이션을 하나의 저사양 서버에 배포할 수 있다.
2. 실제 트래픽이 있는 애플리케이션만 실행한다.
3. HTTP Host를 기반으로 애플리케이션을 라우팅한다.
4. 요청이 발생하면 sleeping application을 자동으로 시작한다.
5. 일정 시간 idle 상태가 유지되면 자동으로 종료한다.
6. 동일 앱에 concurrent request가 들어와도 process는 한 번만 시작한다.
7. application runtime 및 framework에 종속되지 않는다.
8. 외부 ingress provider에 종속되지 않는다.
9. 특정 CI/CD provider에 종속되지 않는다.
10. Docker 없이 실행 가능해야 한다.
11. Rust daemon 하나를 중심으로 낮은 memory footprint를 유지한다.
12. Android에서 검증하되 제품 자체는 일반 Linux 환경을 대상으로 한다.
13. 향후 memory pressure에 따라 application eviction을 수행할 수 있어야 한다.
14. Core architecture가 향후 Process / Container / MicroVM runtime으로 확장 가능해야 한다.

---

[문서 인덱스](../README.md) · [제품 핵심 원칙](../01-product/02-principles.md) →
