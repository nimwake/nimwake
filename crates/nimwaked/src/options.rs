//! 명령줄 옵션.
//!
//! 옵션이 둘뿐이라 직접 파싱한다. `clap`은 `nimwake` CLI가 생길 때 함께 들인다. (PRD §35)

use std::path::PathBuf;

use nimwake_core::DEFAULT_GATEWAY_PORT;

const DEFAULT_APPS_DIR: &str = "apps";

pub const USAGE: &str = "\
nimwaked — Nimwake daemon

USAGE:
    nimwaked [APPS_DIR] [OPTIONS]

ARGS:
    <APPS_DIR>          manifest 디렉터리 (기본: apps)

OPTIONS:
    --apps <DIR>        APPS_DIR와 같다
    --port <PORT>       gateway listen port (기본: 8080)
                        0을 주면 비어 있는 port를 골라 실제 값을 출력한다
    --trace-json        구간 계측을 JSON 한 줄씩 출력한다 (benchmark mode, PRD §72)
    -h, --help          이 도움말
";

#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub apps_dir: PathBuf,
    pub port: u16,
    pub trace_json: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            apps_dir: PathBuf::from(DEFAULT_APPS_DIR),
            port: DEFAULT_GATEWAY_PORT,
            trace_json: false,
        }
    }
}

/// 도움말을 요청받았을 때와 파싱에 실패했을 때를 구분한다.
#[derive(Debug)]
pub enum Parsed {
    Options(Box<Options>),
    Help,
}

pub fn parse<I>(args: I) -> Result<Parsed, String>
where
    I: IntoIterator<Item = String>,
{
    let mut options = Options::default();
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

            "--port" => {
                let raw = value(&mut args, "--port")?;
                options.port = raw
                    .parse()
                    .map_err(|_| format!("`--port` expects a number, got `{raw}`"))?;
            }

            other if other.starts_with('-') => return Err(format!("unknown option `{other}`")),

            positional if apps_dir_seen => {
                return Err(format!("unexpected argument `{positional}`"))
            }

            positional => {
                options.apps_dir = PathBuf::from(positional);
                apps_dir_seen = true;
            }
        }
    }

    Ok(Parsed::Options(Box::new(options)))
}

fn value<I: Iterator<Item = String>>(args: &mut I, flag: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("`{flag}` needs a value"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Options, String> {
        match parse(args.iter().map(|arg| (*arg).to_owned()))? {
            Parsed::Options(options) => Ok(*options),
            Parsed::Help => Err("help".to_owned()),
        }
    }

    #[test]
    fn defaults_to_the_apps_directory_and_the_gateway_port() {
        let options = parse_args(&[]).expect("no arguments is valid");
        assert_eq!(options.apps_dir, PathBuf::from("apps"));
        assert_eq!(options.port, DEFAULT_GATEWAY_PORT);
        assert!(!options.trace_json);
    }

    #[test]
    fn accepts_the_apps_directory_as_a_positional_argument() {
        let options = parse_args(&["examples"]).expect("positional apps dir");
        assert_eq!(options.apps_dir, PathBuf::from("examples"));
    }

    #[test]
    fn accepts_flags() {
        let options =
            parse_args(&["--apps", "examples", "--port", "0", "--trace-json"]).expect("flags");
        assert_eq!(options.apps_dir, PathBuf::from("examples"));
        assert_eq!(options.port, 0);
        assert!(options.trace_json);
    }

    #[test]
    fn rejects_a_missing_value() {
        assert!(parse_args(&["--port"]).is_err());
    }

    #[test]
    fn rejects_a_port_that_is_not_a_number() {
        assert!(parse_args(&["--port", "http"]).is_err());
    }

    #[test]
    fn rejects_an_unknown_option() {
        assert!(parse_args(&["--verbose"]).is_err());
    }

    #[test]
    fn rejects_a_second_positional_argument() {
        assert!(parse_args(&["one", "two"]).is_err());
    }

    #[test]
    fn asks_for_help() {
        assert!(matches!(
            parse(["--help".to_owned()]).expect("help parses"),
            Parsed::Help
        ));
    }
}
