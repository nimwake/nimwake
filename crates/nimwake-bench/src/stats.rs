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
