//! 명령줄 옵션.
//!
//! 요구사항: PRD §74 Host Interface.
//!
//! 옵션이 적어 직접 파싱한다. `clap`은 `nimwake` CLI가 생길 때 함께 들인다. (PRD §35)
//! **인자 형태는 계약이다.** supervisor가 여기에 의존하므로 바뀌면 PRD와 CHANGELOG에 남긴다.

use std::net::SocketAddr;
use std::path::PathBuf;

use nimwake_core::DEFAULT_GATEWAY_PORT;
use nimwake_core::{Limits, RequestLimits};

use crate::exit::Failure;

const DEFAULT_APPS_DIR: &str = "apps";

/// 기본 listen 주소를 정하는 환경 변수. 명령줄 인자가 우선한다.
const LISTEN_ENV: &str = "NIMWAKE_LISTEN";

pub const USAGE: &str = "\
nimwaked — Nimwake daemon

USAGE:
    nimwaked [APPS_DIR] [OPTIONS]

ARGS:
    <APPS_DIR>          manifest 디렉터리 (기본: apps)

OPTIONS:
    --apps <DIR>        APPS_DIR와 같다
    --listen <ADDR>     gateway listen 주소 (기본: 127.0.0.1:8080)
                        외부 노출은 opt-in이다. Core는 TLS도 인증도 제공하지 않는다
    --port <PORT>       listen 주소의 port만 바꾼다
                        0을 주면 비어 있는 port를 골라 실제 값을 출력한다
    --min-free-memory-mb <MB>
                        잠든 application을 깨우기 전에 있어야 하는 가용 메모리
                        기본 0은 검사하지 않는다. 부족하면 503으로 거절한다 (PRD §78)
                        돌고 있는 application은 내리지 않는다
    --header-read-timeout-seconds <SECONDS>
                        요청 header를 다 받기까지의 상한 (기본 15초)
                        0은 끈다. 느린 header는 Core의 연결을 붙잡는다 (PRD §83)
    --max-request-body-mb <MB>
                        요청 본문의 최대 크기. 기본 0은 검사하지 않는다
                        넘으면 413. gateway는 본문을 버퍼링하지 않으므로
                        이것은 application을 위한 방어다 (PRD §83)
    --control-socket <PATH>
                        기계가 읽는 상태 창구를 그 경로에 연다 (PRD §85)
                        없으면 만들지 않는다. 권한 0600, 같은 uid만 닿는다
                        디렉터리는 호스트가 만든다 — 없으면 기동하지 않는다
    --trace-json        구간 계측을 JSON 한 줄씩 출력한다 (benchmark mode, PRD §72)
                        요청 구간은 평소 꺼져 있고 이때만 켜진다
    -V, --version       버전만 출력하고 끝낸다 (daemon을 띄우지 않는다)
    -h, --help          이 도움말

SIGNALS:
    SIGINT, SIGTERM     정상 종료. 모든 application을 정지시킨 뒤 끝낸다
    SIGHUP              manifest 디렉터리를 다시 읽어 차이만 적용한다 (PRD §76)
                        검증 실패와 빈 집합은 거부하고 기존 상태를 유지한다
    SIGUSR1             지금 상태를 로그에 쓴다 (PRD §79)
                        버전 · app별 state · OS pid · 활성 요청 · 유휴 시간

ENVIRONMENT:
    NIMWAKE_LISTEN      기본 listen 주소. --listen / --port가 우선한다

EXIT CODES:
    0                   정상 종료
    78                  설정 오류 (재기동해도 나아지지 않는다)
    69                  listen 주소 bind 실패 (backoff 후 재시도 가능)
";

#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub apps_dir: PathBuf,
    pub listen: SocketAddr,
    pub trace_json: bool,
    /// daemon 수준 자원 한계. (PRD §78)
    pub limits: Limits,
    /// 요청을 받는 쪽의 상한. (PRD §83)
    pub request_limits: RequestLimits,
    /// 기계가 읽는 상태 창구의 경로. `None`이면 열지 않는다. (PRD §85)
    pub control_socket: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            apps_dir: PathBuf::from(DEFAULT_APPS_DIR),
            listen: SocketAddr::from(([127, 0, 0, 1], DEFAULT_GATEWAY_PORT)),
            trace_json: false,
            limits: Limits::default(),
            request_limits: RequestLimits::default(),
            control_socket: None,
        }
    }
}

/// 도움말을 요청받았을 때와 파싱에 실패했을 때를 구분한다.
#[derive(Debug)]
pub enum Parsed {
    Options(Box<Options>),
    Help,
    /// 버전만 알려 주고 끝낸다.
    ///
    /// 호스트가 **daemon을 띄우지 않고** 이 binary가 무엇인지 물을 수 있어야 한다.
    /// 모르는 인자는 설정 오류(`78`)이므로, 새 flag를 붙일지 결정하려면 먼저
    /// 버전을 알아야 한다. (PRD §74)
    Version,
}

/// 환경 변수를 기본값으로 삼고 명령줄 인자를 덮어쓴다.
///
/// # Errors
///
/// 인자가 잘못되면 설정 오류다. supervisor가 재기동해도 나아지지 않는다. (PRD §74)
pub fn parse<I>(args: I, listen_env: Option<&str>) -> Result<Parsed, Failure>
where
    I: IntoIterator<Item = String>,
{
    let mut options = Options::default();

    if let Some(raw) = listen_env {
        options.listen = listen_addr(raw, LISTEN_ENV)?;
    }

    let mut apps_dir_seen = false;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),

            "-V" | "--version" => return Ok(Parsed::Version),

            "--trace-json" => options.trace_json = true,

            "--apps" => {
                options.apps_dir = PathBuf::from(value(&mut args, "--apps")?);
                apps_dir_seen = true;
            }

            "--listen" => {
                options.listen = listen_addr(&value(&mut args, "--listen")?, "--listen")?;
            }

            // 0은 "검사하지 않음"이다. 기본으로 켜지 않는다. (PRD §78)
            "--min-free-memory-mb" => {
                let raw = value(&mut args, "--min-free-memory-mb")?;
                options.limits.min_free_memory_mb = raw.parse().map_err(|_| {
                    Failure::config(format!(
                        "`--min-free-memory-mb` expects a number, got `{raw}`"
                    ))
                })?;
            }

            // **기본값이 있는 유일한 상한이다.** (PRD §83)
            //
            // 정상 클라이언트가 header를 보내는 데 15초가 걸릴 이유가 없고,
            // 느린 header는 어느 app으로 갈지 모르는 채로 Core를 붙잡는다.
            // 0은 끈다 — 아주 느린 링크에서 운영자가 판단할 수 있다.
            "--header-read-timeout-seconds" => {
                let raw = value(&mut args, "--header-read-timeout-seconds")?;
                let seconds: u64 = raw.parse().map_err(|_| {
                    Failure::config(format!(
                        "`--header-read-timeout-seconds` expects a number, got `{raw}`"
                    ))
                })?;
                options.request_limits.header_read_timeout =
                    (seconds > 0).then(|| std::time::Duration::from_secs(seconds));
            }

            // 0은 "검사하지 않음"이다. 어떤 기본값이든 누군가의 업로드를
            // 깨뜨리므로 켜지 않는다. (PRD §83)
            "--max-request-body-mb" => {
                let raw = value(&mut args, "--max-request-body-mb")?;
                let megabytes: u64 = raw.parse().map_err(|_| {
                    Failure::config(format!(
                        "`--max-request-body-mb` expects a number, got `{raw}`"
                    ))
                })?;
                options.request_limits.max_request_body_bytes =
                    (megabytes > 0).then(|| megabytes * 1024 * 1024);
            }

            // 켜야 생긴다. 아무도 요구하지 않은 파일을 만들지 않는다. (PRD §85)
            "--control-socket" => {
                options.control_socket = Some(PathBuf::from(value(&mut args, "--control-socket")?));
            }

            // port만 바꾼다. `--listen 0.0.0.0:8080 --port 0`처럼 조합할 수 있다.
            "--port" => {
                let raw = value(&mut args, "--port")?;
                let port = raw.parse().map_err(|_| {
                    Failure::config(format!("`--port` expects a number, got `{raw}`"))
                })?;
                options.listen.set_port(port);
            }

            other if other.starts_with('-') => {
                return Err(Failure::config(format!("unknown option `{other}`")))
            }

            positional if apps_dir_seen => {
                return Err(Failure::config(format!(
                    "unexpected argument `{positional}`"
                )))
            }

            positional => {
                options.apps_dir = PathBuf::from(positional);
                apps_dir_seen = true;
            }
        }
    }

    Ok(Parsed::Options(Box::new(options)))
}

/// `host:port` 하나만 받는다. 이름 해석은 하지 않는다.
///
/// DNS를 끌어들이면 기동 시점에 네트워크에 의존하게 된다.
/// daemon이 어디에 붙을지는 의존 없이 결정되어야 한다.
fn listen_addr(raw: &str, source: &str) -> Result<SocketAddr, Failure> {
    raw.parse().map_err(|_| {
        Failure::config(format!(
            "`{source}` expects an address like `127.0.0.1:8080`, got `{raw}`"
        ))
    })
}

fn value<I: Iterator<Item = String>>(args: &mut I, flag: &str) -> Result<String, Failure> {
    args.next()
        .ok_or_else(|| Failure::config(format!("`{flag}` needs a value")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Options, Failure> {
        parse_with_env(args, None)
    }

    fn parse_with_env(args: &[&str], env: Option<&str>) -> Result<Options, Failure> {
        match parse(args.iter().map(|arg| (*arg).to_owned()), env)? {
            Parsed::Options(options) => Ok(*options),
            Parsed::Help => Err(Failure::config("help")),
            Parsed::Version => Err(Failure::config("version")),
        }
    }

    /// `--version`은 옵션을 돌려주지 않는다. (PRD §74)
    #[test]
    fn the_version_flag_short_circuits() {
        for flag in ["--version", "-V"] {
            let parsed = parse([flag.to_owned()], None).expect("the flag parses");
            assert!(
                matches!(parsed, Parsed::Version),
                "`{flag}`는 Version이어야 한다",
            );
        }
    }

    /// 다른 인자와 섞여도 버전이 먼저다. manifest를 읽지 않는다.
    #[test]
    fn the_version_flag_wins_over_other_arguments() {
        let parsed = parse(
            [
                "--apps".to_owned(),
                "/nonexistent".to_owned(),
                "--version".to_owned(),
            ],
            None,
        )
        .expect("the flag parses");
        assert!(matches!(parsed, Parsed::Version));
    }

    /// 인자가 없으면 §83의 기본값이 그대로 온다. (PRD §83)
    /// 켜야 생긴다. 기본은 없음이다. (PRD §85)
    #[test]
    fn the_control_socket_is_off_by_default() {
        assert_eq!(parse_args(&[]).expect("valid").control_socket, None);
    }

    /// 경로를 그대로 받는다.
    #[test]
    fn the_control_socket_path_is_taken_as_given() {
        let options = parse_args(&["--control-socket", "/run/nimwake/x.sock"]).expect("valid");
        assert_eq!(
            options.control_socket,
            Some(PathBuf::from("/run/nimwake/x.sock")),
        );
    }

    /// 값이 없으면 설정 오류다.
    #[test]
    fn the_control_socket_requires_a_value() {
        assert!(parse_args(&["--control-socket"]).is_err());
    }

    #[test]
    fn request_limits_keep_their_defaults() {
        let options = parse_args(&[]).expect("no arguments is valid");
        assert_eq!(options.request_limits, RequestLimits::default());
        assert!(
            options.request_limits.header_read_timeout.is_some(),
            "header 상한은 기본으로 켜져 있다",
        );
        assert!(options.request_limits.max_request_body_bytes.is_none());
    }

    /// `0`은 header 상한을 끈다. (PRD §83)
    #[test]
    fn a_zero_header_timeout_turns_it_off() {
        let options = parse_args(&["--header-read-timeout-seconds", "0"]).expect("valid");
        assert_eq!(options.request_limits.header_read_timeout, None);

        let options = parse_args(&["--header-read-timeout-seconds", "30"]).expect("valid");
        assert_eq!(
            options.request_limits.header_read_timeout,
            Some(std::time::Duration::from_secs(30)),
        );
    }

    /// MB를 바이트로 바꾼다. `0`은 끈다. (PRD §83)
    #[test]
    fn the_body_limit_is_read_in_megabytes() {
        let options = parse_args(&["--max-request-body-mb", "2"]).expect("valid");
        assert_eq!(
            options.request_limits.max_request_body_bytes,
            Some(2 * 1024 * 1024),
        );

        let options = parse_args(&["--max-request-body-mb", "0"]).expect("valid");
        assert_eq!(options.request_limits.max_request_body_bytes, None);
    }

    /// 숫자가 아니면 설정 오류다.
    #[test]
    fn non_numeric_limits_are_rejected() {
        assert!(parse_args(&["--header-read-timeout-seconds", "soon"]).is_err());
        assert!(parse_args(&["--max-request-body-mb", "big"]).is_err());
    }

    #[test]
    fn defaults_to_loopback() {
        let options = parse_args(&[]).expect("no arguments is valid");
        assert_eq!(options.apps_dir, PathBuf::from("apps"));
        assert_eq!(options.listen.ip().to_string(), "127.0.0.1");
        assert_eq!(options.listen.port(), DEFAULT_GATEWAY_PORT);
        assert!(!options.trace_json);
    }

    #[test]
    fn accepts_the_apps_directory_as_a_positional_argument() {
        let options = parse_args(&["examples"]).expect("positional apps dir");
        assert_eq!(options.apps_dir, PathBuf::from("examples"));
    }

    #[test]
    fn opens_to_the_lan_only_when_asked() {
        let options = parse_args(&["--listen", "0.0.0.0:9000"]).expect("listen flag");
        assert_eq!(options.listen.to_string(), "0.0.0.0:9000");
    }

    #[test]
    fn the_environment_sets_the_default_listen_address() {
        let options = parse_with_env(&[], Some("0.0.0.0:9100")).expect("env listen");
        assert_eq!(options.listen.to_string(), "0.0.0.0:9100");
    }

    /// 명령줄이 환경 변수를 이긴다. (PRD §74)
    #[test]
    fn the_command_line_wins_over_the_environment() {
        let options =
            parse_with_env(&["--listen", "127.0.0.1:1234"], Some("0.0.0.0:9100")).expect("both");
        assert_eq!(options.listen.to_string(), "127.0.0.1:1234");
    }

    /// `--port`는 주소의 port만 바꾼다. 조합할 수 있어야 한다.
    #[test]
    fn port_overrides_only_the_port() {
        let options = parse_args(&["--listen", "0.0.0.0:8080", "--port", "0"]).expect("both flags");
        assert_eq!(options.listen.to_string(), "0.0.0.0:0");
    }

    #[test]
    fn rejects_a_missing_value() {
        assert_eq!(
            parse_args(&["--port"]).unwrap_err().code(),
            exitcode_config()
        );
    }

    #[test]
    fn rejects_a_port_that_is_not_a_number() {
        assert!(parse_args(&["--port", "http"]).is_err());
    }

    #[test]
    fn rejects_a_listen_address_without_a_port() {
        assert!(parse_args(&["--listen", "0.0.0.0"]).is_err());
    }

    /// 이름 해석은 하지 않는다. 기동이 네트워크에 의존하면 안 된다.
    #[test]
    fn rejects_a_hostname() {
        assert!(parse_args(&["--listen", "localhost:8080"]).is_err());
    }

    #[test]
    fn rejects_an_unknown_option() {
        assert!(parse_args(&["--verbose"]).is_err());
    }

    #[test]
    fn rejects_a_second_positional_argument() {
        assert!(parse_args(&["one", "two"]).is_err());
    }

    /// 잘못된 인자는 재기동해도 나아지지 않는다. (PRD §74)
    #[test]
    fn a_bad_argument_is_a_configuration_failure() {
        assert_eq!(
            parse_args(&["--verbose"]).unwrap_err().code(),
            exitcode_config()
        );
    }

    #[test]
    fn asks_for_help() {
        assert!(matches!(
            parse(["--help".to_owned()], None).expect("help parses"),
            Parsed::Help
        ));
    }

    const fn exitcode_config() -> u8 {
        78
    }
}
