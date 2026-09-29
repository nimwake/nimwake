# Runtime Abstraction과 Process Ownership

> **Nimwake PRD** › 아키텍처
>
> 원문 구간: `PRD.md` §18–§20, §70

---

## 18. Runtime Abstraction

Nimwake Core는 실행 방식에 종속되지 않는다.

MVP interface:

```rust
trait Runtime {
    async fn start(
        &self,
        app: &AppSpec,
    ) -> Result<StartedProcess>;

    async fn stop(
        &self,
        process: &ProcessHandle,
        timeout: Duration,
    ) -> Result<()>;
}
```

초기 구현:

```text
ProcessRuntime
```

테스트용 구현:

```text
FakeRuntime
```

향후 확장 가능성:

```text
ContainerRuntime
DockerRuntime
PodmanRuntime
MicroVMRuntime
```

Runtime adapter가 추가되더라도 lifecycle semantics는 변경되지 않아야 한다.

---

## 19. Process Runtime

MVP의 기본 runtime은 raw process 방식이다.

```text
Nimwake
   │
   ├── node server.js
   ├── python -m uvicorn ...
   ├── ./go-api
   └── ./rust-api
```

Rust implementation은 기본적으로 `tokio::process`를 사용한다.

---

### stdout / stderr 처리

child를 `Stdio::piped()`로 실행하고 읽지 않으면 pipe buffer가 가득 차면서 **child가 멈출 수 있다.**

따라서 MVP는 다음 중 하나를 반드시 선택한다.

```text
inherit          daemon의 stdout/stderr를 그대로 사용
   또는
drain            pipe를 만들고 daemon이 지속적으로 읽어 버린다
```

**pipe를 생성해 두고 방치하는 구성은 허용하지 않는다.**

본격적인 log 수집/rotation은 Post-MVP 주제다. → [§60 Logging & Observability](../05-beyond-mvp/02-platform-features.md)

---

## 20. Process Isolation

Raw process runtime은 strong sandbox를 제공하지 않는다.

따라서 MVP는 다음 정책을 사용한다.

- trusted application만 실행
- app별 working directory 분리
- app별 environment / secret 분리
- internal application port는 loopback bind 권장
- public exposure는 Nimwake Gateway를 통해 수행
- hostile multi-tenant workload는 지원하지 않음

강한 isolation은 향후 cgroup / container / MicroVM runtime에서 다룬다.

---

## 70. Process Ownership

`tokio::process::Child`의 소유권은 **runtime 내부 task 하나만 가진다.**

AppManager와 monitor가 같은 `Child`를 동시에 다루는 구조는 허용하지 않는다.

```text
                 AppManager
                     │
                     │ ProcessHandle (id + control)
                     ▼
              ProcessController
                     │
                     ▼
              tokio::process::Child
                     │
                     ├── wait
                     ├── terminate
                     └── kill
```

외부에 노출되는 handle은 `Child`가 아니라 식별자와 제어 채널이다.

```rust
struct ProcessHandle {
    id: ProcessId,
    control: ProcessControl,
}

struct StartedProcess {
    handle: ProcessHandle,
    backend: Backend,
}
```

`ProcessController`의 책임:

| 책임                  | 설명                                                        |
| --------------------- | ----------------------------------------------------------- |
| `Child` 소유          | spawn된 child를 유일하게 소유한다                            |
| exit 감시             | `wait()` 결과를 `ProcessExited { process_id, status }`로 보고 |
| graceful termination  | SIGTERM → shutdown timeout → SIGKILL                         |
| stdout/stderr drain   | pipe를 사용할 경우 지속적으로 소비                           |
| process group 관리    | daemon 종료 시 group 단위 정리 ([§71](08-failure-handling.md)) |

이 결정은 `v0.0.2` 구현 이전에 확정되어야 한다.

---

← [Concurrency Model과 Cold Start](../02-architecture/03-concurrency.md) · [문서 인덱스](../README.md) · [Application Manifest와 Readiness](../02-architecture/05-manifest-and-readiness.md) →
