//! 기계가 읽는 상태 창구. (PRD §85)
//!
//! §79의 `SIGUSR1`은 **사람이 읽는** 로그를 낸다. 그리고 §79 자신이 "기계가 읽는
//! 형식을 약속하지 않는다"고 적혀 있다. 그런데 `nimwake-android`가 그것을
//! 파싱하고 있었다 — 신호를 보내고 로그를 거슬러 찾는다.
//!
//! 이 모듈은 **약속하는 창구**다. 둘은 같은 사실을 다른 독자에게 준다.
//!
//! 신호를 없애지 않는다. socket을 켜지 않은 배포에서도 상태를 볼 길이 있어야 한다.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, info, warn};

use crate::AppRegistry;

/// 이 형식의 번호. (PRD §85)
///
/// **다른 저장소가 이 창구에 의존한다.** 필드를 없애거나 뜻을 바꾸면 올린다.
/// 모르는 필드를 더하는 것은 올리지 않는다 — 읽는 쪽이 무시하면 된다.
pub const CONTRACT: u32 = 1;

/// 요청 한 줄의 상한. 긴 입력이 메모리를 먹지 않게 한다. (PRD §85)
const MAX_REQUEST_BYTES: u64 = 8 * 1024;

/// 요청 한 줄을 받기까지의 상한. (PRD §85)
///
/// **크기 상한만으로는 모자라다.** 연결해 놓고 아무것도 보내지 않거나 한 글자씩
/// 보내는 쪽이 있으면 그 task가 영원히 매달린다 — §83이 gateway에서 막은 것과
/// 같은 모양이다. 창구에도 같은 상한이 필요하다.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// socket을 열고 닫을 때까지 들고 있는다.
///
/// `Drop`에서 파일을 지운다. 정상 종료에서 남기지 않는다.
#[derive(Debug)]
pub struct ControlSocket {
    listener: UnixListener,
    path: PathBuf,
}

impl ControlSocket {
    /// 경로에 socket을 연다. (PRD §85)
    ///
    /// # Errors
    ///
    /// 디렉터리가 없거나, 다른 daemon이 이미 그 경로에서 듣고 있거나, bind가
    /// 실패하면 오류다. 호출자는 기동을 거부해야 한다 — 창구가 있다고 믿는
    /// 쪽이 조용히 실패하는 것보다 낫다.
    pub async fn bind(path: &Path) -> io::Result<Self> {
        // **디렉터리를 만들어 주지 않는다.** (PRD §85)
        //
        // 만들면 그 권한을 Core가 정하게 되고, 그것이 socket의 0600보다 느슨하면
        // 0600이 의미를 잃는다. 경로를 아는 쪽이 만든다.
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("`{}` has no parent directory", path.display()),
            )
        })?;
        if !parent.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "control socket directory `{}` does not exist; the host creates it",
                    parent.display()
                ),
            ));
        }

        clear_stale_socket(path).await?;

        let listener = UnixListener::bind(path)?;
        restrict_permissions(path)?;

        info!(path = %path.display(), contract = CONTRACT, "control socket is listening");
        Ok(Self {
            listener,
            path: path.to_path_buf(),
        })
    }

    /// 연결을 받아 응답한다. **돌아오지 않는다** — 호출자가 abort한다.
    ///
    /// §79의 보고 작업, §80의 eviction 작업과 같은 모양이다. abort하면 `self`가
    /// drop되고 `Drop`이 socket 파일을 지운다.
    pub async fn serve(self, registry: Arc<AppRegistry>) {
        loop {
            let stream = match self.listener.accept().await {
                Ok((stream, _)) => stream,
                // accept 실패 하나로 창구 전체를 닫지 않는다. gateway와 같다.
                Err(error) => {
                    warn!(%error, "failed to accept a control connection");
                    continue;
                }
            };
            let registry = Arc::clone(&registry);
            tokio::spawn(async move {
                if let Err(error) = handle(stream, &registry).await {
                    debug!(%error, "control connection ended");
                }
            });
        }
    }
}

impl Drop for ControlSocket {
    fn drop(&mut self) {
        // 지우지 못해도 할 수 있는 것이 없다. 다음 기동이 stale로 보고 치운다.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// 남은 socket 파일을 치운다 — **살아 있는 daemon의 것은 건드리지 않는다.**
///
/// 무조건 지우면 돌고 있는 daemon의 창구를 빼앗아, 두 daemon이 같은 app을
/// 관리하는 상태로 간다. Invariant 1이 금지하는 것이다. (PRD §85)
async fn clear_stale_socket(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }

    match UnixStream::connect(path).await {
        // 누가 듣고 있다. 우리 자리가 아니다.
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "another daemon is already listening on `{}`",
                path.display()
            ),
        )),
        // 아무도 없다. 남은 파일이다.
        Err(_) => {
            std::fs::remove_file(path)?;
            debug!(path = %path.display(), "removed a stale control socket");
            Ok(())
        }
    }
}

/// `0600` — 같은 uid만 닿는다. (PRD §85)
fn restrict_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// 연결 하나: 요청 한 줄 → 응답 한 줄 → 닫음. (PRD §85)
async fn handle(stream: UnixStream, registry: &AppRegistry) -> io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut line = String::new();
    // 크기와 시간에 모두 상한을 건다. (PRD §85)
    //
    // 시간 상한이 없으면 연결해 놓고 보내지 않는 쪽이 task를 영원히 붙잡는다.
    let read = tokio::time::timeout(
        REQUEST_READ_TIMEOUT,
        BufReader::new(reader.take(MAX_REQUEST_BYTES)).read_line(&mut line),
    )
    .await;

    match read {
        Ok(result) => {
            result?;
        }
        // 상한을 넘겼다. 오류를 돌려주고 닫는다 — 끊기만 하면 왜인지 알 수 없다.
        Err(_) => {
            let response = error_response(&format!("no request within {REQUEST_READ_TIMEOUT:?}"));
            writer.write_all(response.as_bytes()).await?;
            writer
                .write_all(
                    b"
",
                )
                .await?;
            return writer.flush().await;
        }
    }

    let response = respond(&line, registry).await;
    writer.write_all(response.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

/// 요청 한 줄에 대한 응답 한 줄.
///
/// **오류여도 연결을 끊지 않는다.** 끊으면 읽는 쪽이 "daemon이 죽었나"와 "명령을
/// 모르나"를 구분할 수 없다. (PRD §85)
async fn respond(line: &str, registry: &AppRegistry) -> String {
    let Some(command) = parse_command(line) else {
        return error_response("request must be a JSON object with a `command` string");
    };

    match command.as_str() {
        "status" => status_response(registry).await,
        other => error_response(&format!("unknown command: {}", escape(other))),
    }
}

/// `{"command":"status"}`에서 `status`를 꺼낸다.
///
/// 의존성을 들이지 않는다. 이 workspace는 기반 설비뿐이고 유틸리티 crate가 없다.
/// 찾는 것이 문자열 필드 하나라 직접 읽는 편이 serde_json을 들이는 것보다 작다.
fn parse_command(line: &str) -> Option<String> {
    let line = line.trim();
    if !line.starts_with('{') {
        return None;
    }
    let key = line.find("\"command\"")?;
    let rest = &line[key + "\"command\"".len()..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    let mut chars = after.chars();
    if chars.next()? != '"' {
        return None;
    }

    let mut value = String::new();
    let mut escaped = false;
    for ch in chars {
        if escaped {
            value.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(value);
        } else {
            value.push(ch);
        }
    }
    None
}

async fn status_response(registry: &AppRegistry) -> String {
    let mut apps = Vec::new();
    for handle in registry.apps() {
        let host = handle.host().to_owned();
        match handle.status().await {
            Ok(status) => apps.push(app_object(&host, &status)),
            // actor가 내려갔으면 그 사실이 상태다. 전체를 멈추지 않는다. (§79와 같다)
            Err(error) => apps.push(format!(
                r#"{{"name":"{}","host":"{}","state":"unreachable","error":"{}"}}"#,
                escape(handle.name()),
                escape(&host),
                escape(&error.to_string()),
            )),
        }
    }

    format!(
        r#"{{"contract":{CONTRACT},"version":"{}","apps":[{}]}}"#,
        escape(env!("CARGO_PKG_VERSION")),
        apps.join(","),
    )
}

fn app_object(host: &str, status: &crate::app::AppStatus) -> String {
    let mut fields = vec![
        format!(r#""name":"{}""#, escape(&status.name)),
        format!(r#""host":"{}""#, escape(host)),
        format!(r#""state":"{}""#, escape(&status.state.to_string())),
        format!(r#""active":{}"#, status.active_requests),
        format!(r#""pending":{}"#, status.pending_waiters),
    ];

    // **`pid`는 OS pid다.** 내부 식별자가 아니라 `ps`와 맞출 수 있는 값이어야
    // 한다 — §79에서 한 번 틀렸던 것이다. (PRD §85)
    fields.push(match status.os_pid {
        Some(pid) => format!(r#""pid":{pid}"#),
        None => r#""pid":null"#.to_owned(),
    });
    fields.push(match status.since {
        Some(since) => format!(r#""since_ms":{}"#, since.as_millis()),
        None => r#""since_ms":null"#.to_owned(),
    });
    fields.push(match status.idle_for {
        Some(idle) => format!(r#""idle_ms":{}"#, idle.as_millis()),
        None => r#""idle_ms":null"#.to_owned(),
    });
    fields.push(match &status.last_error {
        Some(error) => format!(r#""error":"{}""#, escape(&error.to_string())),
        None => r#""error":null"#.to_owned(),
    });

    format!("{{{}}}", fields.join(","))
}

fn error_response(message: &str) -> String {
    format!(r#"{{"contract":{CONTRACT},"error":"{}"}}"#, escape(message))
}

/// JSON 문자열 안에서 쓸 수 없는 문자를 바꾼다.
///
/// app 이름과 오류 메시지가 여기로 들어온다. 따옴표 하나가 섞이면 읽는 쪽의
/// 파서가 깨지므로, 만드는 쪽에서 막는다.
fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // 제어 문자는 JSON에서 그대로 쓸 수 없다.
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `{"command":"status"}`를 읽는다.
    #[test]
    fn it_parses_a_command() {
        assert_eq!(
            parse_command(r#"{"command":"status"}"#),
            Some("status".to_owned()),
        );
        // 공백과 다른 필드가 섞여도 읽는다.
        assert_eq!(
            parse_command(r#"  { "other": 1, "command" : "status" }  "#),
            Some("status".to_owned()),
        );
    }

    /// JSON이 아니거나 `command`가 없으면 `None`.
    #[test]
    fn it_rejects_malformed_requests() {
        for raw in [
            "",
            "status",
            "{}",
            r#"{"cmd":"status"}"#,
            r#"{"command":1}"#,
            r#"{"command":"unterminated"#,
        ] {
            assert!(
                parse_command(raw).is_none(),
                "`{raw}`는 명령이 아니다: {:?}",
                parse_command(raw),
            );
        }
    }

    /// **따옴표가 섞인 값이 응답을 깨뜨리지 않는다.** (PRD §85)
    ///
    /// app 이름과 오류 메시지가 그대로 JSON에 들어간다. 만드는 쪽에서 막지
    /// 않으면 읽는 쪽의 파서가 깨진다.
    #[test]
    fn it_escapes_values() {
        assert_eq!(escape(r#"a"b"#), r#"a\"b"#);
        assert_eq!(escape(r"a\b"), r"a\\b");
        assert_eq!(escape("a\nb"), r"a\nb");
        assert_eq!(escape("a\u{1}b"), r"a\u0001b");
        assert_eq!(escape("정상"), "정상");
    }

    /// 모르는 명령은 오류를 돌려준다. 연결을 끊지 않는다. (PRD §85)
    #[test]
    fn an_unknown_command_is_an_error_object() {
        let response = error_response("unknown command: nonsense");
        assert!(response.contains(r#""contract":1"#), "{response}");
        assert!(response.contains(r#""error""#), "{response}");
    }

    /// 오류 메시지에 섞인 따옴표도 escape된다.
    #[test]
    fn an_error_message_is_escaped() {
        let response = error_response(r#"unknown command: say "hi""#);
        assert!(response.contains(r#"say \"hi\""#), "{response}");
        // 따옴표 개수가 짝이 맞아야 파싱된다.
        assert_eq!(
            response.matches(r#"\""#).count(),
            2,
            "escape된 따옴표가 둘이어야 한다: {response}",
        );
    }
}
