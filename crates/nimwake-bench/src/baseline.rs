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
}

impl Change {
    /// 변화율. 이전 값이 0이면 비율을 낼 수 없다.
    pub fn percent(&self) -> Option<f64> {
        (self.previous.abs() > f64::EPSILON)
            .then(|| (self.current - self.previous) / self.previous * 100.0)
    }

    /// 눈에 띄는 악화인가. 경고 표시에만 쓴다.
    ///
    /// 여기 걸린다고 release를 막지 않는다. runner 편차가 30%를 넘는 일도 흔하다.
    pub fn is_regression(&self) -> bool {
        self.percent().is_some_and(|percent| percent > WARN_PERCENT)
    }
}

/// 경고를 띄우는 악화 폭.
const WARN_PERCENT: f64 = 30.0;

/// 이전 baseline과 지금 값을 맞춰 비교 목록을 만든다.
///
/// 이전 값이 없는 항목은 비교할 것이 없으므로 빠진다.
pub fn compare(
    previous: &Baseline,
    current: &Baseline,
    metrics: &[(&'static str, &'static str, &'static str)],
) -> Vec<Change> {
    metrics
        .iter()
        .filter_map(|(key, metric, unit)| {
            Some(Change {
                metric,
                unit,
                previous: previous.get(key)?,
                current: current.get(key)?,
            })
        })
        .collect()
}

/// 리포트에 싣는 지표. `(baseline key, 표시 이름, 단위)`
pub const TRACKED: &[(&str, &str, &str)] = &[
    ("warm_p50_ms", "warm p50", "ms"),
    ("warm_p95_ms", "warm p95", "ms"),
    ("proxy_overhead_p95_ms", "proxy overhead p95", "ms"),
    ("daemon_rss_mb", "daemon RSS", "MB"),
    ("cold_start_total_ms", "cold start", "ms"),
    ("cold_start_queue_ms", "actor queue", "ms"),
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
        };

        assert!(!change.is_regression());
    }

    #[test]
    fn a_zero_previous_value_has_no_percentage() {
        let change = Change {
            metric: "warm p95",
            unit: "ms",
            previous: 0.0,
            current: 1.0,
        };

        assert_eq!(change.percent(), None);
        assert!(!change.is_regression());
    }
}
