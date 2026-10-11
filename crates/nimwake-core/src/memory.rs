//! 시스템 메모리 가용량.
//!
//! 요구사항: PRD §78 Memory Admission Control.
//!
//! `MemAvailable`을 쓴다. `MemFree`가 아니다. `MemFree`는 page cache로 쓰이지
//! 않은 양만 센다. Linux는 남는 메모리를 cache로 쓰므로 `MemFree`는 정상
//! 상태에서도 작고, 그것으로 판정하면 **멀쩡한 기기에서 계속 거절한다.**

/// `MemAvailable`을 MiB로 돌려준다.
///
/// 읽을 수 없거나 항목이 없으면 `None`이다. 호출하는 쪽은 **통과시킨다.**
/// 판독 실패로 서비스를 멈추지 않는다. (PRD §78)
#[cfg(target_os = "linux")]
pub(crate) fn available_mb() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_available_mb(&text)
}

/// Linux가 아니면 판정하지 않는다.
///
/// daemon은 Unix 전용이지만(§19) `/proc/meminfo`는 Linux의 것이다. macOS와 BSD에는
/// 없다. 다른 방법을 추측하는 대신 검사를 끈다.
#[cfg(not(target_os = "linux"))]
pub(crate) fn available_mb() -> Option<u64> {
    None
}

/// `/proc/meminfo` 본문에서 `MemAvailable`을 MiB로 뽑는다.
///
/// 형식은 이렇다. 단위는 항상 `kB`이고 커널이 그렇게 고정해 두었다.
///
/// ```text
/// MemTotal:       16284016 kB
/// MemAvailable:   15597656 kB
/// ```
fn parse_available_mb(text: &str) -> Option<u64> {
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("MemAvailable:") else {
            continue;
        };

        let mut parts = rest.split_whitespace();
        let value: u64 = parts.next()?.parse().ok()?;

        // 단위가 `kB`가 아니면 환산을 추측하지 않는다. 틀린 숫자보다 없는 것이 낫다.
        if parts.next()? != "kB" {
            return None;
        }

        return Some(value / 1024);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "MemTotal:       16284016 kB
MemFree:        14378608 kB
MemAvailable:   15597656 kB
Buffers:           35764 kB
";

    #[test]
    fn it_reads_mem_available_not_mem_free() {
        // 15597656 kB = 15232 MiB. MemFree(14378608 kB = 14041 MiB)가 아니다.
        assert_eq!(parse_available_mb(SAMPLE), Some(15232));
    }

    /// `MemFree`를 보면 멀쩡한 기기에서 계속 거절한다. (PRD §78)
    ///
    /// cache가 가득한 상태를 흉내낸다 — `MemFree`는 작고 `MemAvailable`은 크다.
    #[test]
    fn a_full_page_cache_does_not_look_like_pressure() {
        let text = "MemTotal:        5452048 kB
MemFree:          120000 kB
MemAvailable:    2671192 kB
";
        let available = parse_available_mb(text).expect("MemAvailable is present");
        assert_eq!(available, 2608, "MemAvailable 기준이어야 한다");
        assert!(
            available > 1000,
            "MemFree(117MiB)를 보면 압박으로 읽힌다: {available}",
        );
    }

    #[test]
    fn a_missing_entry_is_none() {
        assert_eq!(parse_available_mb("MemTotal: 16284016 kB\n"), None);
        assert_eq!(parse_available_mb(""), None);
    }

    /// 단위를 추측하지 않는다. 틀린 숫자보다 없는 것이 낫다.
    #[test]
    fn an_unexpected_unit_is_none() {
        assert_eq!(parse_available_mb("MemAvailable:   15597656 MB\n"), None);
        assert_eq!(parse_available_mb("MemAvailable:   15597656\n"), None);
    }

    #[test]
    fn a_malformed_value_is_none() {
        assert_eq!(parse_available_mb("MemAvailable:   many kB\n"), None);
        assert_eq!(parse_available_mb("MemAvailable:\n"), None);
    }

    /// `MemAvailable`로 시작하는 다른 항목에 걸리지 않는다.
    #[test]
    fn it_matches_the_exact_key() {
        // 접두사가 같은 가상의 항목. `strip_prefix`가 콜론까지 보므로 걸리지 않는다.
        let text = "MemAvailableSomething:   1 kB
MemAvailable:   2097152 kB
";
        assert_eq!(parse_available_mb(text), Some(2048));
    }

    /// 실제 기기에서 읽힌다. (Linux에서만)
    #[cfg(target_os = "linux")]
    #[test]
    fn it_reads_this_machine() {
        let available = available_mb().expect("/proc/meminfo is readable on Linux");
        assert!(available > 0, "가용량이 0일 수는 없다");
    }
}
