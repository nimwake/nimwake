# Nimwake

> A lightweight, runtime-agnostic, scale-to-zero PaaS for resource-constrained Linux machines.

**Nimwake** wakes applications when they are needed and lets them sleep when they are not.

The project is designed for small Linux environments where keeping every application running continuously is unnecessary or expensive — including low-memory VPS instances, mini PCs, home servers, and Android devices running Linux through Termux/proot.

```text
request
   │
   ▼
┌─────────┐
│ Nimwake │
└────┬────┘
     │
     │ wake
     ▼
┌─────────────┐
│ Application │
└──────┬──────┘
       │
       │ idle
       ▼
     sleep
```

## Status

> **Version: `v0.1.4` — Core Lifecycle**

Nimwake runs. It serves a real Next.js blog on a real phone — a Galaxy A51 5G on
Android 13, through Termux and proot — reachable from the internet through a
tunnel. Cold start is 1.6 seconds, of which Nimwake's own share is about 30 ms;
warm requests take 27–52 ms. Across 72-hour and 24-hour soak runs it logged zero
non-200 responses, zero daemon restarts and zero orphaned processes.

What works today:

- HTTP gateway with host-based routing and reverse proxy
- real process execution, readiness probing and graceful shutdown
- scale-to-zero: idle applications are stopped and woken by the next request
- concurrent cold-start deduplication — N simultaneous requests spawn one process
- crash-loop backoff, memory admission control and idle eviction under pressure
- manifest reload on `SIGHUP`, status snapshot on `SIGUSR1`
- `nimwake status` over a unix socket, with `--json` for scripts
- per-application peer allowlists — `allow = ["192.168.0.0/16"]`
- `X-Forwarded-For`, request-span instrumentation, a benchmark harness

```text
$ nimwake status --socket /run/nimwake/nimwake.sock
nimwaked 0.1.4
APP              STATE           PID  ACTIVE     IDLE
blog             RUNNING       12971       0     3.1s
notes            SLEEPING          —       0        —
```

Not yet:

- authentication, TLS or rate limiting — **by design**, see [Security](#security)
- WebSocket upgrades — refused with `501` rather than silently downgraded
- Server-Sent Events work, but hold the application awake while a stream is open
- deployment and rollback — the host layer owns these, not Nimwake
- persistent state across daemon restarts: a restart stops every application
- commands that change state (`wake`, `sleep`, `restart`) — `status` only, for now

Nimwake is **pre-1.0 and not ready for production use**. The HTTP contract and
the manifest format may still change.

## Security

**Do not expose `nimwaked` directly to the internet.** It provides no
authentication, no TLS and no rate limiting. That is a scope decision, not an
oversight — exposure belongs to the thing that does exposure (a tunnel, a reverse
proxy), and Nimwake handles lifecycle.

The gateway listens on `127.0.0.1:8080` by default. Binding elsewhere is opt-in
through `--listen`.

Read [SECURITY.md](SECURITY.md) before exposing anything, and report
vulnerabilities privately through
[Security Advisories](https://github.com/nimwake/nimwake/security/advisories/new)
rather than public issues.

## License

[MIT](LICENSE).

---

# Why Nimwake?

A small server may host many applications while only a few of them receive traffic at any given time.

Traditional deployment often looks like:

```text
App A ── RUNNING
App B ── RUNNING
App C ── RUNNING
App D ── RUNNING
App E ── RUNNING

even when:

traffic ≈ 0
```

On machines with limited memory, continuously running every application wastes resources.

Nimwake aims for:

```text
App A ── SLEEPING
App B ── SLEEPING
App C ── RUNNING   ← currently receiving traffic
App D ── SLEEPING
App E ── SLEEPING
```

When traffic reaches a sleeping application:

```text
HTTP request
     │
     ▼
  SLEEPING
     │
     ▼
  STARTING
     │
     ▼
   RUNNING
     │
     ▼
serve request
```

After the application becomes idle:

```text
RUNNING
   │
idle timeout
   │
   ▼
STOPPING
   │
   ▼
SLEEPING
```

This allows a machine to have many deployed applications while keeping only active applications in memory.

---

# Design Goals

Nimwake is designed around several principles.

### Scale-to-zero first

Applications should consume almost no runtime resources while unused.

### Low overhead

The Nimwake daemon itself should remain small enough to run comfortably on resource-constrained machines.

### Runtime agnostic

Nimwake should not care whether an application is written in:

```text
Node.js
Python
Go
Rust
Java
Bun
Deno
...
```

The runtime contract is intentionally simple:

> Run this command and expect the application to listen on this port.

### Provider agnostic

Nimwake Core does not depend on:

- Cloudflare
- Tailscale
- GitHub Actions
- AWS
- DigitalOcean
- any specific DNS provider

These can be integrated externally.

### Linux first, Android friendly

Linux is the primary runtime environment.

Android support is planned through:

```text
Android
   ↓
Termux
   ↓
Linux / proot
   ↓
Nimwake
```

Android-specific lifecycle management will remain outside Nimwake Core.

### Single-machine first

Nimwake is not trying to become Kubernetes.

The initial goal is to make scale-to-zero application hosting reliable on **one small machine**.

---

# Architecture

The planned architecture is:

```text
                     HTTP
                      │
                      ▼
               ┌─────────────┐
               │   Gateway   │
               └──────┬──────┘
                      │
                      ▼
                App Registry
                      │
                      ▼
                 AppHandle
                      │
                      ▼
               ┌─────────────┐
               │ AppManager  │
               │             │
               │ State       │
               │ Machine     │
               └──────┬──────┘
                      │
                      ▼
                   Runtime
                      │
                      ▼
              Application Process
```

The gateway never directly controls application processes.

All lifecycle transitions are owned by the application's `AppManager`.

---

# Lifecycle

The initial state machine is:

```text
                    request
                       │
                       ▼
SLEEPING ─────────▶ STARTING
   ▲                   │
   │                   │ ready
   │                   ▼
   │                RUNNING
   │                   │
   │               idle timeout
   │                   ▼
   └────────────── STOPPING

STARTING ── failure ──▶ FAILED

RUNNING ── crash ─────▶ FAILED

FAILED ── retry ──────▶ STARTING
```

The lifecycle architecture follows several invariants.

### One process per application

Regardless of concurrent traffic:

```text
application process count <= 1
```

### One lifecycle authority

Only `AppManager` may mutate application lifecycle state.

### Cold starts are deduplicated

If 1,000 requests arrive simultaneously while an application is sleeping:

```text
1,000 requests
      │
      ▼
  AppManager
      │
      ▼
 Runtime.start()
      │
      │ exactly once
      ▼
 Application
```

All requests wait for the same startup operation.

---

# Why an Actor Model?

Application lifecycle events may arrive concurrently:

```text
HTTP request ───────┐
startup result ─────┤
idle timeout ───────┤
process exit ───────┼──▶ AppManager
shutdown ───────────┤
future CLI ─────────┘
```

Instead of allowing each subsystem to mutate shared state directly, every lifecycle event flows through one per-application manager.

The manager processes state transitions sequentially.

This makes lifecycle races easier to reason about and test.

---

# Runtime Abstraction

Nimwake does not couple lifecycle management to a particular application runtime.

Conceptually:

```rust
trait Runtime {
    async fn start(
        &self,
        app: &AppSpec,
    ) -> Result<ProcessHandle>;

    async fn stop(
        &self,
        process: &ProcessHandle,
    ) -> Result<()>;
}
```

The test suite primarily uses:

```text
FakeRuntime
```

to test lifecycle behavior without creating real operating-system processes.

A real:

```text
ProcessRuntime
```

based on `tokio::process` is planned for `v0.0.2`.

---

# Application Manifest

Applications will eventually be configured using `nimwake.toml`.

Example:

```toml
name = "hello"

[server]
host = "hello.localhost"
port = 3101

[runtime]
command = ["node", "server.js"]
working_dir = "./examples/hello-node"

[lifecycle]
idle_timeout_seconds = 60
startup_timeout_seconds = 5
shutdown_timeout_seconds = 5
```

The manifest format is still experimental and may change before `v0.1.0`.

---

# Repository Structure

The initial repository is intentionally small.

```text
nimwake/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── rustfmt.toml
│
├── crates/
│   ├── nimwaked/
│   ├── nimwake-core/
│   └── nimwake-cli/
│
├── tests/
│
├── examples/
│
└── .docs/
```

Subsystems should remain modules inside `nimwake-core` until there is a clear reason to extract them into separate crates.

---

# Development

## Requirements

Install the Rust toolchain using `rustup`.

The repository uses the toolchain defined in:

```text
rust-toolchain.toml
```

Verify your environment:

```bash
rustc --version
cargo --version
```

---

## Build

```bash
cargo build --workspace
```

---

## Test

```bash
cargo test --workspace
```

Concurrency and lifecycle correctness are considered first-class tests in Nimwake.

---

## Format

```bash
cargo fmt --all
```

Check formatting:

```bash
cargo fmt --all -- --check
```

---

## Lint

```bash
cargo clippy \
  --workspace \
  --all-targets \
  --all-features \
  -- \
  -D warnings
```

---

# Testing Philosophy

Nimwake manages process lifecycle under concurrent traffic.

Race conditions are therefore treated as correctness bugs rather than edge cases.

The most important test is conceptually:

```text
1,000 concurrent EnsureRunning calls

              ↓

         AppManager

              ↓

       FakeRuntime.start()

              ↓

         start_count

              =

              1
```

Required properties include:

- no duplicate startup
- no lost waiters
- no deadlocks
- consistent state transitions
- deterministic failure propagation

Every lifecycle race discovered in development should receive a regression test.

---

# Roadmap

The milestones below are the development history. `.docs/prd/` is authoritative
for what is planned — see [§62](.docs/prd/05-beyond-mvp/03-long-term-roadmap.md).

**`v0.0.1` through `v0.0.5` have shipped.** They were the steps to `v0.1.0`.

### v0.0.1 — Lifecycle Foundation *(shipped)*

- state machine
- AppManager actor
- Runtime abstraction
- FakeRuntime
- cold-start deduplication
- concurrency tests

### v0.0.2 — Process Runtime *(shipped)*

- real process spawning
- process monitoring
- TCP readiness
- startup timeout
- graceful shutdown
- forced termination

### v0.0.3 — HTTP + Scale-to-Zero *(shipped)*

- HTTP gateway
- Host routing
- reverse proxy
- request tracking
- idle shutdown
- stale timer protection

### v0.0.4 — E2E & Load Testing *(shipped)*

- automated E2E environment
- cold-start storm
- crash recovery
- HTTP load tests

### v0.0.5 — Release Qualification *(shipped)*

- lifecycle stress testing
- resource leak detection
- process leak detection
- performance baseline
- release gate

### v0.1.0 — Core MVP

The first Core MVP release.

The goal is not merely that scale-to-zero works manually, but that its lifecycle has passed automated concurrency, E2E, and stress testing.

---

# After v0.1.0

The first major integration target is Android.

```text
Android
   ↓
Termux
   ↓
Ubuntu / proot
   ↓
Nimwake
   ↓
Applications
```

The initial validation will include:

- boot recovery
- automatic daemon startup
- application cold start
- scale-to-zero
- repeated wake/sleep cycles
- realistic Next.js application
- 24-hour soak test
- 72-hour soak test

Android-specific functionality will live outside Nimwake Core.

---

# Non-Goals

Nimwake is currently **not** intended to provide:

- Kubernetes-style orchestration
- distributed scheduling
- strong container isolation
- untrusted multi-tenant execution
- database scale-to-zero
- container image management
- built-in DNS
- built-in TLS
- built-in CI/CD

Nimwake Core focuses on doing one thing reliably:

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

# Documentation

Detailed design documents live under [`.docs/`](.docs/README.md), split into two trees of small,
single-topic files:

```text
.docs/
├── README.md      # documentation index
├── prd/           # single source of truth: product, architecture, quality, roadmap
└── phases/        # execution plans: phase-1 … phase-4
```

- Requirements — what and why: [`.docs/prd/`](.docs/prd/README.md)
- Execution — when and in what order: [`.docs/phases/`](.docs/phases/README.md)
- Current work: [Phase 1 — Core MVP](.docs/phases/phase-1/README.md) targeting `v0.1.0`

The PRD is authoritative. Phase documents link to PRD sections instead of restating them.
