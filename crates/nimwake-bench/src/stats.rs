//! latency 표본 집계.
//!
//! 요구사항: PRD §42 Load Tests.
//!
//! 히스토그램 라이브러리를 들이지 않는다. 표본을 다 들고 있다가 정렬한다.
//! 표본 수가 수십만을 넘기 시작하면 그때 바꾼다.

use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct Samples {
    latencies: Vec<Duration>,
    errors: usize,
    wall: Duration,
}

impl Samples {
    pub fn record(&mut self, latency: Duration) {
        self.latencies.push(latency);
    }

    pub fn record_error(&mut self) {
        self.errors += 1;
    }

    pub fn merge(&mut self, other: Self) {
        self.latencies.extend(other.latencies);
        self.errors += other.errors;
    }

    pub fn set_wall(&mut self, wall: Duration) {
        self.wall = wall;
    }

    pub fn summarize(mut self) -> Summary {
        self.latencies.sort_unstable();

        Summary {
            count: self.latencies.len(),
            errors: self.errors,
            p50: percentile(&self.latencies, 50.0),
            p95: percentile(&self.latencies, 95.0),
            p99: percentile(&self.latencies, 99.0),
            rps: rate(self.latencies.len(), self.wall),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Summary {
    pub count: usize,
    pub errors: usize,
    pub p50: Duration,
    pub p95: Duration,
    pub p99: Duration,
    pub rps: f64,
}

/// 처리량 곡선이 평평해지기 시작하는 지점. (PRD §50 concurrency knee point)
///
/// "최대 처리량을 낸 동시성"으로 정의하지 않는다. 포화된 뒤에는 어느 단계가
/// 가장 빨랐는지가 runner 편차로 갈려 값이 튄다.
///
/// **최대 처리량의 95%에 처음 도달한 동시성**을 쓴다. 그 뒤로 동시성을 더 올리면
/// 처리량은 거의 늘지 않고 지연만 늘어난다. 저사양 환경에서 이 지점이 얼마나
/// 앞으로 당겨지는지가 §50이 보려는 것이다.
///
/// 임계값 0.95는 정한 값이다. 근거가 아니라 선택이므로 바꾸려면 여기를 본다.
const KNEE_FRACTION: f64 = 0.95;

/// `(동시성, 처리량)` 쌍에서 knee를 찾는다. 비어 있으면 `None`.
#[must_use]
pub fn knee(points: &[(usize, f64)]) -> Option<usize> {
    let peak = points
        .iter()
        .map(|&(_, rps)| rps)
        .fold(f64::NEG_INFINITY, f64::max);

    if !peak.is_finite() || peak <= 0.0 {
        return None;
    }

    let threshold = peak * KNEE_FRACTION;
    points
        .iter()
        .filter(|&&(_, rps)| rps >= threshold)
        .map(|&(concurrency, _)| concurrency)
        .min()
}

/// 가장 가까운 순위 방식. 표본이 적을 때 보간보다 읽기 쉽다.
fn percentile(sorted: &[Duration], percent: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }

    let rank = (percent / 100.0 * sorted.len() as f64).ceil() as usize;
    let index = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[index]
}

fn rate(count: usize, wall: Duration) -> f64 {
    if wall.is_zero() {
        return 0.0;
    }
    count as f64 / wall.as_secs_f64()
}

/// `1.83 ms`처럼 읽기 좋은 형태.
pub fn ms(duration: Duration) -> String {
    format!("{:.2} ms", duration.as_secs_f64() * 1000.0)
}

/// 두 값의 차이. 음수도 그대로 보여 준다.
pub fn delta_ms(slower: Duration, faster: Duration) -> String {
    let delta = slower.as_secs_f64() - faster.as_secs_f64();
    format!("{:+.2} ms", delta * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(millis: &[u64]) -> Samples {
        let mut samples = Samples::default();
        for value in millis {
            samples.record(Duration::from_millis(*value));
        }
        samples.set_wall(Duration::from_secs(1));
        samples
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let summary = samples(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]).summarize();
        assert_eq!(summary.p50, Duration::from_millis(5));
        assert_eq!(summary.p95, Duration::from_millis(10));
        assert_eq!(summary.count, 10);
    }

    #[test]
    fn an_empty_sample_set_reports_zero() {
        let summary = Samples::default().summarize();
        assert_eq!(summary.p95, Duration::ZERO);
        assert_eq!(summary.rps, 0.0);
    }

    #[test]
    fn errors_are_counted_separately_from_latencies() {
        let mut samples = samples(&[1, 2]);
        samples.record_error();
        let summary = samples.summarize();
        assert_eq!(summary.count, 2);
        assert_eq!(summary.errors, 1);
    }

    #[test]
    fn rps_uses_the_wall_clock() {
        let mut samples = samples(&[1; 100]);
        samples.set_wall(Duration::from_secs(2));
        assert!((samples.summarize().rps - 50.0).abs() < f64::EPSILON);
    }
}

#[cfg(test)]
mod knee_tests {
    use super::knee;

    #[test]
    fn knee_is_the_first_concurrency_reaching_the_plateau() {
        // 100에서 이미 최대(1000)의 95%를 넘으므로 그 뒤는 지연만 늘어난다.
        let points = [(1, 100.0), (10, 600.0), (100, 980.0), (500, 1000.0)];
        assert_eq!(knee(&points), Some(100));
    }

    #[test]
    fn a_peak_in_the_middle_does_not_move_the_knee_backwards() {
        // 포화 뒤에는 어느 단계가 가장 빨랐는지가 편차로 갈린다. 최대값을
        // 그대로 쓰면 knee가 튄다. 이 경우 500이 최대지만 knee는 100이다.
        let points = [(1, 100.0), (100, 960.0), (500, 1000.0), (1000, 940.0)];
        assert_eq!(knee(&points), Some(100));
    }

    #[test]
    fn a_single_point_is_its_own_knee() {
        assert_eq!(knee(&[(1, 42.0)]), Some(1));
    }

    #[test]
    fn no_points_or_no_throughput_has_no_knee() {
        assert_eq!(knee(&[]), None);
        assert_eq!(knee(&[(1, 0.0), (10, 0.0)]), None);
    }
}
