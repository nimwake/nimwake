//! cron 표현식. 다섯 칸만 받는다. (PRD §89)
//!
//! 의존성을 들이지 않는다 — §82의 CIDR과 같은 판단이다. 필요한 것은 "다음 due가
//! 언제인가" 하나뿐이고, 그것은 백 줄 남짓이다.
//!
//! **UTC로만 읽는다.** 지역 시간을 받으면 DST가 들어온다 — 건너뛴 시각은 실행이
//! 사라지고 반복된 시각은 두 번 돈다. 그 두 경우를 정의할 근거가 아직 없다.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 다섯 칸의 cron 표현식.
///
/// 각 칸을 비트 집합으로 담는다. 범위가 작아서 `u64` 하나로 충분하다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
}

/// 칸 하나의 뜻과 허용 범위.
struct Field {
    name: &'static str,
    min: u32,
    max: u32,
}

const FIELDS: [Field; 5] = [
    Field {
        name: "minute",
        min: 0,
        max: 59,
    },
    Field {
        name: "hour",
        min: 0,
        max: 23,
    },
    Field {
        name: "day of month",
        min: 1,
        max: 31,
    },
    Field {
        name: "month",
        min: 1,
        max: 12,
    },
    Field {
        name: "day of week",
        min: 0,
        max: 6,
    },
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CronError {
    #[error(
        "`{raw}`: a cron expression needs 5 fields (minute hour day month weekday), got {got}"
    )]
    FieldCount { raw: String, got: usize },

    #[error("`{raw}`: {field} `{part}` is malformed")]
    Malformed {
        raw: String,
        field: &'static str,
        part: String,
    },

    #[error("`{raw}`: {field} `{part}` is outside {min}-{max}")]
    OutOfRange {
        raw: String,
        field: &'static str,
        part: String,
        min: u32,
        max: u32,
    },

    #[error("`{raw}`: a step of 0 never matches")]
    ZeroStep { raw: String },

    /// **표준 cron은 이 경우를 OR로 읽는다.** AND로 읽는 사람이 많다.
    ///
    /// `0 3 1 * 1`은 "매월 1일 **또는** 매주 월요일"이고, AND로 기대한 사람에게는
    /// 실행이 네 배로 늘어난다. 조용히 넓어지는 쪽으로 틀리지 않는다.
    #[error(
        "`{raw}`: restricting both day-of-month and day-of-week is refused — \
         standard cron reads it as OR, not AND, which surprises most readers. \
         Split the application or pick one field (PRD §89)"
    )]
    BothDayFields { raw: String },
}

impl Cron {
    /// 표현식을 읽는다. 틀리면 기동을 막는다. (PRD §89)
    pub fn parse(raw: &str) -> Result<Self, CronError> {
        let parts: Vec<&str> = raw.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(CronError::FieldCount {
                raw: raw.to_owned(),
                got: parts.len(),
            });
        }

        let mut sets = [0_u64; 5];
        for (index, part) in parts.iter().enumerate() {
            sets[index] = parse_field(raw, &FIELDS[index], part)?;
        }

        // 일과 요일을 **동시에** 제한하면 거부한다.
        let day_restricted = parts[2] != "*";
        let weekday_restricted = parts[4] != "*";
        if day_restricted && weekday_restricted {
            return Err(CronError::BothDayFields {
                raw: raw.to_owned(),
            });
        }

        Ok(Self {
            minutes: sets[0],
            hours: sets[1],
            days: sets[2],
            months: sets[3],
            weekdays: sets[4],
        })
    }

    /// `after`보다 **뒤**인 가장 이른 due. UTC.
    ///
    /// 분 단위로 훑는다. 가장 드문 일정(2월 29일 같은 것은 받지 않으므로)도 네 해
    /// 안에 반드시 걸리고, 분 단위 반복은 최악에도 백만 번 남짓이다 — 기동 때 한
    /// 번, 그리고 due마다 한 번만 부른다.
    #[must_use]
    pub fn next_after(&self, after: SystemTime) -> Option<SystemTime> {
        let start = after.duration_since(UNIX_EPOCH).ok()?.as_secs();
        // 다음 분의 0초부터 본다. 같은 분에 두 번 돌지 않는다.
        let mut minute = start / 60 + 1;

        // 네 해 + 하루. 그 안에 없으면 표현식이 영영 맞지 않는 것이다.
        let limit = minute + 4 * 366 * 24 * 60;
        while minute < limit {
            let stamp = Timestamp::from_minute(minute);
            if self.matches(&stamp) {
                return Some(UNIX_EPOCH + Duration::from_secs(minute * 60));
            }
            minute += 1;
        }
        None
    }

    fn matches(&self, at: &Timestamp) -> bool {
        if bit(self.minutes, at.minute) && bit(self.hours, at.hour) && bit(self.months, at.month) {
            // 일과 요일은 **둘 다 제한될 수 없다**(parse에서 거부한다). 그래서
            // 여기서는 각자 검사하면 되고 표준 cron의 OR 문제가 생기지 않는다.
            bit(self.days, at.day) && bit(self.weekdays, at.weekday)
        } else {
            false
        }
    }
}

impl fmt::Display for Cron {
    /// 사람이 읽는 형태. 원문을 담지 않으므로 비트에서 되살린다.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} {} {} {} {}",
            render(self.minutes, &FIELDS[0]),
            render(self.hours, &FIELDS[1]),
            render(self.days, &FIELDS[2]),
            render(self.months, &FIELDS[3]),
            render(self.weekdays, &FIELDS[4]),
        )
    }
}

impl<'de> serde::Deserialize<'de> for Cron {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

const fn bit(set: u64, value: u32) -> bool {
    set & (1_u64 << value) != 0
}

fn render(set: u64, field: &Field) -> String {
    if (field.min..=field.max).all(|value| bit(set, value)) {
        return "*".to_owned();
    }
    (field.min..=field.max)
        .filter(|value| bit(set, *value))
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// 칸 하나를 비트 집합으로.
///
/// `*` · `5` · `1,3,5` · `1-5` · `*/15` · `1-20/5`를 받는다.
fn parse_field(raw: &str, field: &Field, part: &str) -> Result<u64, CronError> {
    let malformed = || CronError::Malformed {
        raw: raw.to_owned(),
        field: field.name,
        part: part.to_owned(),
    };

    let mut set = 0_u64;
    for item in part.split(',') {
        if item.is_empty() {
            return Err(malformed());
        }

        let (spec, step) = match item.split_once('/') {
            Some((spec, step)) => {
                let step: u32 = step.parse().map_err(|_| malformed())?;
                if step == 0 {
                    return Err(CronError::ZeroStep {
                        raw: raw.to_owned(),
                    });
                }
                (spec, step)
            }
            None => (item, 1),
        };

        let (lo, hi) = if spec == "*" {
            (field.min, field.max)
        } else if let Some((lo, hi)) = spec.split_once('-') {
            (
                parse_value(raw, field, part, lo)?,
                parse_value(raw, field, part, hi)?,
            )
        } else {
            let value = parse_value(raw, field, part, spec)?;
            // `5/15`는 "5부터 끝까지 15 간격"이다 — 표준 cron과 같다.
            if step == 1 {
                (value, value)
            } else {
                (value, field.max)
            }
        };

        if lo > hi {
            return Err(malformed());
        }
        let mut value = lo;
        while value <= hi {
            set |= 1_u64 << value;
            value += step;
        }
    }

    if set == 0 {
        return Err(malformed());
    }
    Ok(set)
}

fn parse_value(raw: &str, field: &Field, part: &str, text: &str) -> Result<u32, CronError> {
    let value: u32 = text.trim().parse().map_err(|_| CronError::Malformed {
        raw: raw.to_owned(),
        field: field.name,
        part: part.to_owned(),
    })?;
    if value < field.min || value > field.max {
        return Err(CronError::OutOfRange {
            raw: raw.to_owned(),
            field: field.name,
            part: part.to_owned(),
            min: field.min,
            max: field.max,
        });
    }
    Ok(value)
}

/// UTC로 쪼갠 시각. 분 단위다.
///
/// `chrono`를 들이지 않는다. 필요한 것은 "이 분이 표현식에 맞나"뿐이고, 그것은
/// 윤년 규칙 하나로 끝난다.
struct Timestamp {
    minute: u32,
    hour: u32,
    day: u32,
    month: u32,
    weekday: u32,
}

impl Timestamp {
    /// epoch 이후의 분 수에서 UTC 날짜를 낸다.
    fn from_minute(total_minutes: u64) -> Self {
        let minute = u32::try_from(total_minutes % 60).unwrap_or(0);
        let total_hours = total_minutes / 60;
        let hour = u32::try_from(total_hours % 24).unwrap_or(0);
        let mut days = total_hours / 24;

        // 1970-01-01은 목요일이다.
        let weekday = u32::try_from((days + 4) % 7).unwrap_or(0);

        let mut year = 1970_u64;
        loop {
            let in_year = if is_leap(year) { 366 } else { 365 };
            if days < in_year {
                break;
            }
            days -= in_year;
            year += 1;
        }

        let lengths = month_lengths(year);
        let mut month = 1_u32;
        for length in lengths {
            if days < length {
                break;
            }
            days -= length;
            month += 1;
        }

        Self {
            minute,
            hour,
            day: u32::try_from(days).unwrap_or(0) + 1,
            month,
            weekday,
        }
    }
}

const fn is_leap(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

const fn month_lengths(year: u64) -> [u64; 12] {
    [
        31,
        if is_leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> SystemTime {
        // "YYYY-MM-DD HH:MM" UTC를 SystemTime으로. 테스트 전용이다.
        let (date, time) = text.split_once(' ').expect("date and time");
        let mut date = date.split('-');
        let year: u64 = date.next().unwrap().parse().unwrap();
        let month: u64 = date.next().unwrap().parse().unwrap();
        let day: u64 = date.next().unwrap().parse().unwrap();
        let mut time = time.split(':');
        let hour: u64 = time.next().unwrap().parse().unwrap();
        let minute: u64 = time.next().unwrap().parse().unwrap();

        let mut days = 0_u64;
        for y in 1970..year {
            days += if is_leap(y) { 366 } else { 365 };
        }
        for (index, length) in month_lengths(year).iter().enumerate() {
            if u64::try_from(index).unwrap() + 1 >= month {
                break;
            }
            days += length;
        }
        days += day - 1;
        UNIX_EPOCH + Duration::from_secs(((days * 24 + hour) * 60 + minute) * 60)
    }

    fn render_next(expr: &str, from: &str) -> String {
        let cron = Cron::parse(expr).unwrap_or_else(|error| panic!("`{expr}`: {error}"));
        let next = cron.next_after(at(from)).expect("a due exists");
        let minutes = next.duration_since(UNIX_EPOCH).unwrap().as_secs() / 60;
        let stamp = Timestamp::from_minute(minutes);
        // 연도는 되살리지 않는다 — 테스트가 보려는 것은 월·일·시·분이다.
        format!(
            "{:02}-{:02} {:02}:{:02}",
            stamp.month, stamp.day, stamp.hour, stamp.minute
        )
    }

    /// 매일 한 번. (PRD §89)
    #[test]
    fn a_daily_schedule_finds_the_next_day() {
        assert_eq!(render_next("0 18 * * *", "2026-03-10 17:59"), "03-10 18:00");
        assert_eq!(render_next("0 18 * * *", "2026-03-10 18:00"), "03-11 18:00");
        assert_eq!(render_next("0 18 * * *", "2026-03-10 18:01"), "03-11 18:00");
    }

    /// 같은 분에 두 번 돌지 않는다. **`after`보다 뒤**를 돌려준다.
    #[test]
    fn the_same_minute_never_fires_twice() {
        let cron = Cron::parse("*/5 * * * *").expect("parses");
        let first = cron.next_after(at("2026-03-10 12:00")).expect("a due");
        let second = cron.next_after(first).expect("a due");
        assert!(second > first, "{first:?} → {second:?}");
        assert_eq!(
            second.duration_since(first).unwrap(),
            Duration::from_secs(300),
        );
    }

    /// 목록 · 범위 · 간격.
    #[test]
    fn lists_ranges_and_steps() {
        assert_eq!(
            render_next("0,30 * * * *", "2026-01-01 00:05"),
            "01-01 00:30"
        );
        assert_eq!(
            render_next("0 9-17 * * *", "2026-01-01 08:30"),
            "01-01 09:00"
        );
        assert_eq!(
            render_next("*/15 * * * *", "2026-01-01 00:01"),
            "01-01 00:15"
        );
        assert_eq!(
            render_next("0 0 1-20/5 * *", "2026-01-02 00:00"),
            "01-06 00:00"
        );
    }

    /// 요일만 제한. 1970-01-01이 목요일인 것에 기댄다.
    #[test]
    fn a_weekday_schedule_lands_on_that_weekday() {
        // 2026-03-10은 화요일이다. 다음 월요일은 03-16.
        assert_eq!(render_next("0 3 * * 1", "2026-03-10 00:00"), "03-16 03:00");
        // 일요일은 0이다. 2026-03-15.
        assert_eq!(render_next("0 3 * * 0", "2026-03-10 00:00"), "03-15 03:00");
    }

    /// 윤년의 2월을 넘어간다.
    #[test]
    fn it_crosses_a_leap_february() {
        // 2028은 윤년이다. 2월 29일이 있다.
        assert_eq!(render_next("0 0 29 2 *", "2028-01-01 00:00"), "02-29 00:00");
        // 2026은 아니다 — 다음 2월 29일은 2028이므로 월·일만 보면 같다.
        assert_eq!(render_next("0 0 29 2 *", "2026-03-01 00:00"), "02-29 00:00");
    }

    /// **일과 요일을 동시에 제한하면 거부한다.** (PRD §89)
    #[test]
    fn restricting_both_day_fields_is_refused() {
        let error = Cron::parse("0 3 1 * 1").expect_err("refused");
        assert!(matches!(error, CronError::BothDayFields { .. }), "{error}");

        // 한쪽만 제한하는 것은 받는다.
        assert!(Cron::parse("0 3 1 * *").is_ok());
        assert!(Cron::parse("0 3 * * 1").is_ok());
    }

    /// 틀린 표현식은 거부한다. 조용히 건너뛰지 않는다. (PRD §89)
    #[test]
    fn malformed_expressions_are_refused() {
        for (expr, expected) in [
            ("0 18 * *", "FieldCount"),
            ("0 18 * * * *", "FieldCount"),
            ("60 * * * *", "OutOfRange"),
            ("* 24 * * *", "OutOfRange"),
            ("* * 0 * *", "OutOfRange"),
            ("* * * 13 *", "OutOfRange"),
            ("* * * * 7", "OutOfRange"),
            ("*/0 * * * *", "ZeroStep"),
            ("a * * * *", "Malformed"),
            ("5-1 * * * *", "Malformed"),
            ("1,,2 * * * *", "Malformed"),
            ("@daily", "FieldCount"),
        ] {
            let error = Cron::parse(expr).expect_err("거부되어야 한다");
            let name = match error {
                CronError::FieldCount { .. } => "FieldCount",
                CronError::OutOfRange { .. } => "OutOfRange",
                CronError::ZeroStep { .. } => "ZeroStep",
                CronError::Malformed { .. } => "Malformed",
                CronError::BothDayFields { .. } => "BothDayFields",
            };
            assert_eq!(name, expected, "`{expr}`");
        }
    }

    /// 오류 메시지가 **어느 칸인지** 말한다.
    ///
    /// 다섯 칸 중 하나가 틀렸을 때 "표현식이 틀렸다"만 말하면 운영자가 다섯을
    /// 다 본다.
    #[test]
    fn the_error_names_the_field() {
        let error = Cron::parse("* 24 * * *").expect_err("refused");
        let message = error.to_string();
        assert!(message.contains("hour"), "{message}");
        assert!(message.contains("0-23"), "{message}");
    }
}
