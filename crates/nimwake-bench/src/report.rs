//! 리포트 조립.
//!
//! 요구사항: PRD §73 Performance Report.
//!
//! 수치를 판정하지 않는다. 다음 release가 직전과 비교할 수 있도록 형식을 고정할 뿐이다.

use std::fmt::Write;

use crate::baseline::{self, Baseline};
use crate::stats::{delta_ms, ms, Summary};
use crate::stress::{Resources, StressResult, SustainedResult};
use crate::{StormResult, SweepRow, WarmResult};

pub struct Report {
    commit: String,
    warm: Option<(Summary, WarmResult)>,
    storm: Option<StormResult>,
    sweep: Vec<SweepRow>,
    stress: Option<StressResult>,
    sustained: Option<SustainedResult>,
    previous: Option<Baseline>,
}

impl Report {
    pub fn new(commit: &str) -> Self {
        Self {
            commit: commit.to_owned(),
            warm: None,
            storm: None,
            sweep: Vec::new(),
            stress: None,
            sustained: None,
            previous: None,
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

    pub fn stress(&mut self, result: StressResult) {
        self.stress = Some(result);
    }

    pub fn sustained(&mut self, result: SustainedResult) {
        self.sustained = Some(result);
    }

    pub fn compare_with(&mut self, previous: Baseline) {
        self.previous = Some(previous);
    }

    /// 다음 실행이 비교에 쓸 값들. (PRD §73)
    pub fn baseline(&self) -> Baseline {
        let mut baseline = Baseline::new(&self.commit);

        if let Some((direct, nimwake)) = &self.warm {
            baseline.set("warm_p50_ms", millis(nimwake.summary.p50));
            baseline.set("warm_p95_ms", millis(nimwake.summary.p95));
            baseline.set(
                "proxy_overhead_p95_ms",
                millis(nimwake.summary.p95) - millis(direct.p95),
            );
            if let Some(rss) = nimwake.rss {
                baseline.set("daemon_rss_mb", rss as f64 / (1024.0 * 1024.0));
            }
        }

        if let Some(spans) = self.storm.as_ref().and_then(|storm| storm.spans.first()) {
            baseline.set("cold_start_total_ms", millis(spans.total));
            baseline.set("cold_start_queue_ms", millis(spans.queue));
        }

        baseline
    }

    /// release를 막을 사유. **correctness만 들어온다.** (PRD §44)
    pub fn correctness_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();

        if let Some(storm) = &self.storm {
            if storm.failures > 0 {
                problems.push(format!(
                    "cold-start storm: {} failed request(s)",
                    storm.failures
                ));
            }
            if storm.unique_pids > 1 {
                problems.push(format!(
                    "cold-start storm started {} processes, expected 1",
                    storm.unique_pids
                ));
            }
        }

        if let Some(stress) = &self.stress {
            problems.extend(stress.leaks().into_iter().map(|p| format!("stress: {p}")));
        }
        if let Some(sustained) = &self.sustained {
            problems.extend(sustained.leaks());
        }

        problems
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
        self.render_stress(&mut out);
        self.render_sustained(&mut out);
        self.render_comparison(&mut out);
        out
    }

    /// 누수 검증. before / after를 함께 기록한다. (PRD §43)
    fn render_stress(&self, out: &mut String) {
        let Some(stress) = &self.stress else {
            return;
        };

        let _ = writeln!(
            out,
            "## Lifecycle stress ({} cycles / {} apps)\n",
            stress.cycles, stress.apps
        );
        let _ = writeln!(out, "| | |");
        let _ = writeln!(out, "| --- | ---: |");
        let _ = writeln!(out, "| wall clock | {} |", ms(stress.wall));
        let _ = writeln!(out, "| spawned processes | {} |", stress.spawned);
        let _ = writeln!(out, "| failed requests | {} |", stress.failures);
        let _ = writeln!(out, "| orphan processes | {} |", stress.survivors.len());
        let _ = writeln!(out, "| zombie processes | {} |", stress.zombies);
        let _ = writeln!(out, "| ports left open | {} |\n", stress.open_ports.len());

        let _ = writeln!(out, "| | before | after |");
        let _ = writeln!(out, "| --- | ---: | ---: |");
        let _ = writeln!(
            out,
            "| FD | {} | {} |",
            optional(stress.before.fds),
            optional(stress.after.fds)
        );
        let _ = writeln!(
            out,
            "| RSS | {} | {} |\n",
            megabytes(stress.before.rss),
            megabytes(stress.after.rss)
        );

        let leaks = stress.leaks();
        if leaks.is_empty() {
            let _ = writeln!(out, "누수 없음.\n");
        } else {
            let _ = writeln!(out, "**누수 발견:**\n");
            for leak in leaks {
                let _ = writeln!(out, "- {leak}");
            }
            let _ = writeln!(out);
        }
    }

    /// proxy 경로 자체의 누수. 기동 없이 지속되는 경로를 본다. (PRD §43)
    fn render_sustained(&self, out: &mut String) {
        let Some(sustained) = &self.sustained else {
            return;
        };

        let (rss, fds) = sustained.growth();
        let _ = writeln!(
            out,
            "## Sustained warm load ({}s)\n",
            sustained.duration.as_secs()
        );
        let _ = writeln!(out, "| | |");
        let _ = writeln!(out, "| --- | ---: |");
        let _ = writeln!(out, "| requests | {} |", sustained.requests);
        let _ = writeln!(out, "| failed | {} |", sustained.failures);
        let _ = writeln!(out, "| RSS 증가 | {} |", growth_bytes(rss));
        let _ = writeln!(out, "| FD 증가 | {} |\n", growth_count(fds));

        let _ = render_trend(out, &sustained.samples);
    }

    /// 직전 release 대비. 경고만 남기고 release를 막지 않는다. (PRD §73)
    fn render_comparison(&self, out: &mut String) {
        let Some(previous) = &self.previous else {
            return;
        };

        let current = self.baseline();
        let changes = baseline::compare(previous, &current, baseline::TRACKED);

        let _ = writeln!(out, "## 직전 대비\n");
        if changes.is_empty() {
            let _ = writeln!(out, "비교할 공통 지표가 없다.\n");
            return;
        }

        let _ = writeln!(out, "| 지표 | {} | current | Δ |", previous.label());
        let _ = writeln!(out, "| --- | ---: | ---: | ---: |");

        for change in &changes {
            let delta = change.percent().map_or_else(
                || "n/a".to_owned(),
                |percent| {
                    let mark = if change.is_regression() { " (!)" } else { "" };
                    format!("{percent:+.0}%{mark}")
                },
            );
            let _ = writeln!(
                out,
                "| {} | {:.2} {} | {:.2} {} | {delta} |",
                change.metric, change.previous, change.unit, change.current, change.unit,
            );
        }

        let _ = writeln!(
            out,
            "\n(!)는 경고일 뿐이다. runner 편차가 커서 절대 수치를 gate로 쓰지 않는다. (PRD §73)\n"
        );
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

fn millis(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn growth_bytes(delta: Option<i64>) -> String {
    delta.map_or_else(
        || "n/a".to_owned(),
        |delta| format!("{:+.1} MB", delta as f64 / (1024.0 * 1024.0)),
    )
}

fn growth_count(delta: Option<i64>) -> String {
    delta.map_or_else(|| "n/a".to_owned(), |delta| format!("{delta:+}"))
}

/// 자원 추이를 간단한 막대로 보여 준다. 숫자 나열보다 추세가 눈에 들어온다.
fn render_trend(out: &mut String, samples: &[Resources]) -> std::fmt::Result {
    let values: Vec<u64> = samples.iter().filter_map(|sample| sample.rss).collect();
    let (Some(min), Some(max)) = (values.iter().min(), values.iter().max()) else {
        return Ok(());
    };

    writeln!(out, "```text")?;
    writeln!(out, "RSS {:.1} MB -> {:.1} MB", mb(*min), mb(*max))?;
    for value in &values {
        let span = (max - min).max(1);
        let width = usize::try_from((value - min) * 40 / span).unwrap_or(0);
        writeln!(out, "{:>8.1} |{}", mb(*value), "#".repeat(width.max(1)))?;
    }
    writeln!(out, "```\n")
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
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
