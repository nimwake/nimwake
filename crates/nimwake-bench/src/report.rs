//! 리포트 조립.
//!
//! 요구사항: PRD §73 Performance Report.
//!
//! 수치를 판정하지 않는다. 다음 release가 직전과 비교할 수 있도록 형식을 고정할 뿐이다.

use std::fmt::Write;
use std::time::Duration;

use crate::baseline::{self, Baseline};
use crate::stats::{self, ms};
use crate::stress::{cpu_used, CrashLoopResult, Resources, StressResult, SustainedResult};
use crate::{StormResult, SweepRow, WarmResult};

pub struct Report {
    commit: String,
    warm: Option<WarmResult>,
    storm: Option<StormResult>,
    sweep: Vec<SweepRow>,
    sleeping: Option<crate::SleepingResult>,
    stress: Option<StressResult>,
    sustained: Option<SustainedResult>,
    crash_loop: Option<CrashLoopResult>,
    previous: Option<Baseline>,
}

impl Report {
    pub fn new(commit: &str) -> Self {
        Self {
            commit: commit.to_owned(),
            warm: None,
            storm: None,
            sweep: Vec::new(),
            sleeping: None,
            stress: None,
            sustained: None,
            crash_loop: None,
            previous: None,
        }
    }

    pub fn warm_path(&mut self, warm: WarmResult) {
        self.warm = Some(warm);
    }

    pub fn sleeping(&mut self, sleeping: crate::SleepingResult) {
        self.sleeping = Some(sleeping);
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

    pub fn crash_loop(&mut self, result: CrashLoopResult) {
        self.crash_loop = Some(result);
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

        if let Some(warm) = &self.warm {
            let direct = &warm.paired.direct;
            let nimwake = &warm.paired.gateway;
            baseline.set("warm_p50_ms", millis(nimwake.p50));
            baseline.set("warm_p95_ms", millis(nimwake.p95));

            // **쌍마다의 차이에서 낸다.** 백분위를 뺀 값이 아니다. (PRD §50)
            //
            // 정의가 바뀌었다. 이전 baseline의 같은 키는 `p95(nimwake) - p95(direct)`
            // 였고, 두 실행의 부하가 달라 기기에서 음수까지 나왔다.
            if let Some(ns) = warm.paired.overhead_p50_ns {
                baseline.set("proxy_overhead_p50_ms", ns as f64 / 1_000_000.0);
            }
            if let Some(ns) = warm.paired.overhead_p95_ns {
                baseline.set("proxy_overhead_p95_ms", ns as f64 / 1_000_000.0);
            }
            // direct 쪽 백분위를 함께 남긴다. overhead가 음수로 나왔을 때
            // **왜 그런지 재구성할 수 있어야 한다.** 기기에서 그럴 일이 있었다.
            //
            // **`warm_rps`와 `warm_direct_rps`는 늘 같은 값이다.** 쌍마다 두
            // 경로에 하나씩 보내므로 count와 경과 시간이 양쪽 모두 같다 —
            // 구조적으로 같아지는 것이고 측정이 두 개인 것이 아니다. release
            // baseline 다섯 개에서 소수점까지 일치했다. 처리량은 이 부하의
            // 비교축이 아니다(무릎 탐색이 그것을 본다).
            baseline.set("warm_direct_p50_ms", millis(direct.p50));
            baseline.set("warm_direct_p95_ms", millis(direct.p95));
            baseline.set("warm_direct_rps", direct.rps);
            baseline.set("warm_rps", nimwake.rps);
            baseline.set("warm_connections", warm.connections as f64);
            if let Some(rss) = warm.rss {
                baseline.set("daemon_rss_mb", rss as f64 / (1024.0 * 1024.0));
            }
        }

        // cold start는 구간별로 담는다. §50의 비교 항목이
        // "spawn / readiness / proxy connect" 단위이기 때문이다.
        //
        // **표본은 lifecycle stress에서 가져온다.** cold-start storm은 기동 중복
        // 제거를 증명하려고 spawn을 하나만 만들고, 그 하나는 readiness probe
        // 간격(50ms)에 양자화되어 0ms 또는 50ms로 튄다. stress는 cycle마다
        // cold start를 하므로 표본이 수백 개다.
        if let Some(stress) = &self.stress {
            let p50 = |pick: fn(&crate::daemon::StartupSpans) -> Duration| {
                let values: Vec<Duration> = stress.spans.iter().map(pick).collect();
                stats::percentile_of(&values, 0.50)
            };

            if let Some(total) = p50(|s| s.total) {
                baseline.set("cold_start_total_ms", millis(total));
            }
            if let Some(queue) = p50(|s| s.queue) {
                baseline.set("cold_start_queue_ms", millis(queue));
            }
            if let Some(spawn) = p50(|s| s.spawn) {
                baseline.set("cold_start_spawn_ms", millis(spawn));
            }
            if let Some(readiness) = p50(|s| s.readiness) {
                baseline.set("cold_start_readiness_ms", millis(readiness));
            }
            baseline.set("cold_start_samples", stress.spans.len() as f64);
        }

        // 요청 구간. proxy connect가 §50의 비교 항목이고 기동 trace에 없다.
        if let Some(stress) = &self.stress {
            if !stress.request_spans.is_empty() {
                let p50 = |pick: fn(&crate::daemon::RequestSpans) -> Duration| {
                    let values: Vec<Duration> = stress.request_spans.iter().map(pick).collect();
                    millis(stats::percentile_of(&values, 0.50).unwrap_or_default())
                };
                baseline.set("request_routing_ms", p50(|s| s.routing));
                baseline.set("request_acquire_ms", p50(|s| s.acquire));
                baseline.set("request_proxy_connect_ms", p50(|s| s.proxy_connect));
                baseline.set("request_backend_ms", p50(|s| s.backend));
                baseline.set("request_ttfb_ms", p50(|s| s.ttfb));
                baseline.set("request_samples", stress.request_spans.len() as f64);
            }
        }

        // 처리량 곡선이 평평해지는 지점. 저사양 환경에서 얼마나 앞으로
        // 당겨지는지가 §50이 보려는 것이다.
        let points: Vec<(usize, f64)> = self
            .sweep
            .iter()
            .map(|row| (row.concurrency, row.summary.rps))
            .collect();
        if let Some(knee) = stats::knee(&points) {
            baseline.set("concurrency_knee", knee as f64);

            if let Some(row) = self.sweep.iter().find(|row| row.concurrency == knee) {
                baseline.set("concurrency_knee_p95_ms", millis(row.summary.p95));
                baseline.set("concurrency_knee_rps", row.summary.rps);
            }
        }

        // CPU는 절대 시간이 아니라 경과 대비 비율로 담는다. 비교 대상이
        // CI runner와 저사양 기기여서 절대값은 나란히 놓기 어렵다.
        if let Some(sustained) = &self.sustained {
            if let Some(used) = cpu_used(&sustained.samples) {
                if !sustained.duration.is_zero() {
                    baseline.set(
                        "sustained_cpu_pct_of_wall",
                        used.as_secs_f64() / sustained.duration.as_secs_f64() * 100.0,
                    );
                }
            }
        }

        // 잠든 발자국. **scale-to-zero의 정상 상태 비용이다.**
        if let Some(sleeping) = &self.sleeping {
            baseline.set("sleeping_apps", sleeping.apps as f64);
            if let Some(rss) = sleeping.rss {
                baseline.set("sleeping_rss_mb", rss as f64 / (1024.0 * 1024.0));
            }
            if let Some(fds) = sleeping.fds {
                baseline.set("sleeping_open_fds", fds as f64);
            }
        }

        Self::set_ratios(&mut baseline);

        baseline
    }

    /// direct 경로로 나눈 비율을 담는다. (PRD §51)
    ///
    /// **기계가 느려지면 절대값은 전부 움직인다.** v0.1.4의 baseline에서 절대
    /// 지표 셋이 문턱을 넘었고 **셋 다 오경보였다** — Core를 지나지 않는
    /// `warm_direct_*`가 같은 방향으로 같은 크기로 움직였다. runner가 느렸다.
    ///
    /// 비율은 그것을 지워 준다. release baseline 다섯 개에서:
    ///
    /// ```text
    /// warm_p50 / direct_p50     2.82 3.25 3.16 3.04 3.07    폭  15%   (절대값 114%)
    /// warm_p95 / direct_p95     1.83 2.37 2.33 2.30 2.15    폭  30%   (절대값 117%)
    /// proxy_connect / direct    0.12 0.14 0.13 0.13 0.13    폭  10%   (절대값 114%)
    /// ```
    ///
    /// **문턱을 걸 수 있는 쪽은 비율이다.** 절대값은 표에 남기지만 폭이 100%를
    /// 넘으므로 경보가 되지 않는다.
    ///
    /// `warm_rps / warm_direct_rps`는 넣지 않는다 — 구조적으로 늘 `1.0`이다.
    fn set_ratios(baseline: &mut Baseline) {
        const RATIOS: &[(&str, &str, &str)] = &[
            ("warm_p50_over_direct", "warm_p50_ms", "warm_direct_p50_ms"),
            ("warm_p95_over_direct", "warm_p95_ms", "warm_direct_p95_ms"),
            (
                "proxy_connect_over_direct",
                "request_proxy_connect_ms",
                "warm_direct_p50_ms",
            ),
        ];

        for (key, numerator, denominator) in RATIOS {
            // 분모가 0이면 비율이 뜻을 갖지 않는다. 담지 않으면 빠진 지표를
            // 알리는 경고가 그것을 이름으로 말한다.
            let Some(bottom) = baseline.get(denominator).filter(|value| *value > 0.0) else {
                continue;
            };
            if let Some(top) = baseline.get(numerator) {
                baseline.set(key, top / bottom);
            }
        }
    }

    /// 돌린 시나리오가 내놓아야 하는데 **없는** 지표들.
    ///
    /// 없으면 그 지표는 `직전 대비`에서 비교되지 않는다. 행이 사라지는 것이
    /// 전부여서 보는 사람은 "변화 없음"과 "재지 못했음"을 구분할 수 없다.
    ///
    /// 빠지는 경로가 실제로 있다. `daemon_rss_mb`와 `sustained_cpu_pct_of_wall`은
    /// `/proc/<pid>/`에서 오고 **기기의 proot는 `/proc` 일부를 막는다.** 개발
    /// 기계와 CI에서는 늘 읽히므로 그 경로를 아무도 보지 않는다 — §50이 둘을
    /// 같은 지표로 비교하라고 하는데, 비교가 가장 중요한 쪽에서 조용히 사라질
    /// 수 있었다.
    ///
    /// **돌지 않은 시나리오의 지표는 세지 않는다.** 처음에 그것까지 셌더니 기본
    /// 실행에서 12개 중 5개가 "없다"고 나왔다 — stress와 sustained를 돌지 않았으니
    /// 당연히 없는 것이다. 상시로 뜨는 경고는 없는 것과 같다.
    #[must_use]
    pub fn unmeasured_metrics(&self, measured: &Baseline) -> Vec<&'static str> {
        baseline::TRACKED
            .iter()
            .map(|(key, _, _, _)| *key)
            .filter(|key| self.scenario_ran(key) && measured.get(key).is_none())
            .collect()
    }

    /// 그 지표를 내놓는 시나리오가 이번 실행에 돌았는가.
    fn scenario_ran(&self, key: &str) -> bool {
        if key.starts_with("cold_start_") {
            return self.stress.is_some();
        }
        if key.starts_with("sleeping_") {
            return self.sleeping.is_some();
        }
        if key.starts_with("sustained_") {
            return self.sustained.is_some();
        }
        if key.starts_with("concurrency_") {
            return !self.sweep.is_empty();
        }
        // proxy connect는 요청 trace에서 온다. `--trace-json`을 켜고 stress를
        // 돌려야 생긴다.
        if key == "proxy_connect_over_direct" {
            return self.stress.is_some();
        }
        // warm 측정은 늘 돈다.
        true
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
        if let Some(crash_loop) = &self.crash_loop {
            problems.extend(crash_loop.leaks());
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

        self.render_sleeping(&mut out);
        self.render_warm(&mut out);
        self.render_cold_start(&mut out);
        self.render_sweep(&mut out);
        self.render_stress(&mut out);
        self.render_sustained(&mut out);
        self.render_crash_loop(&mut out);
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

        // **cold start 분해는 여기서 낸다.** cycle마다 기동이 하나씩 쌓여
        // 표본이 수백 개다. storm은 중복 제거를 증명하려고 spawn을 하나만
        // 만들고, 그 하나는 readiness probe 간격(50ms)에 양자화된다. (PRD §42)
        if !stress.spans.is_empty() {
            let p50 = |pick: fn(&crate::daemon::StartupSpans) -> Duration| {
                let values: Vec<Duration> = stress.spans.iter().map(pick).collect();
                stats::percentile_of(&values, 0.50).unwrap_or_default()
            };
            let median = crate::daemon::StartupSpans {
                queue: p50(|s| s.queue),
                spawn: p50(|s| s.spawn),
                readiness: p50(|s| s.readiness),
                total: p50(|s| s.total),
            };

            let _ = writeln!(out, "구간별 분해 — 표본 {}개의 p50:\n", stress.spans.len());
            let _ = writeln!(out, "| 구간 | p50 | p95 |");
            let _ = writeln!(out, "| --- | ---: | ---: |");
            for (label, pick) in [
                (
                    "actor queue",
                    (|s: &crate::daemon::StartupSpans| s.queue) as fn(&_) -> _,
                ),
                ("process spawn", |s: &crate::daemon::StartupSpans| s.spawn),
                ("app readiness", |s: &crate::daemon::StartupSpans| {
                    s.readiness
                }),
                ("**total**", |s: &crate::daemon::StartupSpans| s.total),
            ] {
                let values: Vec<Duration> = stress.spans.iter().map(pick).collect();
                let _ = writeln!(
                    out,
                    "| {label} | {} | {} |",
                    ms(stats::percentile_of(&values, 0.50).unwrap_or_default()),
                    ms(stats::percentile_of(&values, 0.95).unwrap_or_default()),
                );
            }
            let _ = writeln!(out, "\n지배적인 구간: **{}**\n", dominant(&median));
        }
        // **요청 구간은 기동 분해와 중첩이다.** 섞어 한 표에 두면 거짓 분해가 된다.
        //
        // ```text
        // ttfb
        // ├── routing          gateway 자체
        // ├── acquire          깨우기 — cold start면 위 기동 분해가 이 안에 있다
        // ├── proxy connect    backend와 TCP 연결 수립까지
        // └── backend          app이 응답 header를 만든 시간
        // ```
        //
        // §50이 "spawn / readiness / proxy connect"를 비교 항목으로 적었고,
        // proxy connect는 기동 trace가 아니라 여기 있다.
        if !stress.request_spans.is_empty() {
            let _ = writeln!(
                out,
                "요청 구간 — 표본 {}개. **위 기동 분해는 `acquire` 안에 있다.** (PRD §50)\n\n구간마다 따로 낸 백분위이므로 **더해서 ttfb가 되지 않는다.** 같은 요청의 값이 아니다.\n",
                stress.request_spans.len(),
            );
            let _ = writeln!(out, "| 구간 | p50 | p95 |");
            let _ = writeln!(out, "| --- | ---: | ---: |");
            for (label, pick) in [
                (
                    "routing",
                    (|s: &crate::daemon::RequestSpans| s.routing) as fn(&_) -> _,
                ),
                ("acquire", |s: &crate::daemon::RequestSpans| s.acquire),
                ("proxy connect", |s: &crate::daemon::RequestSpans| {
                    s.proxy_connect
                }),
                ("backend", |s: &crate::daemon::RequestSpans| s.backend),
                ("**ttfb**", |s: &crate::daemon::RequestSpans| s.ttfb),
            ] {
                let values: Vec<Duration> = stress.request_spans.iter().map(pick).collect();
                let _ = writeln!(
                    out,
                    "| {label} | {} | {} |",
                    ms(stats::percentile_of(&values, 0.50).unwrap_or_default()),
                    ms(stats::percentile_of(&values, 0.95).unwrap_or_default()),
                );
            }
            let _ = writeln!(out);
        }

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
        let _ = writeln!(out, "| FD 증가 | {} |", growth_count(fds));
        let _ = writeln!(
            out,
            "| CPU | {} |\n",
            cpu_line(cpu_used(&sustained.samples), sustained.duration),
        );

        let _ = render_trend(out, &sustained.samples);
    }

    /// PRD §33의 주장을 확인한 결과. backoff 없이도 경계가 있는지 본다.
    fn render_crash_loop(&self, out: &mut String) {
        let Some(crash_loop) = &self.crash_loop else {
            return;
        };

        let (rss, fds) = crash_loop.growth();
        let _ = writeln!(
            out,
            "## Crash loop ({}s, {} apps)
",
            crash_loop.duration.as_secs(),
            crash_loop.apps,
        );
        let _ = writeln!(
            out,
            "**초당 기동 시도**가 이 표의 핵심이다. backoff가 내리려는 수치다. (PRD §77)
"
        );
        let _ = writeln!(out, "| | |");
        let _ = writeln!(out, "| --- | ---: |");
        let _ = writeln!(out, "| requests | {} |", crash_loop.requests);
        let _ = writeln!(
            out,
            "| **초당 기동 시도** | {:.1} |",
            crash_loop.spawns_per_second()
        );
        let _ = writeln!(out, "| 시도하고 실패 (502) | {} |", crash_loop.attempted);
        let _ = writeln!(out, "| backoff로 거절 (503) | {} |", crash_loop.backed_off);
        let _ = writeln!(out, "| 502도 503도 아닌 응답 | {} |", crash_loop.unexpected);
        let _ = writeln!(
            out,
            "| 동시 자식 최대 (<= {}) | {} |",
            crash_loop.apps, crash_loop.max_children,
        );
        let _ = writeln!(out, "| 정착 후 zombie | {} |", crash_loop.zombies);
        let _ = writeln!(
            out,
            "| 부하 중 zombie 최대 (판정 안 함) | {} |",
            crash_loop.max_zombies_seen,
        );
        let _ = writeln!(out, "| RSS 증가 | {} |", growth_bytes(rss));
        let _ = writeln!(out, "| FD 증가 | {} |", growth_count(fds));
        let _ = writeln!(
            out,
            "| CPU | {} |\n",
            cpu_line(cpu_used(&crash_loop.samples), crash_loop.duration),
        );

        let _ = render_trend(out, &crash_loop.samples);
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

        // **문턱이 있는데 비교되지 않은 것을 말한다.**
        //
        // 비교는 양쪽 baseline에 다 있어야 성립한다. 이전 것이 옛 코드로
        // 만들어졌으면 비율 행이 통째로 빠지는데, 표만 보면 그 사실이 보이지
        // 않는다 — 아래 설명문은 비율 행이 있다고 말하므로 더 헷갈린다.
        let ungated: Vec<&str> = baseline::TRACKED
            .iter()
            .filter(|(.., noise)| noise.is_some())
            .filter(|(key, metric, ..)| {
                current.get(key).is_some() && !changes.iter().any(|c| c.metric == *metric)
            })
            .map(|(_, metric, ..)| *metric)
            .collect();
        if !ungated.is_empty() {
            let _ = writeln!(
                out,
                "> 이번에 잰 지표 중 **{}개가 비교되지 않았다** — 이전 baseline에 그 값이 없다: {}.\n>\n> 지표를 새로 더한 다음 release에서 한 번 나타나는 상태다. 다음 비교부터 보인다.\n",
                ungated.len(),
                ungated.join(" · "),
            );
        }

        // **잡음 폭을 함께 낸다.** (PRD §73)
        //
        // Δ만 보면 거의 모든 행이 ±50%다 — 12개 지표 중 11개가 그만큼 흔들린다.
        // 옆에 그 지표의 관측 편차를 적어야 읽는 사람이 신호와 잡음을 가른다.
        let _ = writeln!(
            out,
            "| 지표 | {} | current | Δ | runner 편차 |",
            previous.label(),
        );
        let _ = writeln!(out, "| --- | ---: | ---: | ---: | ---: |");

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
                "| {} | {:.2} {} | {:.2} {} | {delta} | {} |",
                change.metric,
                change.previous,
                change.unit,
                change.current,
                change.unit,
                // 문턱이 없으면 그렇게 적는다. 숫자를 적으면 있는 척하는 guard가 된다.
                match change.noise_pct {
                    Some(noise) => format!("±{noise:.0}%"),
                    None => "비교 불가".to_owned(),
                },
            );
        }

        // `(!)`의 뜻이 바뀌었다 — 공통 30%가 아니라 **그 지표의 관측 편차**다.
        let _ = writeln!(
            out,
            "\n`(!)`는 그 지표의 문턱을 넘은 것이고 경고일 뿐이다 — release를 막지 않는다.\n\n`/ direct`가 붙은 행이 **문턱을 걸 수 있는 것들이다.** 기계가 느려지면 절대 지표는 전부 움직이지만 그 비율은 붙어 있다 — `warm_direct_*`는 Core를 지나지 않으므로 같은 기계 효과를 함께 받고, 나누면 지워진다. 문턱은 v0.1.0~v0.1.4의 baseline 다섯 개에서 관측한 **최대 전이의 1.5배**다.\n\n`비교 불가`로 적힌 행은 표에 남기지만 경보하지 않는다. 같은 다섯 baseline에서 폭이 100%를 넘었다 — 문턱을 적으면 있는 척하는 guard가 된다. 그 행이 크게 움직였으면 **같은 행의 `/ direct` 비율을 먼저 본다.** 비율이 조용하면 기계가 느렸던 것이다.\n\n**다른 기계에서 돌린 baseline과 비교하면 절대 지표는 뜻을 갖지 않는다** — 비율은 그래도 비교할 수 있다. (PRD §51)\n"
        );
    }

    /// 이번 실행이 측정한 무릎. sweep이 없으면 `None`.
    fn knee(&self) -> Option<usize> {
        let points: Vec<(usize, f64)> = self
            .sweep
            .iter()
            .map(|row| (row.concurrency, row.summary.rps))
            .collect();
        stats::knee(&points)
    }

    fn render_warm(&self, out: &mut String) {
        let Some(warm) = &self.warm else {
            return;
        };
        let paired = &warm.paired;
        let direct = &paired.direct;
        let proxy = &paired.gateway;

        let _ = writeln!(out, "## Warm path\n");
        let _ = writeln!(
            out,
            "direct backend와 gateway를 **번갈아** 때린다. 두 경로가 같은 worker 수와 \
             같은 순간의 기계 상태를 공유한다. (PRD §50)\n"
        );
        let _ = writeln!(out, "| | direct backend | nimwake |");
        let _ = writeln!(out, "| --- | ---: | ---: |");
        for (label, a, b) in [
            ("p50", direct.p50, proxy.p50),
            ("p95", direct.p95, proxy.p95),
            ("p99", direct.p99, proxy.p99),
        ] {
            let _ = writeln!(out, "| {label} | {} | {} |", ms(a), ms(b));
        }
        let _ = writeln!(
            out,
            "| requests/sec | {:.0} | {:.0} |",
            direct.rps, proxy.rps
        );
        let _ = writeln!(out, "| errors | {} | {} |\n", direct.errors, proxy.errors);

        // **overhead는 쌍마다의 차이에서 낸다.**
        //
        // 백분위를 빼면 서로 다른 두 분포의 순서통계량 차이가 되고, 두 실행의
        // 부하가 다르면 그것은 overhead가 아니다. 기기에서 direct 50 rps 대
        // nimwake 25 rps로 direct 쪽이 더 느려져 **부호까지 뒤집혔다.**
        let _ = writeln!(out, "| overhead (쌍마다의 차이) | |");
        let _ = writeln!(out, "| --- | ---: |");
        let _ = writeln!(out, "| p50 | {} |", signed_ms(paired.overhead_p50_ns));
        let _ = writeln!(out, "| p95 | {} |", signed_ms(paired.overhead_p95_ns));
        let _ = writeln!(out, "| 쌍 | {} |", paired.pairs);
        let _ = writeln!(out, "| gateway가 더 빨랐던 쌍 | {} |\n", paired.inverted);

        // **무릎보다 높은 농도로 재면 뜻이 없다.** (PRD §50)
        //
        // sweep이 뒤에 돌지만 이 시점에 이미 결과가 있다. 기기에서 무릎이 1인데
        // 기본값 8로 재서 direct 쪽이 더 느려졌고 overhead가 음수로 나왔다 —
        // `echo-server` fixture가 그 농도에서 nimwaked보다 느리기 때문이다.
        if let Some(knee) = self.knee() {
            if warm.connections > knee {
                let _ = writeln!(
                    out,
                    "> **overhead를 읽지 않는다.** 동시 연결 {}로 쟀는데 이 기계의 무릎은 {knee}다. 무릎을 넘기면 direct backend 쪽이 먼저 막혀 비교 기준이 병목이 된다. `--warm-connections {knee}`로 다시 잰다.
",
                    warm.connections,
                );
            }
        }

        if paired.pairs > 0 && paired.inverted * 2 > paired.pairs {
            let _ = writeln!(
                out,
                "> **overhead를 읽지 않는다.** 쌍의 절반 이상에서 gateway가 더 빨랐다. \
                 proxy를 한 번 더 거치는데 그럴 수는 없으므로, 기계가 포화되어 \
                 측정이 잡음에 묻혔다는 뜻이다.\n"
            );
        }
        let _ = writeln!(out, "| daemon | |");
        let _ = writeln!(out, "| --- | ---: |");
        let _ = writeln!(out, "| RSS | {} |", megabytes(warm.rss));
        let _ = writeln!(out, "| open fds | {} |\n", optional(warm.fds));
    }

    /// 잠든 app만 있을 때의 발자국. (PRD §50)
    fn render_sleeping(&self, out: &mut String) {
        let Some(sleeping) = &self.sleeping else {
            return;
        };

        let _ = writeln!(out, "## 잠든 발자국\n");
        let _ = writeln!(
            out,
            "app {}개가 **전부 잠든** 상태의 daemon이다. 요청을 보내지 않았다 — scale-to-zero의 정상 상태 비용이다.\n",
            sleeping.apps,
        );
        let _ = writeln!(out, "| | |");
        let _ = writeln!(out, "| --- | ---: |");
        let _ = writeln!(out, "| RSS | {} |", megabytes(sleeping.rss));
        let _ = writeln!(out, "| open fds | {} |", optional(sleeping.fds));
        // **총 RSS를 app 수로 나누지 않는다.** 5 MB쯤의 고정 비용이 섞여
        // "app당 56 KB"처럼 보인다. 두 점의 기울기가 실제 한계 비용이다.
        if let Some(per_app) = sleeping.marginal_kb_per_app() {
            let _ = writeln!(out, "| app당 (기울기) | {per_app:.1} KB |");
        }
        let _ = writeln!(out);
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

        // storm은 기동 중복 제거를 증명하는 것이 목적이고 **spawn이 하나**다.
        // 그 하나로 분해를 적으면 readiness가 probe 간격(50ms)에 양자화되어
        // 0ms 또는 50ms로 튀고, "지배적인 구간"이 실행마다 뒤집힌다.
        // 분해는 lifecycle stress 절에서 표본 수백 개로 낸다. (PRD §42)
        if let Some(spans) = storm.spans.first() {
            let _ = writeln!(
                out,
                "이 storm의 기동 한 건: queue {} · spawn {} · readiness {} · total {}",
                ms(spans.queue),
                ms(spans.spawn),
                ms(spans.readiness),
                ms(spans.total),
            );
            let _ = writeln!(
                out,
                "\n표본이 하나이므로 구간 비교에는 쓰지 않는다. readiness는 probe 간격(50ms)에 양자화된다. 분해는 lifecycle stress 절에 있다.\n"
            );
        }
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

        // 표만으로는 "어디서 평평해지는가"를 눈으로 찾아야 한다. §50의 비교
        // 항목이므로 숫자로 적는다.
        let points: Vec<(usize, f64)> = self
            .sweep
            .iter()
            .map(|row| (row.concurrency, row.summary.rps))
            .collect();
        if let Some(knee) = stats::knee(&points) {
            let _ = writeln!(out);
            if let Some(row) = self.sweep.iter().find(|row| row.concurrency == knee) {
                let _ = writeln!(
                    out,
                    "knee: 동시성 **{knee}** ({:.0} req/s, p95 {}) — 최대 처리량의 95%에 처음 도달한 지점",
                    row.summary.rps,
                    ms(row.summary.p95),
                );
            }
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

/// 부호 있는 나노초를 ms로. `None`은 표본이 없었다는 뜻이다.
fn signed_ms(nanos: Option<i64>) -> String {
    match nanos {
        #[allow(clippy::cast_precision_loss)]
        Some(ns) => format!("{:+.2} ms", ns as f64 / 1_000_000.0),
        None => "—".to_owned(),
    }
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

/// CPU 사용량을 사람이 읽을 형태로. 절대 시간과 경과 대비 비율을 함께 낸다.
///
/// 비교 대상이 CI Linux와 Android 실기기이므로 절대 시간만으로는 읽기 어렵다. (PRD §50)
fn cpu_line(used: Option<std::time::Duration>, elapsed: std::time::Duration) -> String {
    let Some(used) = used else {
        return "n/a".to_owned();
    };

    let ratio = if elapsed.is_zero() {
        0.0
    } else {
        used.as_secs_f64() / elapsed.as_secs_f64() * 100.0
    };

    format!("{:.2}s ({ratio:.0}% of wall)", used.as_secs_f64())
}

fn growth_count(delta: Option<i64>) -> String {
    delta.map_or_else(|| "n/a".to_owned(), |delta| format!("{delta:+}"))
}

/// 추세 그림의 최대 줄 수.
///
/// 표본이 이보다 많으면 솎아 낸다. 24시간 soak의 표본을 전부 적으면 리포트가
/// 읽을 수 없게 길어지고, 추세를 보는 데 그만큼은 필요하지 않다.
const TREND_ROWS: usize = 40;

/// 자원 추이를 간단한 막대로 보여 준다. 숫자 나열보다 추세가 눈에 들어온다.
fn render_trend(out: &mut String, samples: &[Resources]) -> std::fmt::Result {
    let values: Vec<u64> = samples.iter().filter_map(|sample| sample.rss).collect();
    let (Some(min), Some(max)) = (values.iter().min(), values.iter().max()) else {
        return Ok(());
    };

    let step = values.len().div_ceil(TREND_ROWS).max(1);

    writeln!(out, "```text")?;
    writeln!(
        out,
        "RSS {:.1} MB -> {:.1} MB (표본 {}개, {}개마다 한 줄)",
        mb(*min),
        mb(*max),
        values.len(),
        step,
    )?;
    for value in values.iter().step_by(step) {
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
    use crate::stats::Summary;
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

    /// 비율이 분자 ÷ 분모로 담긴다. (PRD §51)
    #[test]
    fn ratios_divide_by_the_direct_path() {
        let mut baseline = Baseline::new("t");
        baseline.set("warm_p50_ms", 1.076);
        baseline.set("warm_direct_p50_ms", 0.351);
        baseline.set("warm_p95_ms", 1.811);
        baseline.set("warm_direct_p95_ms", 0.844);

        Report::set_ratios(&mut baseline);

        // v0.1.4의 실제 값이다. 3.07과 2.15가 나와야 한다.
        let p50 = baseline.get("warm_p50_over_direct").expect("p50 비율");
        let p95 = baseline.get("warm_p95_over_direct").expect("p95 비율");
        assert!((p50 - 1.076 / 0.351).abs() < 1e-9, "{p50}");
        assert!((p95 - 1.811 / 0.844).abs() < 1e-9, "{p95}");
    }

    /// **분모가 없거나 0이면 담지 않는다.**
    ///
    /// 0으로 나눈 `inf`를 담으면 비교 표에 `inf`가 찍히고, 그보다 나쁘게는
    /// 그 지표가 "있다"고 세어져 빠진 지표 경고가 침묵한다.
    #[test]
    fn a_ratio_without_a_denominator_is_not_recorded() {
        for direct in [None, Some(0.0)] {
            let mut baseline = Baseline::new("t");
            baseline.set("warm_p50_ms", 1.0);
            if let Some(value) = direct {
                baseline.set("warm_direct_p50_ms", value);
            }

            Report::set_ratios(&mut baseline);

            assert_eq!(
                baseline.get("warm_p50_over_direct"),
                None,
                "분모가 {direct:?}일 때 비율을 담으면 안 된다",
            );
        }
    }

    /// **느린 기계는 비율을 움직이지 않는다.** (PRD §51)
    ///
    /// v0.1.3과 v0.1.4의 실제 baseline이다. 두 run 사이에 요청 경로의 코드는
    /// 바뀌지 않았고(v0.1.4가 고친 것은 CLI의 오류 판별이다) **절대값은 40%
    /// 넘게 움직였다.** direct 경로도 같이 움직였으므로 원인은 기계다.
    ///
    /// 절대값 문턱으로는 경보가 나고 비율로는 나지 않아야 한다.
    #[test]
    fn a_slower_runner_moves_the_absolute_values_but_not_the_ratio() {
        let mut before = Baseline::new("v0.1.3");
        before.set("warm_p50_ms", 0.769);
        before.set("warm_direct_p50_ms", 0.253);
        let mut after = Baseline::new("v0.1.4");
        after.set("warm_p50_ms", 1.076);
        after.set("warm_direct_p50_ms", 0.351);
        Report::set_ratios(&mut before);
        Report::set_ratios(&mut after);

        let absolute = (after.get("warm_p50_ms").unwrap() - before.get("warm_p50_ms").unwrap())
            / before.get("warm_p50_ms").unwrap()
            * 100.0;
        let ratio = (after.get("warm_p50_over_direct").unwrap()
            - before.get("warm_p50_over_direct").unwrap())
            / before.get("warm_p50_over_direct").unwrap()
            * 100.0;

        assert!(absolute > 35.0, "절대값은 크게 움직였다: {absolute:.0}%");
        assert!(
            ratio.abs() < 5.0,
            "비율은 거의 그대로여야 한다: {ratio:.0}%"
        );

        // 문턱과 함께 보면: 절대값은 자기 폭(114%) 안이고 비율도 자기 폭(15%)
        // 안이다. 중요한 것은 **비율의 폭이 훨씬 좁은데도 들어온다**는 것이다.
        let ratio_noise = noise_of("warm_p50_over_direct");
        assert!(ratio.abs() < ratio_noise, "{ratio:.0}% < {ratio_noise}%");

        // **문턱이 절대값 이동폭보다 좁은데도 비율은 그 안에 든다.** 그것이
        // 비율로 가는 이유다 — 같은 문턱을 절대값에 걸면 울렸을 변화다.
        assert!(
            ratio_noise < absolute,
            "문턱 {ratio_noise}%를 절대값에 걸면 {absolute:.0}% 변화가 울린다 —              비율은 {ratio:.0}%로 그 안에 든다",
        );
    }

    /// **진짜 회귀는 잡는다.** (PRD §51)
    ///
    /// 위 테스트가 "느린 기계는 울리지 않는다"를 본다. 그것만으로는 아무것도
    /// 울리지 않는 문턱도 통과한다. 이쪽이 반대 방향이다 — **기계는 그대로인데
    /// gateway만 느려진 경우.**
    ///
    /// 그런 회귀는 `direct`를 움직이지 않으므로 비율에 그대로 나타난다.
    #[test]
    fn a_real_regression_still_fires() {
        let direct = 0.253;
        let mut before = Baseline::new("before");
        before.set("warm_p50_ms", 0.769);
        before.set("warm_direct_p50_ms", direct);

        // gateway만 30% 느려졌다. 기계는 그대로다.
        let mut after = Baseline::new("after");
        after.set("warm_p50_ms", 0.769 * 1.30);
        after.set("warm_direct_p50_ms", direct);

        Report::set_ratios(&mut before);
        Report::set_ratios(&mut after);

        let changes = baseline::compare(&before, &after, baseline::TRACKED);
        let ratio = changes
            .iter()
            .find(|change| change.metric == "warm p50 / direct")
            .expect("비율이 비교 목록에 있어야 한다");

        assert!(
            ratio.is_regression(),
            "gateway만 30% 느려진 것은 잡아야 한다: {:?} vs 문턱 {:?}",
            ratio.percent(),
            ratio.noise_pct,
        );

        // 절대값 쪽은 **문턱이 없으므로** 울리지 않는다. 그것이 설계다 —
        // 같은 사실을 비율이 말한다.
        let absolute = changes
            .iter()
            .find(|change| change.metric == "warm p50")
            .expect("절대값도 표에는 있어야 한다");
        assert_eq!(absolute.noise_pct, None, "절대값은 비교 불가로 적는다");
        assert!(!absolute.is_regression(), "문턱이 없으면 울리지 않는다");
    }

    /// 그 지표의 문턱. 없으면 테스트를 실패시킨다 — 비율은 문턱이 있어야 한다.
    fn noise_of(key: &str) -> f64 {
        baseline::TRACKED
            .iter()
            .find(|(name, ..)| *name == key)
            .unwrap_or_else(|| panic!("{key}가 TRACKED에 없다"))
            .3
            .unwrap_or_else(|| panic!("{key}에 문턱이 없다 — 비율은 경보할 수 있어야 한다"))
    }
}
