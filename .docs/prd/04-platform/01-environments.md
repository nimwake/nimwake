# Target Environments와 외부 의존성

> **Nimwake PRD** › 플랫폼
>
> 원문 구간: `PRD.md` §46–§48

---

## 46. Ingress Independence

Ingress는 Nimwake Core 외부 영역이다.

```text
                Internet / LAN / VPN
                         │
          ┌──────────────┼──────────────┐
          │              │              │
 Cloudflare Tunnel     Caddy       Port Forward
          │              │              │
          └──────────────┼──────────────┘
                         │
                         ▼
                      Nimwake
```

가능한 구성:

- Cloudflare Tunnel
- Caddy
- Nginx
- HAProxy
- Router Port Forwarding
- Reverse SSH
- VPS Relay
- Tailscale
- Tailscale Funnel
- LAN only
- Custom reverse proxy

Nimwake Core는 TLS certificate와 public DNS를 직접 책임지지 않는다.

---

## 47. External Dependencies

다음 서비스는 Nimwake Core 사용에 필수가 아니다.

```text
Cloudflare      Optional
Tailscale       Optional
GitHub          Optional
GitHub Actions  Optional
Docker          Optional / Future
Nginx           Optional
Caddy           Optional
```

Core가 요구하는 것은 기본적으로 다음뿐이다.

```text
Linux-like OS
Executable process
TCP port
HTTP request
Writable filesystem
```

---

## 48. Target Environments

### Tier 1

- Linux x86_64
- Linux ARM64

### Tier 2

- Android + Termux + Ubuntu proot
- Raspberry Pi
- Orange Pi
- ARM SBC

### 장기 후보

- WSL
- macOS development environment
- NAS Linux
- containerized Nimwake

Android는 제품 자체가 아니라 중요한 deployment target이다.

---

← [Core MVP Definition of Done](../03-delivery/08-definition-of-done.md) · [문서 인덱스](../README.md) · [Android Deployment와 검증](../04-platform/02-android.md) →
