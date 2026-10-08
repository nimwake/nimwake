# 프레임워크 앱 cold start 측정

> 재현 절차. 측정 결과는 [설계 결정 기록](../.docs/phases/phase-1/05-design-decisions.md)의
> "실측: 프레임워크 앱"에 있다.

PRD §45는 "경량 API 2초 / Next.js 5초"를 목표로 잡는다.
그 숫자가 현실적인지, `startup_timeout_seconds` 기본 5초가 맞는지 보기 위한 측정이다.

`examples/hello-node`는 `http.createServer` 하나여서 런타임 비용만 보여 준다.
프레임워크 비용은 별도로 재야 한다.

---

## 준비

node를 루트 없이 홈에 설치한다.

```bash
V=v22.12.0
curl -fsSL "https://nodejs.org/dist/$V/node-$V-linux-x64.tar.xz" -o /tmp/node.tar.xz
mkdir -p "$HOME/node" && tar -xJf /tmp/node.tar.xz -C "$HOME/node" --strip-components=1
export PATH="$HOME/node/bin:$PATH"
```

### Express

```bash
mkdir -p "$HOME/nwapps/express" && cd "$HOME/nwapps/express"
npm init -y >/dev/null && npm i express --silent
```

`server.js`는 `PORT`를 읽어 `127.0.0.1`에 bind하고 `SIGTERM`에 나간다.

### Next.js

```bash
mkdir -p "$HOME/nwapps/next" && cd "$HOME/nwapps/next"
npm i next react react-dom --silent
# next.config.js: module.exports = { output: "standalone" }
npx next build
cp -r .next/static .next/standalone/.next/
```

`output: standalone`을 쓰는 이유가 있다. `next start` CLI를 거치면 wrapper
process가 하나 더 끼고, 우리가 재려는 것은 서버 자체의 기동 비용이다.

---

## 측정

세 앱을 한 daemon에 등록하고 `--trace-json`으로 띄운다.
`[runtime.env]`로 `PORT`와 `HOSTNAME`을 넘긴다.

```toml
[runtime]
command = ["<node>", "server.js"]
working_dir = "<app>"

[runtime.env]
PORT = "3513"
HOSTNAME = "127.0.0.1"

[lifecycle]
startup_timeout_seconds = 60   # 먼저 실제 값을 알아야 한다
```

`RUST_LOG="info,nimwake::request=debug"`를 주면 요청 구간도 함께 나온다.
각 앱에 **두 번** 요청한다. 첫 번째가 cold, 두 번째가 warm이다.

기동 구간은 `nimwake::startup`의 `total_us`, 사용자가 겪는 시간은
`nimwake::request`의 `ttfb_us`다. **둘의 차이가 이 측정의 요점이다.**

RSS는 `/proc/<pid>/status`의 `VmRSS`를 본다.
Next.js는 요청을 받으며 더 오르므로 기동 직후와 몇 번 요청한 뒤를 함께 본다.

---

## 주의

- 이 절차는 **한 페이지짜리 앱**을 잰다. route가 많고 DB에 붙고 middleware가
  있는 실제 서비스는 더 느리다. 여기서 나온 수치는 하한이다.
- 부하 측정(`nimwake-bench`)과 동시에 돌리면 서로 오염된다.
