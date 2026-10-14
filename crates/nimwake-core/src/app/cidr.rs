//! IP 대역 하나. (PRD §82)
//!
//! **접근 제어에 쓰는 코드다.** 틀리면 조용히 넓어지거나 조용히 좁아진다. 그래서
//! 의존성을 들이지 않고 직접 쓰되, 경계값을 전부 테스트한다 — prefix `0`,
//! prefix 전체 길이, byte 경계, 주소 계열이 다른 경우, IPv4로 맞춘 IPv6.
//!
//! `ipnet` 같은 crate를 쓰지 않은 이유: 이 workspace의 의존성은 전부 기반
//! 설비(hyper · tokio · serde · toml)뿐이고 유틸리티 crate가 하나도 없다.
//! 60줄을 위해 그 방침을 깨지 않는다.

use std::fmt;
use std::net::IpAddr;

/// `192.168.0.0/16` 같은 대역 하나.
///
/// **host bit는 비어 있어야 한다.** `192.168.0.9/16`은 거부한다 — 아래
/// [`Cidr::parse`]를 보라.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    network: IpAddr,
    prefix: u8,
}

/// CIDR 문법 오류. 기동을 막는다. (PRD §82)
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CidrError {
    #[error("`{raw}` is not an IP address or CIDR range")]
    Malformed { raw: String },
    #[error("`{raw}` has prefix /{prefix}, but {family} allows at most /{max}")]
    PrefixTooLong {
        raw: String,
        prefix: u8,
        family: &'static str,
        max: u8,
    },
    /// `192.168.0.9/16`처럼 prefix 밖에 bit가 켜져 있는 경우.
    ///
    /// **묵인하지 않고 거부한다.** 묵인하려면 host bit를 지워야 하는데, 그것은
    /// 쓴 사람이 "이 주소 하나"를 의도했을 때 **대역 전체를 여는 것**이다.
    /// 접근 제어에서 조용히 넓어지는 쪽으로 틀리면 안 된다.
    #[error("`{raw}` has bits set outside its /{prefix} prefix; did you mean `{suggestion}`?")]
    HostBitsSet {
        raw: String,
        prefix: u8,
        suggestion: String,
    },
    /// `::ffff:192.168.0.0/112`처럼 IPv4로 맞춘 IPv6를 그대로 쓴 경우.
    ///
    /// **거부한다.** IPv4로 정규화하면 prefix의 의미가 조용히 바뀌고(IPv6의
    /// `/112`가 IPv4의 `/112`일 수 없다), 정규화하지 않으면 peer 쪽만 IPv4로
    /// 되돌아가 영영 맞지 않는다. 어느 쪽도 쓴 사람의 뜻과 다르다.
    #[error("`{raw}` is an IPv4-mapped IPv6 address; write it as `{suggestion}` instead")]
    Ipv4MappedLiteral { raw: String, suggestion: String },
}

impl Cidr {
    /// `192.168.0.0/16` · `10.0.0.0/8` · `127.0.0.1` · `::1`을 읽는다.
    ///
    /// prefix를 생략하면 주소 하나다 (IPv4 `/32`, IPv6 `/128`).
    ///
    /// # Errors
    ///
    /// 문법이 틀리거나, prefix가 주소 계열의 길이를 넘거나, prefix 밖에 bit가
    /// 켜져 있으면 오류다.
    pub fn parse(raw: &str) -> Result<Self, CidrError> {
        let raw = raw.trim();
        let malformed = || CidrError::Malformed {
            raw: raw.to_owned(),
        };

        let (address_part, prefix_part) = match raw.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (raw, None),
        };

        let address: IpAddr = address_part.parse().map_err(|_| malformed())?;

        // **IPv4로 맞춘 IPv6 literal은 거부한다.**
        //
        // 정규화하면 `/112`가 IPv4의 prefix로 재해석되어 뜻이 조용히 바뀐다.
        // 정규화하지 않으면 peer 쪽만 IPv4로 되돌아가 영영 맞지 않는다.
        // 둘 다 틀리므로 쓴 사람에게 IPv4로 쓰라고 말한다.
        if let IpAddr::V6(v6) = address {
            if let Some(v4) = v6.to_ipv4_mapped() {
                let suggestion = match prefix_part {
                    // IPv6 prefix에서 IPv4 prefix를 추측하지 않는다. 주소만 보인다.
                    Some(_) => format!("{v4}/<prefix>"),
                    None => v4.to_string(),
                };
                return Err(CidrError::Ipv4MappedLiteral {
                    raw: raw.to_owned(),
                    suggestion,
                });
            }
        }

        let max = match address {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };

        let prefix = match prefix_part {
            None => max,
            Some(text) => {
                // `/08`이나 `/+8`을 받지 않는다. 숫자만 받는다.
                if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(malformed());
                }
                let prefix: u8 = text.parse().map_err(|_| malformed())?;
                if prefix > max {
                    return Err(CidrError::PrefixTooLong {
                        raw: raw.to_owned(),
                        prefix,
                        family: if max == 32 { "IPv4" } else { "IPv6" },
                        max,
                    });
                }
                prefix
            }
        };

        let masked = mask(address, prefix);
        if masked != address {
            return Err(CidrError::HostBitsSet {
                raw: raw.to_owned(),
                prefix,
                suggestion: format!("{masked}/{prefix}"),
            });
        }

        Ok(Self {
            network: address,
            prefix,
        })
    }

    /// 이 대역이 주소를 품는가.
    ///
    /// 주소 계열이 다르면 `false`다. IPv4 대역은 IPv6 주소를 품지 않는다 —
    /// IPv4로 맞춘 IPv6는 비교 전에 IPv4로 되돌리므로 예외가 아니다.
    #[must_use]
    pub fn contains(&self, address: IpAddr) -> bool {
        let address = normalize(address);
        match (self.network, address) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                prefix_matches(&network.octets(), &address.octets(), self.prefix)
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                prefix_matches(&network.octets(), &address.octets(), self.prefix)
            }
            _ => false,
        }
    }
}

impl fmt::Display for Cidr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.network, self.prefix)
    }
}

impl<'de> serde::Deserialize<'de> for Cidr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// `::ffff:192.168.0.9` → `192.168.0.9`.
///
/// gateway가 `::`에 bind하면 LAN의 IPv4 접속이 이 형태로 도착한다. 되돌리지
/// 않으면 `192.168.0.0/16` 규칙이 조용히 어긋난다. (PRD §82)
///
/// `to_ipv4()`가 아니라 `to_ipv4_mapped()`를 쓴다 — 전자는 `::1`까지
/// `0.0.0.1`로 바꾼다.
fn normalize(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    }
}

/// prefix 길이만큼 host bit를 지운 주소.
fn mask(address: IpAddr, prefix: u8) -> IpAddr {
    match address {
        IpAddr::V4(v4) => {
            let mut octets = v4.octets();
            mask_octets(&mut octets, prefix);
            IpAddr::V4(octets.into())
        }
        IpAddr::V6(v6) => {
            let mut octets = v6.octets();
            mask_octets(&mut octets, prefix);
            IpAddr::V6(octets.into())
        }
    }
}

fn mask_octets(octets: &mut [u8], prefix: u8) {
    let whole = usize::from(prefix / 8);
    let remainder = prefix % 8;

    if remainder > 0 && whole < octets.len() {
        // `0xff << (8 - remainder)`는 remainder가 1..=7일 때만 안전하다.
        // remainder == 0이면 shift가 8이 되어 overflow다. 그래서 분기한다.
        octets[whole] &= 0xff_u8 << (8 - remainder);
    }

    let length = octets.len();
    let tail_start = (whole + usize::from(remainder > 0)).min(length);
    for octet in &mut octets[tail_start..] {
        *octet = 0;
    }
}

/// 앞에서부터 `prefix` bit가 같은가.
fn prefix_matches(network: &[u8], address: &[u8], prefix: u8) -> bool {
    debug_assert_eq!(network.len(), address.len());

    let whole = usize::from(prefix / 8);
    let remainder = prefix % 8;

    if network[..whole] != address[..whole] {
        return false;
    }
    if remainder == 0 {
        // prefix가 byte 경계에 딱 맞는다. `network[whole]`을 보면 안 된다 —
        // prefix가 전체 길이면 범위를 벗어난다.
        return true;
    }

    let mask = 0xff_u8 << (8 - remainder);
    network[whole] & mask == address[whole] & mask
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(raw: &str) -> IpAddr {
        raw.parse().expect("test address parses")
    }

    fn cidr(raw: &str) -> Cidr {
        Cidr::parse(raw).unwrap_or_else(|error| panic!("`{raw}` should parse: {error}"))
    }

    /// 흔한 LAN 대역. (PRD §82)
    #[test]
    fn it_matches_a_lan_range() {
        let lan = cidr("192.168.0.0/16");
        assert!(lan.contains(ip("192.168.0.1")));
        assert!(lan.contains(ip("192.168.255.255")));
        assert!(!lan.contains(ip("192.169.0.1")));
        assert!(!lan.contains(ip("10.0.0.1")));
    }

    /// **`::ffff:192.168.0.9`를 `192.168.0.9`로 본다.** (PRD §82)
    ///
    /// gateway가 `::`에 bind하면 LAN 접속이 이 형태로 온다. 이것을 놓치면
    /// LAN 규칙이 LAN 요청을 막는다.
    #[test]
    fn an_ipv4_mapped_address_matches_an_ipv4_range() {
        assert!(cidr("192.168.0.0/16").contains(ip("::ffff:192.168.0.9")));
        assert!(cidr("127.0.0.1").contains(ip("::ffff:127.0.0.1")));
        assert!(!cidr("192.168.0.0/16").contains(ip("::ffff:10.0.0.1")));
    }

    /// `::1`은 `127.0.0.1`이 아니다.
    ///
    /// `to_ipv4()`를 썼다면 `::1`이 `0.0.0.1`이 되어 엉뚱한 비교를 한다.
    #[test]
    fn ipv6_loopback_is_not_ipv4_loopback() {
        assert!(!cidr("127.0.0.0/8").contains(ip("::1")));
        assert!(cidr("::1").contains(ip("::1")));
    }

    /// 주소 계열이 다르면 품지 않는다.
    #[test]
    fn address_families_do_not_mix() {
        assert!(!cidr("192.168.0.0/16").contains(ip("fd00::1")));
        assert!(!cidr("fd00::/8").contains(ip("192.168.0.1")));
    }

    /// prefix `0`은 그 계열 전체다.
    #[test]
    fn a_zero_prefix_matches_everything_in_its_family() {
        assert!(cidr("0.0.0.0/0").contains(ip("1.2.3.4")));
        assert!(cidr("0.0.0.0/0").contains(ip("192.168.0.1")));
        // IPv4로 맞춘 IPv6는 IPv4로 되돌아가므로 품는다.
        assert!(cidr("0.0.0.0/0").contains(ip("::ffff:1.2.3.4")));
        // 진짜 IPv6는 품지 않는다.
        assert!(!cidr("0.0.0.0/0").contains(ip("fd00::1")));
    }

    /// prefix가 전체 길이면 주소 하나다. **범위를 벗어나 읽지 않는다.**
    #[test]
    fn a_full_prefix_matches_exactly_one_address() {
        let one = cidr("10.1.2.3/32");
        assert!(one.contains(ip("10.1.2.3")));
        assert!(!one.contains(ip("10.1.2.4")));

        let v6 = cidr("fd00::1/128");
        assert!(v6.contains(ip("fd00::1")));
        assert!(!v6.contains(ip("fd00::2")));
    }

    /// prefix를 생략하면 주소 하나다.
    #[test]
    fn a_bare_address_is_a_single_host() {
        assert_eq!(cidr("127.0.0.1"), cidr("127.0.0.1/32"));
        assert_eq!(cidr("::1"), cidr("::1/128"));
    }

    /// byte 경계가 아닌 prefix. 1 bit 차이를 가른다.
    #[test]
    fn it_handles_prefixes_inside_a_byte() {
        let half = cidr("10.0.0.0/9");
        assert!(half.contains(ip("10.0.0.1")));
        assert!(half.contains(ip("10.127.255.255")));
        // 10.128.0.0은 9번째 bit가 다르다.
        assert!(!half.contains(ip("10.128.0.0")));

        let seven = cidr("10.0.0.0/7");
        assert!(seven.contains(ip("11.0.0.1")));
        assert!(!seven.contains(ip("12.0.0.1")));
    }

    /// **host bit가 켜져 있으면 거부한다.** (PRD §82)
    ///
    /// `192.168.0.9/16`을 묵인해서 host bit를 지우면, "이 주소 하나"를 의도한
    /// 사람에게 대역 전체를 열어 준다.
    #[test]
    fn host_bits_outside_the_prefix_are_rejected() {
        let error = Cidr::parse("192.168.0.9/16").expect_err("host bits must be rejected");
        assert!(
            matches!(error, CidrError::HostBitsSet { ref suggestion, .. } if suggestion == "192.168.0.0/16"),
            "{error}",
        );
        // 고친 쪽은 통과한다.
        assert!(Cidr::parse("192.168.0.0/16").is_ok());
        assert!(Cidr::parse("192.168.0.9/32").is_ok());
    }

    /// prefix가 계열의 길이를 넘으면 거부한다.
    #[test]
    fn a_prefix_longer_than_the_family_is_rejected() {
        assert!(matches!(
            Cidr::parse("10.0.0.0/33"),
            Err(CidrError::PrefixTooLong { max: 32, .. })
        ));
        assert!(matches!(
            Cidr::parse("fd00::/129"),
            Err(CidrError::PrefixTooLong { max: 128, .. })
        ));
    }

    /// 문법이 아닌 것을 거부한다.
    #[test]
    fn malformed_input_is_rejected() {
        for raw in [
            "",
            "not-an-address",
            "192.168.0.0/",
            "192.168.0.0/abc",
            "192.168.0.0/16/24",
            "192.168.0.256",
            "192.168.0.0 /16",
            "/16",
        ] {
            assert!(
                Cidr::parse(raw).is_err(),
                "`{raw}`는 거부해야 한다: {:?}",
                Cidr::parse(raw),
            );
        }
    }

    /// **IPv4로 맞춘 IPv6 literal은 거부한다.** (PRD §82)
    ///
    /// 받아들이면 둘 중 하나로 틀린다 — prefix를 재해석하거나(`/112`가 IPv4의
    /// prefix가 될 수 없다), peer만 IPv4로 되돌아가 영영 맞지 않거나.
    #[test]
    fn an_ipv4_mapped_literal_is_rejected() {
        let error = Cidr::parse("::ffff:192.168.0.0/112").expect_err("mapped literal is rejected");
        assert!(
            matches!(error, CidrError::Ipv4MappedLiteral { .. }),
            "{error}",
        );
        // 주소만 쓴 경우에도 거부하고, IPv4 형태를 알려 준다.
        let error = Cidr::parse("::ffff:127.0.0.1").expect_err("mapped literal is rejected");
        assert!(
            matches!(error, CidrError::Ipv4MappedLiteral { ref suggestion, .. } if suggestion == "127.0.0.1"),
            "{error}",
        );
    }

    /// 사람이 읽는 형태로 다시 쓴다. 로그에 쓰인다.
    #[test]
    fn it_displays_as_written() {
        assert_eq!(cidr("192.168.0.0/16").to_string(), "192.168.0.0/16");
        assert_eq!(cidr("127.0.0.1").to_string(), "127.0.0.1/32");
        assert_eq!(cidr("fd00::/8").to_string(), "fd00::/8");
    }
}
