# 보안

## 취약점 신고

**공개 이슈로 올리지 말아 주세요.**

[Security Advisories](https://github.com/nimwake/nimwake/security/advisories/new)로
비공개 신고를 받습니다. GitHub 계정만 있으면 됩니다.

확인에 며칠 걸릴 수 있습니다. 개인이 여가로 만드는 프로젝트라 SLA를 약속하지
않습니다.

## 먼저 알아야 할 것 — Core는 인증을 제공하지 않는다

이것을 모르고 배포하면 다른 모든 것이 의미가 없습니다.

```text
기본 listen   127.0.0.1:8080      loopback. 밖에서 닿지 않는다
--listen      0.0.0.0:8080        opt-in. 이 순간 LAN의 누구나 닿는다
```

**nimwaked를 인터넷에 직접 노출하지 마세요.** 인증도, TLS도, 속도 제한도
없습니다. 이것은 빠뜨린 것이 아니라 범위를 그은 것입니다
([§61](.docs/prd/05-beyond-mvp/02-platform-features.md)) — 노출은 그 일을 하는
것(터널, reverse proxy)에 맡기고 Core는 생애주기만 맡습니다.

노출할 때 **반드시** 앞에 두어야 하는 것:

| | |
| --- | --- |
| TLS 종료 | Core는 평문 HTTP만 말한다 |
| 인증 | Core는 요청자를 구분하지 않는다 |
| 속도 제한 | Core는 요청 수를 세지 않는다 |

## 지금 지켜지는 것

| | |
| --- | --- |
| 기본 비노출 | `127.0.0.1`. 노출은 `--listen`으로 명시해야 한다 |
| 관리 창구 없음 | 상태 조회는 `SIGUSR1`뿐 — **같은 uid를 요구한다.** 네트워크로 열린 관리 API가 없다 ([§79](.docs/prd/04-platform/03-host-interface.md)) |
| application 격리 | 각 application은 자기 process. gateway는 `127.0.0.1:<port>`로만 접속한다 |
| manifest 검증 | 기동 시 전부 검증한다. 하나라도 틀리면 **뜨지 않는다** ([§21](.docs/prd/02-architecture/05-manifest-and-readiness.md)) |
| `X-Forwarded-For` | gateway가 관측한 peer를 덧붙인다. **맨 오른쪽만 믿을 수 있다** ([§81](.docs/prd/02-architecture/06-gateway-and-requests.md)) |

## 아직 없는 것 — 노출 전에 아셔야 합니다

**의도적으로 비어 있고, 메우는 중입니다.**

| | |
| --- | --- |
| application별 접근 제어 | 한 daemon의 모든 app이 같은 listener를 공유한다. "이 app은 LAN만"을 Core가 강제하지 못한다 |
| 요청 본문 크기 상한 | 없다. 큰 본문이 그대로 backend까지 간다 |
| 요청 header 읽기 제한 시간 | 없다 (slowloris). 응답 header 대기는 [§75](.docs/prd/02-architecture/06-gateway-and-requests.md)로 있다 |

앞의 둘 때문에 **터널이나 reverse proxy 뒤에 두는 것이 선택이 아니라 요구사항**
입니다. Cloudflare Tunnel 같은 provider가 본문 크기와 느린 요청을 먼저 걸러 줍니다.

## 범위

신고를 받습니다:

- gateway가 요청을 잘못 라우팅하거나 app 경계를 넘기는 것
- process 생애주기로 권한이 올라가거나 process가 남는 것
- manifest 검증을 우회해 의도하지 않은 명령이 실행되는 것
- 로그나 `SIGUSR1` 보고로 비밀이 새는 것

범위가 아닙니다:

- `--listen 0.0.0.0`으로 열어 둔 것 자체 — 위에 적은 대로 의도된 동작입니다
- Core 앞에 둔 터널·proxy의 설정 실수
- `nimwaked`와 같은 uid로 이미 코드를 실행할 수 있는 공격자

## 지원 버전

`v0.1.0` 이전입니다. **최신 태그만** 고칩니다.
