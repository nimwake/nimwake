# Long-Term Roadmap

> **Nimwake PRD** › Post Core MVP
>
> 원문 구간: `PRD.md` §62–§63

---

## 62. Long-Term Roadmap

로드맵은 페이즈 단위로 실행한다.

| 페이즈                                       | 버전     | 주제                                       | PRD 근거      |
| -------------------------------------------- | -------- | ------------------------------------------ | ------------- |
| [Phase 1](../../phases/phase-1/README.md)    | `v0.1.0` | Core Lifecycle / Scale-to-Zero             | §7, §9–§36    |
| [Phase 2](../../phases/phase-2/README.md)    | `v0.2.x` | CLI · 상태 저장 · 배포 · 롤백              | §56–§59       |
| [Phase 3](../../phases/phase-3/README.md)    | `v0.3.x` | 리소스 모니터링 · memory pressure · eviction | §52–§55       |
| [Phase 4](../../phases/phase-4/README.md)    | Later    | Android host tooling · 스트리밍 · 컨테이너 | §60–§61, §63  |

버전 흐름:

```text
v0.0.1
Lifecycle foundation

v0.0.2
ProcessRuntime

v0.0.3
HTTP + scale-to-zero

v0.0.4
E2E + load environment

v0.0.5
Stress + release qualification

v0.1.0
Core MVP


v0.2.x
CLI
SQLite state
deployment
release management
rollback


v0.3.x
resource monitoring
memory pressure
eviction
priority scheduling


Later
Android host tooling
WebSocket / SSE
streaming
containers
cgroups
dashboard
advanced health checks
scheduled wake
cron / workers
blue-green deployment
automatic rollback
integrations
```

---

## 63. Post-MVP 후보

- Web dashboard
- WebSocket cold start
- SSE
- streaming request handling
- request body 크기 상한 / long-lived connection 정책
- dynamic port allocation
- crash-loop backoff
- process adoption (daemon restart 후 기존 process 인수)
- 상시 metrics exporter (Prometheus 등)
- scheduled wake
- cron jobs
- background workers
- min instances
- encrypted secrets
- deployment hooks
- blue/green release
- health-based rollback
- cgroup resource limits
- Docker Runtime
- Podman Runtime
- MicroVM Runtime
- Cloudflare integration
- Caddy integration
- Tailscale integration
- reusable CI workflows

---

← [CLI · Deployment · Storage · Observability · Security](../05-beyond-mvp/02-platform-features.md) · [문서 인덱스](../README.md) · [Development Rules](../06-conventions/01-development-rules.md) →
