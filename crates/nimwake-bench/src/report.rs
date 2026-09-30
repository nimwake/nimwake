//! 리포트 조립.
//!
//! 요구사항: PRD §73 Performance Report.
//!
//! 수치를 판정하지 않는다. 다음 release가 직전과 비교할 수 있도록 형식을 고정할 뿐이다.

use std::fmt::Write;

use crate::stats::{delta_ms, ms, Summary};
use crate::{StormResult, SweepRow, WarmResult};

pub struct Report {
    commit: String,
    warm: Option<(Summary, WarmResult)>,
    storm: Option<StormResult>,
    sweep: Vec<SweepRow>,
}

impl Report {
    pub fn new(commit: &str) -> Self {
        Self {
            commit: commit.to_owned(),
            warm: None,
            storm: None,
            sweep: Vec::new(),
        }
    }

    pub fn warm_path(&mut self, direct: Summary, nimwake: WarmResult) {
        self.warm = Some((direct, nimwake));
    }

    pub fn cold_start(&mut self, storm: StormResult) {
        self.storm = Some(storm);
    }

    pub fn sweep_row(&mut self, row: SweepRow) {
        self.sweep.push(row);
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Nimwake Performance Report\n");
        let _ = writeln!(out, "commit: {}\n", self.commit);
        let _ = writeln!(
            out,
            "수치는 기록만 한다. merge와 release를 막지 않는다. (PRD §73)\n"
        );

        self.render_warm(&mut out);
        self.render_cold_start(&mut out);
        self.render_sweep(&mut out);
        out
    }

    fn render_warm(&self, out: &mut String) {
        let Some((direct, nimwake)) = &self.warm else {
            return;
        };
        let proxy = &nimwake.summary;

        let _ = writeln!(out, "## Warm path\n");
        let _ = writeln!(out, "| | direct backend | nimwake | overhead |");
        let _ = writeln!(out, "| --- | ---: | ---: | ---: |");
        for (label, a, b) in [
            ("p50", direct.p50, proxy.p50),
            ("p95", direct.p95, proxy.p95),
            ("p99", direct.p99, proxy.p99),
        ] {
            let _ = writeln!(
                out,
                "| {label} | {} | {} | {} |",
                ms(a),
                ms(b),
                delta_ms(b, a)
            );
        }
        let _ = writeln!(
            out,
            "| requests/sec | {:.0} | {:.0} | |",
            direct.rps, proxy.rps
        );
        let _ = writeln!(out, "| errors | {} | {} | |\n", direct.errors, proxy.errors);

        let _ = writeln!(out, "| daemon | |");
        let _ = writeln!(out, "| --- | ---: |");
        let _ = writeln!(out, "| RSS | {} |", megabytes(nimwake.rss));
        let _ = writeln!(out, "| open fds | {} |\n", optional(nimwake.fds));
    }

    fn render_cold_start(&self, out: &mut String) {
        let Some(storm) = &self.storm else {
            return;
        };

        let _ = writeln!(out, "## Cold-start storm ({} requests)\n", storm.requests);
        let _ = writeln!(out, "| | |");
        let _ = writeln!(out, "| --- | ---: |");
        let _ = writeln!(out, "| spawn count | {} |", storm.spans.len());
        let _ = writeln!(out, "| unique backend pids | {} |", storm.unique_pids);
        let _ = writeln!(out, "| failed requests | {} |", storm.failures);
        let _ = writeln!(out, "| wall clock | {} |\n", ms(storm.wall));

        // cold start를 단일 숫자로 기록하지 않는다. (PRD §42)
        let Some(spans) = storm.spans.first() else {
            let _ = writeln!(out, "구간 계측이 수집되지 않았다.\n");
            return;
        };

        let _ = writeln!(out, "구간별 분해:\n");
        let _ = writeln!(out, "| 구간 | |");
        let _ = writeln!(out, "| --- | ---: |");
        let _ = writeln!(out, "| actor queue | {} |", ms(spans.queue));
        let _ = writeln!(out, "| process spawn | {} |", ms(spans.spawn));
        let _ = writeln!(out, "| app readiness | {} |", ms(spans.readiness));
        let _ = writeln!(out, "| **total** | **{}** |\n", ms(spans.total));
        let _ = writeln!(out, "지배적인 구간: **{}**\n", dominant(spans));
    }

    fn render_sweep(&self, out: &mut String) {
        if self.sweep.is_empty() {
            return;
        }

        let _ = writeln!(out, "## Concurrency sweep\n");
        let _ = writeln!(
            out,
            "| concurrency | p50 | p95 | p99 | req/s | errors | RSS |"
        );
        let _ = writeln!(out, "| ---: | ---: | ---: | ---: | ---: | ---: | ---: |");

        for row in &self.sweep {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {:.0} | {} | {} |",
                row.concurrency,
                ms(row.summary.p50),
                ms(row.summary.p95),
                ms(row.summary.p99),
                row.summary.rps,
                row.summary.errors,
                megabytes(row.rss),
            );
        }

        let _ = writeln!(out);
        if let Some(knee) = self.knee_point() {
            let _ = writeln!(
                out,
                "p95가 직전 단계의 두 배를 넘는 첫 지점: **concurrency {knee}**\n"
            );
        } else {
            let _ = writeln!(out, "측정 범위 안에서 knee point가 나타나지 않았다.\n");
        }
    }

    /// p95가 직전 단계의 두 배를 넘는 첫 지점.
    ///
    /// 정확한 정의가 아니라 눈으로 찾던 것을 기계가 먼저 가리키게 하는 정도의 기준이다.
    fn knee_point(&self) -> Option<usize> {
        self.sweep.windows(2).find_map(|pair| {
            let (before, after) = (&pair[0], &pair[1]);
            (after.summary.p95 > before.summary.p95 * 2).then_some(after.concurrency)
        })
    }
}

fn dominant(spans: &crate::daemon::StartupSpans) -> &'static str {
    let candidates = [
        ("actor queue", spans.queue),
        ("process spawn", spans.spawn),
        ("app readiness", spans.readiness),
    ];

    candidates
        .iter()
        .max_by_key(|(_, span)| *span)
        .map_or("unknown", |(label, _)| *label)
}

fn megabytes(bytes: Option<u64>) -> String {
    bytes.map_or_else(
        || "n/a".to_owned(),
        |bytes| format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)),
    )
}

fn optional(value: Option<usize>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::stats::Samples;

    fn summary(p95_millis: u64) -> Summary {
        let mut samples = Samples::default();
        samples.record(Duration::from_millis(p95_millis));
        samples.set_wall(Duration::from_secs(1));
        samples.summarize()
    }

    fn row(concurrency: usize, p95_millis: u64) -> SweepRow {
        SweepRow {
            concurrency,
            summary: summary(p95_millis),
            rss: None,
        }
    }

    #[test]
    fn the_knee_point_is_where_p95_doubles() {
        let mut report = Report::new("test");
        report.sweep_row(row(1, 2));
        report.sweep_row(row(10, 3));
        report.sweep_row(row(50, 30));

        assert_eq!(report.knee_point(), Some(50));
    }

    #[test]
    fn a_flat_sweep_has_no_knee_point() {
        let mut report = Report::new("test");
        report.sweep_row(row(1, 2));
        report.sweep_row(row(10, 3));

        assert_eq!(report.knee_point(), None);
    }

    #[test]
    fn the_dominant_span_is_named() {
        let spans = crate::daemon::StartupSpans {
            queue: Duration::from_millis(3),
            spawn: Duration::from_millis(8),
            readiness: Duration::from_millis(821),
            total: Duration::from_millis(832),
        };
        assert_eq!(dominant(&spans), "app readiness");
    }

    #[test]
    fn a_report_without_measurements_still_renders() {
        let text = Report::new("abc123").render();
        assert!(text.contains("abc123"));
    }
}
