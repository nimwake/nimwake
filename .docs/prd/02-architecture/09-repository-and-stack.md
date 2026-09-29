# Repository Architecture와 Technology Stack

> **Nimwake PRD** › 아키텍처
>
> 원문 구간: `PRD.md` §34–§36

---

## 34. Repository Architecture

초기에는 architecture를 과도하게 crate 단위로 나누지 않는다.

```text
nimwake/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── rustfmt.toml
├── clippy.toml
├── README.md
├── LICENSE
│
├── crates/
│   ├── nimwaked/
│   ├── nimwake-core/
│   └── nimwake-cli/
│
├── tests/
│   ├── fixtures/
│   ├── e2e/
│   └── load/
│
├── examples/
│   └── hello-node/
│
├── .docs/
│   ├── README.md
│   ├── prd/
│   └── phases/
│
└── .github/
    └── workflows/
        ├── ci.yml
        ├── integration.yml
        └── release.yml
```

Core 내부:

```text
nimwake-core/src/

├── lib.rs
│
├── app/
│   ├── mod.rs
│   ├── spec.rs
│   ├── state.rs
│   ├── command.rs
│   ├── manager.rs
│   └── handle.rs
│
├── runtime/
│   ├── mod.rs
│   ├── process.rs
│   └── handle.rs
│
├── gateway/
│   ├── mod.rs
│   ├── router.rs
│   └── proxy.rs
│
└── registry/
    └── mod.rs
```

MVP에서는 의도적으로 crate 수를 적게 유지하고, subsystem은 우선 `nimwake-core`의 module로 둔다.

Gateway, Runtime, Resource Manager, Storage 등의 crate extraction은 complexity가 실제로 발생했을 때 수행한다.

---

## 35. Technology Stack

| 영역              | 기술                                 |
| ----------------- | ------------------------------------ |
| Language          | Rust                                 |
| Async Runtime     | Tokio                                |
| HTTP              | Hyper                                |
| Internal HTTP API | Axum 필요 시 사용                    |
| Serialization     | Serde                                |
| Configuration     | TOML                                 |
| Logging           | tracing                              |
| CLI               | clap                                 |
| Process Control   | tokio::process, Unix signals         |
| IPC               | Unix Domain Socket, Post-MVP         |
| Internal Storage  | SQLite, Post-MVP                     |
| System Metrics    | sysinfo + `/proc` fallback, Post-MVP |

MVP에서 필수 의존하지 않는 구성:

- Docker
- PM2
- Redis
- Kubernetes
- Nginx
- systemd
- Prometheus
- Grafana

---

## 36. Nimwake Daemon

항상 실행되는 핵심 binary 이름은:

```text
nimwaked
```

장기적으로 다음 책임을 가진다.

```text
nimwaked

├── HTTP Gateway
├── Host Router
├── Reverse Proxy
├── App Registry
├── Lifecycle Manager
├── Process Supervisor
├── Startup Coordinator
├── Idle Manager
│
├── Future:
│   ├── Resource Monitor
│   ├── Eviction Manager
│   ├── Metrics
│   ├── Health Checker
│   └── Admin IPC
```

가능하면 하나의 Rust binary로 운영하는 것을 목표로 한다.

---

← [Failure Handling](../02-architecture/08-failure-handling.md) · [문서 인덱스](../README.md) · [Core MVP Version Plan](../03-delivery/01-version-plan.md) →
