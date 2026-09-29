# Non-Goals와 Core MVP 범위

> **Nimwake PRD** › 제품 정의
>
> 원문 구간: `PRD.md` §6–§8

---

## 6. Non-Goals

초기 Nimwake에서는 다음을 목표로 하지 않는다.

- Kubernetes API compatibility
- Multi-node cluster
- Horizontal Auto Scaling
- Multi-region
- High Availability
- Service Mesh
- distributed scheduler
- GPU workload orchestration
- Local LLM inference platform
- AWS Lambda 수준의 isolation
- hostile multi-tenant workload
- public multi-tenant PaaS
- container orchestration platform
- 대규모 production traffic
- database scale-to-zero
- application build system

초기 사용 목적은 다음과 같다.

```text
Personal Homelab
Side Projects
Low-Traffic APIs
Personal Websites
Internal Tools
Experimental Services
```

---

## 7. Core MVP 범위

Nimwake Core MVP의 목적은 하나의 lifecycle을 신뢰성 있게 증명하는 것이다.

```text
HTTP request
      ↓
sleeping application
      ↓
single cold start
      ↓
readiness
      ↓
HTTP proxy
      ↓
idle timeout
      ↓
graceful shutdown
      ↓
sleep
```

Core MVP에서 반드시 검증해야 하는 것은 기능 수가 아니라 lifecycle correctness다.

---

## 8. Core MVP에서 제외하는 기능

다음은 장기적으로 필요할 수 있지만 `v0.1.0 Core MVP`에는 포함하지 않는다.

- SQLite state persistence
- full CLI management system
- artifact deployment
- GitHub Actions 기반 자동 배포
- rollback
- memory-pressure eviction
- LRU scheduler
- per-app hard memory limits
- Docker runtime
- Podman runtime
- container isolation
- cgroup integration
- dashboard
- WebSocket cold start
- SSE
- streaming upload
- PostgreSQL lifecycle
- Cloudflare integration
- Tailscale integration
- TLS termination
- DNS management
- multi-node
- Android-specific supervisor

---

← [제품 핵심 원칙](../01-product/02-principles.md) · [문서 인덱스](../README.md) · [Core Architecture와 상태 모델](../02-architecture/01-core-architecture.md) →
