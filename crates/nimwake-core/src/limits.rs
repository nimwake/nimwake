//! daemon 수준 자원 한계.
//!
//! 요구사항: PRD §78 Memory Admission Control.
//!
//! manifest가 아니라 여기 있는 이유: 이 값들은 **시스템 전체의 성질**이고
//! application마다 다른 값을 갖는 것이 아니다. daemon 설정은 모두 CLI 인자다. (§74)

/// 기동을 허용할지 판단하는 한계값.
///
/// 전부 "끔"이 기본이다. 켜져 있으면 지금까지 되던 기동이 거절될 수 있고,
/// scale-to-zero 엔진이 기본값 때문에 app을 깨우지 않는 것은 놀랄 일이다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    /// 잠든 application을 깨우기 전에 있어야 하는 가용 메모리(MiB).
    ///
    /// `0`이면 검사하지 않는다. 그때는 `/proc/meminfo`를 읽지도 않는다. (PRD §78)
    pub min_free_memory_mb: u64,
}

impl Limits {
    /// 메모리 입장 제어가 켜져 있는가.
    #[must_use]
    pub const fn checks_memory(&self) -> bool {
        self.min_free_memory_mb > 0
    }
}
