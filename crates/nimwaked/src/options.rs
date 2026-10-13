//! 명령줄 옵션.
//!
//! 요구사항: PRD §74 Host Interface.
//!
//! 옵션이 적어 직접 파싱한다. `clap`은 `nimwake` CLI가 생길 때 함께 들인다. (PRD §35)
//! **인자 형태는 계약이다.** supervisor가 여기에 의존하므로 바뀌면 PRD와 CHANGELOG에 남긴다.

use std::net::SocketAddr;
use std::path::PathBuf;

use nimwake_core::Limits;
use nimwake_core::DEFAULT_GATEWAY_PORT;

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
    --trace-json        구간 계측을 JSON 한 줄씩 출력한다 (benchmark mode, PRD §72)
                        요청 구간은 평소 꺼져 있고 이때만 켜진다
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
}

impl Default for Options {
    fn default() -> Self {
        Self {
            apps_dir: PathBuf::from(DEFAULT_APPS_DIR),
            listen: SocketAddr::from(([127, 0, 0, 1], DEFAULT_GATEWAY_PORT)),
            trace_json: false,
            limits: Limits::default(),
        }
    }
}

/// 도움말을 요청받았을 때와 파싱에 실패했을 때를 구분한다.
#[derive(Debug)]
pub enum Parsed {
    Options(Box<Options>),
    Help,
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
        }
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
