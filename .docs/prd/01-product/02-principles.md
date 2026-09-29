# 제품 핵심 원칙

> **Nimwake PRD** › 제품 정의
>
> 원문 구간: `PRD.md` §5

---

## 5. 제품 핵심 원칙

### 5.1 Provider Agnostic

Cloudflare, GitHub, Tailscale 등에 종속되지 않는다.

Nimwake가 정상 동작하기 위해 특정 SaaS account가 존재할 필요가 없어야 한다.

---

### 5.2 Runtime Agnostic

다음과 같은 runtime을 동일한 모델로 취급한다.

```text
Node.js
Python
Rust
Go
Java
Bun
Deno
...
```

Nimwake와 애플리케이션 간의 계약은 단순하다.

> 지정된 command를 실행하면 설정된 address/port에서 HTTP application이 준비된다.

---

### 5.3 Build Agnostic

Nimwake Core는 application build를 책임지지 않는다.

```text
Source
  ↓
External Build
  ↓
Artifact
  ↓
Nimwake
```

빌드는 다음 어디에서든 수행 가능하다.

- GitHub Actions
- GitLab CI
- Jenkins
- Local machine
- Another build server
- Manual shell
- 기타 CI/CD 시스템

---

### 5.4 Low Overhead

Core infrastructure가 사용하는 RAM과 CPU를 최소화한다.

Nimwake 자체가 resource-constrained machine에서 가장 무거운 서비스가 되어서는 안 된다.

---

### 5.5 Scale-to-Zero First

application은 기본적으로 sleeping 상태를 허용한다.

```text
deployed != running
```

---

### 5.6 Single Machine First

초기 Nimwake는 distributed scheduler를 만들지 않는다.

```text
one machine
many applications
few running processes
```

를 효율적으로 처리하는 것을 우선한다.

---

### 5.7 Linux First, Android Friendly

Core는 Linux를 기준으로 설계한다.

Android는 Linux-compatible deployment target 중 하나로 취급한다.

```text
Android
   ↓
Termux
   ↓
Ubuntu / proot
   ↓
Nimwake
```

Android-specific lifecycle management는 Core에 포함하지 않는다.

---

← [제품 개요와 문제 정의](../01-product/01-overview.md) · [문서 인덱스](../README.md) · [Non-Goals와 Core MVP 범위](../01-product/03-scope.md) →
