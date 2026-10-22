//! 직전 release와의 비교.
//!
//! 요구사항: PRD §73 Performance Report와 Regression.
//!
//! 리포트는 단독 수치가 아니라 **직전 release 대비 변화**로 읽는다.
//! 그 비교가 있어야 "이번 변경이 실제로 빨라졌는가"를 객관적으로 판단할 수 있다.
//!
//! ```text
//!                     v0.0.4     current      Δ
//! proxy p95           1.20ms      0.91ms    -24%
//! daemon RSS          16.8MB      14.2MB    -15%
//! actor queue p95     0.45ms      0.70ms    +56%  ⚠
//! ```
//!
//! **차이는 경고로만 남긴다.** release를 막지 않는다.
//! GitHub-hosted runner의 편차가 커서 절대 수치를 gate로 쓰기에 적절하지 않다.
//! baseline이 충분히 쌓인 뒤에 상대 threshold를 도입한다.

use std::collections::BTreeMap;
use std::path::Path;

/// 다음 실행이 비교에 쓸 수 있도록 남기는 값들.
///
/// `key=value` 한 줄씩이다. JSON을 쓰지 않는 이유는 이 파일을 읽는 쪽이
/// 우리 자신뿐이고, 파서가 필요 없을 만큼 단순한 형태로 충분하기 때문이다.
#[derive(Debug, Default, Clone)]
pub struct Baseline {
    values: BTreeMap<String, f64>,
    label: String,
}

impl Baseline {
    pub fn new(label: &str) -> Self {
        Self {
            values: BTreeMap::new(),
            label: label.to_owned(),
        }
    }

    pub fn set(&mut self, key: &str, value: f64) {
        self.values.insert(key.to_owned(), value);
    }

    pub fn get(&self, key: &str) -> Option<f64> {
        self.values.get(key).copied()
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn render(&self) -> String {
        let mut out = format!("label={}\n", self.label);
        for (key, value) in &self.values {
            out.push_str(&format!("{key}={value}\n"));
        }
        out
    }

    /// 형식이 조금 어긋난 줄은 건너뛴다.
    ///
    /// 비교는 부가 정보다. baseline 파일 하나 때문에 release가 멈추면 안 된다.
    pub fn parse(text: &str) -> Self {
        let mut baseline = Self::new("previous");

        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());

            if key == "label" {
                baseline.label = value.to_owned();
            } else if let Ok(number) = value.parse() {
                baseline.set(key, number);
            }
        }

        baseline
    }

    pub fn load(path: &Path) -> Option<Self> {
        std::fs::read_to_string(path)
            .ok()
            .map(|text| Self::parse(&text))
    }
}

/// 비교 한 줄.
pub struct Change {
    pub metric: &'static str,
    pub unit: &'static str,
    pub previous: f64,
    pub current: f64,
    /// 이 지표에서 **관측한** runner 편차(%). (PRD §73)
    ///
    /// 경고를 띄울 문턱이다. 모든 지표에 같은 30%를 쓰면 12개 중 7개에서
    /// 잡음이 그것을 넘어 `(!)`가 상시로 뜬다 — 상시로 뜨는 경고는 읽지 않게
    /// 되고, 읽지 않는 경고는 없는 것과 같다.
    pub noise_pct: f64,
}

impl Change {
    /// 변화율. 이전 값이 0이면 비율을 낼 수 없다.
    pub fn percent(&self) -> Option<f64> {
        (self.previous.abs() > f64::EPSILON)
            .then(|| (self.current - self.previous) / self.previous * 100.0)
    }

    /// 눈에 띄는 악화인가. 경고 표시에만 쓴다.
    ///
    /// **지표마다 문턱이 다르다.** `v0.1.0`~`v0.1.3`의 baseline 네 개에서 관측한
    /// 편차를 쓴다 — 그 구간에 런타임 변경이 거의 없으므로 차이는 거의 전부
    /// runner 편차다 (PRD §73).
    ///
    /// 여기 걸린다고 release를 막지 않는다. 표시만 한다.
    pub fn is_regression(&self) -> bool {
        self.percent()
            .is_some_and(|percent| percent > self.noise_pct)
    }
}

/// 이전 baseline과 지금 값을 맞춰 비교 목록을 만든다.
///
/// 이전 값이 없는 항목은 비교할 것이 없으므로 빠진다.
pub fn compare(
    previous: &Baseline,
    current: &Baseline,
    metrics: &[(&'static str, &'static str, &'static str, f64)],
) -> Vec<Change> {
    metrics
        .iter()
        .filter_map(|(key, metric, unit, noise_pct)| {
            Some(Change {
                metric,
                unit,
                previous: previous.get(key)?,
                current: current.get(key)?,
                noise_pct: *noise_pct,
            })
        })
        .collect()
}

/// 리포트에 싣는 지표. `(baseline key, 표시 이름, 단위, 관측한 runner 편차 %)`
///
/// 마지막 값은 `v0.1.0`~`v0.1.3`의 baseline 네 개에서 **관측한** 폭이다. 그
/// 구간에 런타임 변경이 거의 없으므로(control socket과 CLI는 요청 경로를 지나지
/// 않는다) 차이는 거의 전부 runner 편차다. (PRD §73)
///
/// **표본 네 개로 잰 폭이고 분포가 아니다.** 통계적 한계가 아니라 "적어도 이만큼
/// 흔들린다"는 하한으로 쓴다. 더 좁히려면 같은 commit을 여러 번 돌려야 한다.
///
/// `daemon RSS`만 2%다. 그 행의 변화는 읽을 만하고, 나머지는 이 폭을 넘을 때만
/// 눈여겨볼 값이다.
/// 그 지표가 어디서 오는가. 빠졌을 때 어디를 볼지 알려 준다.
///
/// `/proc`에서 오는 둘을 특히 분명히 적는다 — **기기의 proot는 `/proc` 일부를
/// 막는다.** 경고에 경로가 적혀 있으면 기기에서 바로 확인할 수 있다.
#[must_use]
pub fn metric_source(key: &str) -> &'static str {
    match key {
        "daemon_rss_mb" => "/proc/<pid>/status 의 VmRSS",
        "sustained_cpu_pct_of_wall" => "/proc/<pid>/stat 의 utime·stime",
        key if key.starts_with("cold_start_") => "nimwake::startup trace (--trace-json)",
        key if key.starts_with("request_") => "nimwake::request trace (--trace-json)",
        key if key.starts_with("concurrency_") => "concurrency sweep",
        _ => "warm load",
    }
}

pub const TRACKED: &[(&str, &str, &str, f64)] = &[
    ("warm_p50_ms", "warm p50", "ms", 57.0),
    ("warm_p95_ms", "warm p95", "ms", 57.0),
    ("proxy_overhead_p50_ms", "proxy overhead p50", "ms", 58.0),
    ("proxy_overhead_p95_ms", "proxy overhead p95", "ms", 51.0),
    ("daemon_rss_mb", "daemon RSS", "MB", 2.0),
    ("cold_start_total_ms", "cold start", "ms", 24.0),
    ("cold_start_queue_ms", "actor queue", "ms", 36.0),
    ("cold_start_spawn_ms", "cold start spawn", "ms", 28.0),
    (
        "cold_start_readiness_ms",
        "cold start readiness",
        "ms",
        29.0,
    ),
    ("concurrency_knee", "concurrency knee", "", 100.0),
    ("concurrency_knee_p95_ms", "p95 at the knee", "ms", 86.0),
    (
        "sustained_cpu_pct_of_wall",
        "sustained CPU",
        "% of wall",
        15.0,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_baseline_round_trips() {
        let mut baseline = Baseline::new("v0.0.4");
        baseline.set("warm_p95_ms", 1.2);

        let parsed = Baseline::parse(&baseline.render());

        assert_eq!(parsed.label(), "v0.0.4");
        assert_eq!(parsed.get("warm_p95_ms"), Some(1.2));
    }

    /// **값을 깎지 않고 왕복한다.**
    ///
    /// 위의 테스트는 값이 `1.2` 하나뿐이라 **어떤 정밀도로 적어도 통과한다** —
    /// `{:.3}`으로 적어도, `{:.1}`로 적어도 `1.2`는 살아 돌아온다.
    ///
    /// `render`가 `{value}`를 쓰므로 Rust가 **다시 읽으면 같은 f64가 되는 가장
    /// 짧은 표기**를 낸다. 그것에 기대고 있으니 그것을 고정한다. 누가 자리수를
    /// 맞추려고 `{:.3}`을 넣으면 여기서 걸린다.
    ///
    /// baseline은 `직전 대비` 비교의 입력이다. 값이 깎이면 그 차이가 **없는
    /// 회귀로 보이거나 있는 회귀를 가린다.**
    #[test]
    fn it_round_trips_without_rounding() {
        let mut baseline = Baseline::new("v9.9.9");

        // 실제로 나오는 모양들 — 소수 네 자리, 아주 작은 값, RSS처럼 큰 값.
        let samples = [
            ("warm_p50_ms", 0.021),
            ("warm_p95_ms", 1.2345),
            ("proxy_overhead_p50_ms", 0.0005),
            ("cold_start_p95_ms", 1234.5678),
            ("daemon_rss_kb", 8192.0),
            ("zero_ms", 0.0),
        ];
        for (key, value) in samples {
            baseline.set(key, value);
        }

        let parsed = Baseline::parse(&baseline.render());

        assert_eq!(parsed.label(), "v9.9.9");
        for (key, value) in samples {
            assert_eq!(
                parsed.get(key),
                Some(value),
                "`{key}`가 왕복에서 깎였다: {value} → {:?}",
                parsed.get(key),
            );
        }
    }

    /// 추적하는 지표 **전부**가 왕복한다.
    ///
    /// 표본 몇 개로는 특정 이름만 새는 경우를 보지 못한다. `TRACKED`에 지표를
    /// 더할 때 이 테스트가 같이 커진다.
    #[test]
    fn every_tracked_metric_round_trips() {
        let mut baseline = Baseline::new("all");
        // 이름마다 다른 값을 넣는다. 같은 값이면 뒤섞여도 통과한다.
        for (index, (key, _, _, _)) in TRACKED.iter().enumerate() {
            baseline.set(key, index as f64 + 0.5);
        }

        let parsed = Baseline::parse(&baseline.render());

        for (index, (key, _, _, _)) in TRACKED.iter().enumerate() {
            assert_eq!(
                parsed.get(key),
                Some(index as f64 + 0.5),
                "`{key}`가 왕복에서 사라지거나 뒤섞였다",
            );
        }
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let parsed = Baseline::parse("label=old\ngarbage\nwarm_p95_ms=not-a-number\nok_ms=2\n");

        assert_eq!(parsed.label(), "old");
        assert_eq!(parsed.get("warm_p95_ms"), None);
        assert_eq!(parsed.get("ok_ms"), Some(2.0));
    }

    #[test]
    fn a_metric_missing_from_the_previous_run_is_dropped() {
        let previous = Baseline::new("old");
        let mut current = Baseline::new("new");
        current.set("warm_p95_ms", 1.0);

        assert!(compare(&previous, &current, TRACKED).is_empty());
    }

    #[test]
    fn an_improvement_is_negative_and_not_a_regression() {
        let change = Change {
            metric: "warm p95",
            unit: "ms",
            previous: 1.20,
            current: 0.91,
            // 전의 공통 문턱과 같은 값. 이 테스트들의 뜻을 바꾸지 않는다.
            noise_pct: 30.0,
        };

        assert!(change.percent().expect("comparable") < 0.0);
        assert!(!change.is_regression());
    }

    #[test]
    fn a_large_slowdown_is_flagged() {
        let change = Change {
            metric: "actor queue",
            unit: "ms",
            previous: 0.45,
            current: 0.70,
            // 전의 공통 문턱과 같은 값. 이 테스트들의 뜻을 바꾸지 않는다.
            noise_pct: 30.0,
        };

        assert!(change.is_regression());
    }

    #[test]
    fn a_small_slowdown_is_not_flagged() {
        let change = Change {
            metric: "warm p95",
            unit: "ms",
            previous: 1.00,
            current: 1.10,
            // 전의 공통 문턱과 같은 값. 이 테스트들의 뜻을 바꾸지 않는다.
            noise_pct: 30.0,
        };

        assert!(!change.is_regression());
    }

    /// **지표마다 문턱이 다르다.** (PRD §73)
    ///
    /// 같은 +10%가 `daemon RSS`(잡음 2%)에서는 눈여겨볼 값이고
    /// `proxy overhead`(잡음 58%)에서는 잡음이다. 공통 30%를 쓰면 12개 중
    /// 7개에서 `(!)`가 상시로 뜬다.
    #[test]
    fn the_warning_threshold_follows_each_metrics_noise() {
        let rss = Change {
            metric: "daemon RSS",
            unit: "MB",
            previous: 6.00,
            current: 6.60, // +10%
            noise_pct: 2.0,
        };
        let overhead = Change {
            metric: "proxy overhead p50",
            unit: "ms",
            previous: 0.40,
            current: 0.44, // 같은 +10%
            noise_pct: 58.0,
        };

        assert!(
            rss.is_regression(),
            "RSS는 2%만 흔들린다 — +10%는 눈여겨볼 값이다",
        );
        assert!(
            !overhead.is_regression(),
            "proxy overhead는 58% 흔들린다 — +10%는 잡음이다",
        );
    }

    /// `TRACKED`의 잡음 값이 비어 있지 않다. (PRD §73)
    ///
    /// 0으로 두면 모든 변화가 경고가 되고, 너무 크게 두면 아무것도 경고가
    /// 되지 않는다. 관측한 값을 적었는지 본다.
    #[test]
    fn every_tracked_metric_declares_its_noise() {
        for (key, _, _, noise) in TRACKED {
            assert!(
                *noise > 0.0 && *noise <= 100.0,
                "`{key}`의 잡음이 {noise}%다 — 관측한 값을 적어야 한다",
            );
        }
        let rss = TRACKED
            .iter()
            .find(|(key, ..)| *key == "daemon_rss_mb")
            .expect("RSS는 추적한다");
        assert!(
            rss.3 < 10.0,
            "RSS는 유일하게 안정적인 지표다 — 잡음이 {}%로 적혀 있다",
            rss.3,
        );
    }

    #[test]
    fn a_zero_previous_value_has_no_percentage() {
        let change = Change {
            metric: "warm p95",
            unit: "ms",
            previous: 0.0,
            current: 1.0,
            // 전의 공통 문턱과 같은 값. 이 테스트들의 뜻을 바꾸지 않는다.
            noise_pct: 30.0,
        };

        assert_eq!(change.percent(), None);
        assert!(!change.is_regression());
    }
}
