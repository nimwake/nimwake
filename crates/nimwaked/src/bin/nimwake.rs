//! `nimwaked`에게 묻는 명령. (PRD §86)
//!
//! §85의 control socket에 요청 한 줄을 보내고 응답 한 줄을 받는다. daemon을
//! 띄우지 않는다 — 그것은 `nimwaked`의 일이다.
//!
//! 의존성이 없다. 보내는 것이 고정된 한 줄이고 받는 것이 평평한 객체라,
//! 표준 라이브러리로 충분하다. 이 workspace의 방침이기도 하다.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

/// 환경 변수로도 경로를 받는다. 호스트가 한 번 export하면 운영자가 외우지
/// 않아도 된다. (PRD §86)
const SOCKET_ENV: &str = "NIMWAKE_SOCKET";

/// 응답을 기다리는 상한. daemon이 답하지 않으면 영원히 매달리지 않는다.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

const USAGE: &str = "\
nimwake — nimwaked에게 묻는다 (PRD §86)

USAGE:
    nimwake [OPTIONS] <COMMAND>

COMMANDS:
    status              지금 무엇이 어떤 상태인가

OPTIONS:
    --socket <PATH>     control socket의 경로
                        NIMWAKE_SOCKET 환경 변수로도 줄 수 있다 (플래그가 이긴다)
                        기본값을 두지 않는다 — 경로를 아는 것은 호스트다
    --json              daemon의 응답을 그대로 낸다 (PRD §85의 형식)
    -V, --version       버전만 출력하고 끝낸다
    -h, --help          이 도움말

EXIT CODES:
    0   답을 받았다
    69  창구에 닿지 못했다 — daemon이 꺼졌거나 경로가 틀렸다
    70  daemon이 오류를 돌려줬다
    78  인자가 틀렸다

daemon을 띄우는 것은 `nimwaked`다. 이 명령은 묻기만 한다.
";

/// `nimwaked`의 코드와 같은 뜻을 쓴다. (PRD §86)
///
/// **69와 78을 가른다.** 스크립트가 "아직 안 떴다"(기다린다)와 "설정이
/// 틀렸다"(고친다)를 다르게 다뤄야 한다. 둘을 같은 코드로 내면 재시도 루프가
/// 영원히 돈다.
const EX_UNAVAILABLE: u8 = 69;
const EX_SOFTWARE: u8 = 70;
const EX_CONFIG: u8 = 78;

fn main() -> ExitCode {
    let mut socket: Option<PathBuf> = std::env::var_os(SOCKET_ENV).map(PathBuf::from);
    let mut json = false;
    let mut command: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("nimwake {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--json" => json = true,
            // 플래그가 환경 변수를 이긴다 — 한 번만 다른 곳에 묻고 싶은 경우가 있다.
            "--socket" => match args.next() {
                Some(path) => socket = Some(PathBuf::from(path)),
                None => return fail(EX_CONFIG, "`--socket` expects a path"),
            },
            other if other.starts_with('-') => {
                return fail(EX_CONFIG, &format!("unknown option `{other}`"));
            }
            other => match command {
                None => command = Some(other.to_owned()),
                Some(first) => {
                    return fail(
                        EX_CONFIG,
                        &format!("unexpected argument `{other}` after `{first}`"),
                    );
                }
            },
        }
    }

    let Some(command) = command else {
        eprint!("{USAGE}");
        return ExitCode::from(EX_CONFIG);
    };
    if command != "status" {
        return fail(EX_CONFIG, &format!("unknown command `{command}`"));
    }

    // **경로를 추측하지 않는다.** (PRD §86)
    //
    // CLI가 `/run/nimwake.sock`을 먼저 들여다보면, 그 경로에 다른 것이 있는
    // 기계에서 엉뚱한 곳에 묻는다.
    let Some(socket) = socket else {
        return fail(
            EX_CONFIG,
            &format!("no control socket; pass `--socket <PATH>` or set {SOCKET_ENV}"),
        );
    };

    match ask(&socket, r#"{"command":"status"}"#) {
        Err(error) => fail(
            EX_UNAVAILABLE,
            &format!("cannot reach {}: {error}", socket.display()),
        ),
        Ok(reply) => report(&reply, json),
    }
}

/// 요청 한 줄 → 응답 한 줄. 연결은 응답을 받고 닫는다. (PRD §85)
fn ask(socket: &std::path::Path, request: &str) -> std::io::Result<String> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(REPLY_TIMEOUT))?;
    stream.set_write_timeout(Some(REPLY_TIMEOUT))?;
    writeln!(stream, "{request}")?;
    stream.flush()?;

    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    Ok(reply)
}

/// 받은 응답을 낸다. `--json`이면 **가공하지 않는다.** (PRD §86)
fn report(reply: &str, json: bool) -> ExitCode {
    let reply = reply.trim();

    // daemon이 오류를 돌려준 경우. 닿기는 했으므로 69가 아니다.
    //
    // **`apps`가 있으면 상태 응답이다.** app 객체마다 자기 `error` 필드가 있어서
    // 응답 전체에서 `"error"`를 찾으면 app 하나가 FAILED일 때 daemon 오류로
    // 오인한다 — 운영자가 상태를 가장 보고 싶을 때 CLI가 실패했다.
    //
    // 오류 응답은 `{"contract":1,"error":"…"}`이고 `apps`가 없다 (PRD §85).
    let is_status = reply.contains(r#""apps":["#);
    if !is_status {
        if let Some(message) = field(reply, "error") {
            eprintln!("nimwake: {message}");
            return ExitCode::from(EX_SOFTWARE);
        }
    }

    if json {
        // §85의 응답을 그대로 낸다. CLI가 다시 포맷하면 계약이 두 곳이 된다.
        println!("{reply}");
        return ExitCode::SUCCESS;
    }

    let version = field(reply, "version").unwrap_or_else(|| "unknown".to_owned());
    println!("nimwaked {version}");

    let apps = objects(reply);
    if apps.is_empty() {
        println!("no applications");
        return ExitCode::SUCCESS;
    }

    // 열 구성을 약속하지 않는다. 파싱할 것이 필요하면 `--json`이다. (PRD §86)
    println!(
        "{:<16} {:<10} {:>8} {:>7} {:>8}",
        "APP", "STATE", "PID", "ACTIVE", "IDLE"
    );
    for app in apps {
        println!(
            "{:<16} {:<10} {:>8} {:>7} {:>8}",
            field(&app, "name").unwrap_or_else(|| "?".to_owned()),
            field(&app, "state").unwrap_or_else(|| "?".to_owned()),
            number(&app, "pid").map_or_else(|| "—".to_owned(), |pid| pid.to_string()),
            number(&app, "active").unwrap_or(0),
            number(&app, "idle_ms").map_or_else(|| "—".to_owned(), format_millis),
        );
    }
    ExitCode::SUCCESS
}

fn fail(code: u8, message: &str) -> ExitCode {
    eprintln!("nimwake: {message}");
    ExitCode::from(code)
}

/// `"key":"value"`에서 value를 꺼낸다. escape를 되돌린다.
fn field(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let start = json.find(&needle)? + needle.len();
    let mut out = String::new();
    let mut escaped = false;
    for ch in json[start..].chars() {
        if escaped {
            out.push(match ch {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(out);
        } else {
            out.push(ch);
        }
    }
    None
}

/// `"key":123`에서 숫자를 꺼낸다. `null`이면 `None`.
fn number(json: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{key}\":");
    let start = json.find(&needle)? + needle.len();
    let digits: String = json[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// `apps` 배열의 객체들을 문자열로 가른다.
///
/// 중첩이 한 단계뿐이라 깊이를 세는 것으로 충분하다. 문자열 안의 중괄호는
/// 따옴표 상태를 보고 건너뛴다.
fn objects(json: &str) -> Vec<String> {
    let Some(start) = json.find("\"apps\":[") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut depth = 0_usize;
    let mut current = String::new();
    let mut in_string = false;
    let mut escaped = false;

    for ch in json[start + "\"apps\":[".len()..].chars() {
        if in_string {
            current.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                current.push(ch);
            }
            '{' => {
                depth += 1;
                current.push(ch);
            }
            '}' => {
                current.push(ch);
                depth -= 1;
                if depth == 0 {
                    out.push(std::mem::take(&mut current));
                }
            }
            ']' if depth == 0 => break,
            _ if depth > 0 => current.push(ch),
            _ => {}
        }
    }
    out
}

/// 사람이 읽는 시간. 밀리초는 너무 길고 초는 너무 거칠다.
fn format_millis(millis: u64) -> String {
    if millis < 1_000 {
        return format!("{millis}ms");
    }
    let seconds = millis / 1_000;
    if seconds < 60 {
        return format!("{}.{}s", seconds, (millis % 1_000) / 100);
    }
    if seconds < 3_600 {
        return format!("{}m{}s", seconds / 60, seconds % 60);
    }
    format!("{}h{}m", seconds / 3_600, (seconds % 3_600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = r#"{"contract":1,"version":"0.1.1","apps":[{"name":"blog","host":"blog.local","state":"RUNNING","active":2,"pending":0,"pid":12971,"since_ms":4120,"idle_ms":3100,"error":null},{"name":"notes","host":"notes.local","state":"SLEEPING","active":0,"pending":0,"pid":null,"since_ms":null,"idle_ms":null,"error":null}]}"#;

    #[test]
    fn it_reads_string_fields() {
        assert_eq!(field(REPLY, "version"), Some("0.1.1".to_owned()));
        assert_eq!(field(REPLY, "nothing"), None);
    }

    #[test]
    fn it_reads_numbers_and_nulls() {
        let apps = objects(REPLY);
        assert_eq!(apps.len(), 2, "{apps:?}");
        assert_eq!(number(&apps[0], "pid"), Some(12971));
        assert_eq!(number(&apps[0], "active"), Some(2));
        // `null`은 숫자가 아니다. 사람이 읽는 표에서 `—`가 된다.
        assert_eq!(number(&apps[1], "pid"), None);
    }

    /// app 객체를 정확히 가른다. (PRD §86)
    #[test]
    fn it_splits_the_app_array() {
        let apps = objects(REPLY);
        assert!(apps[0].contains(r#""name":"blog""#), "{}", apps[0]);
        assert!(apps[1].contains(r#""name":"notes""#), "{}", apps[1]);
        assert!(!apps[0].contains("notes"), "객체가 섞였다: {}", apps[0],);
    }

    /// **이름에 중괄호나 따옴표가 있어도 가르기가 깨지지 않는다.**
    ///
    /// app 이름은 manifest가 정하고 사람이 쓴다. 괄호가 들어올 수 있다.
    #[test]
    fn braces_inside_strings_do_not_break_splitting() {
        let reply = r#"{"contract":1,"version":"0.1.1","apps":[{"name":"we{ird}","state":"RUNNING","active":0,"pid":7},{"name":"ok","state":"SLEEPING","active":0,"pid":null}]}"#;
        let apps = objects(reply);
        assert_eq!(apps.len(), 2, "{apps:?}");
        assert_eq!(field(&apps[0], "name"), Some("we{ird}".to_owned()));
        assert_eq!(field(&apps[1], "name"), Some("ok".to_owned()));
    }

    /// escape된 따옴표를 되돌린다.
    #[test]
    fn it_unescapes_values() {
        let reply = r#"{"apps":[{"name":"say \"hi\"","state":"FAILED"}]}"#;
        let apps = objects(reply);
        assert_eq!(apps.len(), 1, "{apps:?}");
        assert_eq!(field(&apps[0], "name"), Some(r#"say "hi""#.to_owned()));
    }

    /// `apps`가 없으면 빈 목록이다. panic하지 않는다.
    #[test]
    fn a_reply_without_apps_is_empty() {
        assert!(objects(r#"{"contract":1,"error":"nope"}"#).is_empty());
        assert!(objects("").is_empty());
    }

    /// 사람이 읽는 시간 — 단위가 바뀌는 경계.
    #[test]
    fn it_formats_durations_for_people() {
        assert_eq!(format_millis(0), "0ms");
        assert_eq!(format_millis(999), "999ms");
        assert_eq!(format_millis(1_000), "1.0s");
        assert_eq!(format_millis(3_100), "3.1s");
        assert_eq!(format_millis(59_999), "59.9s");
        assert_eq!(format_millis(60_000), "1m0s");
        assert_eq!(format_millis(3_599_000), "59m59s");
        assert_eq!(format_millis(3_600_000), "1h0m");
    }
}
