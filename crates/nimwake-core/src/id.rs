//! Lifecycle identity 타입.
//!
//! 요구사항: PRD §68 Lifecycle Identity.
//!
//! actor 밖에서 발생한 비동기 event는 도착 시점에 이미 무효일 수 있다.
//! 모든 event는 아래 identity 중 하나를 동반하며, 현재 state와 불일치하면 무시한다.

use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! define_id {
    ($(#[$doc:meta])* $name:ident, $seq:ident, $label:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(u64);

        impl $name {
            /// 주로 테스트에서 특정 id를 재현하기 위해 사용한다.
            #[must_use]
            pub const fn from_raw(raw: u64) -> Self {
                Self(raw)
            }

            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, concat!($label, "#{}"), self.0)
            }
        }

        #[doc = concat!("`", stringify!($name), "` 발급기. 1부터 단조 증가한다.")]
        #[derive(Debug, Default)]
        pub struct $seq(AtomicU64);

        impl $seq {
            #[must_use]
            pub const fn new() -> Self {
                Self(AtomicU64::new(0))
            }

            pub fn next_id(&self) -> $name {
                $name(self.0.fetch_add(1, Ordering::Relaxed) + 1)
            }
        }
    };
}

define_id!(
    /// startup / shutdown처럼 actor 밖에서 수행하는 비동기 operation의 식별자.
    ///
    /// stale한 `StartCompleted` / `StopCompleted`가 새 state를 덮어쓰는 것을 막는다.
    OperationId,
    OperationIdSeq,
    "op"
);

define_id!(
    /// runtime이 생성한 process 인스턴스의 식별자.
    ///
    /// 이미 교체된 process의 exit event가 잘못 적용되는 것을 막는다.
    ProcessId,
    ProcessIdSeq,
    "proc"
);

define_id!(
    /// idle timer 세대.
    ///
    /// 새 요청이 들어온 뒤 뒤늦게 발화한 idle timer가 활성 app을 종료하는 것을 막는다.
    IdleGeneration,
    IdleGenerationSeq,
    "gen"
);

define_id!(
    /// 진행 중인 요청 하나를 나타내는 식별자.
    ///
    /// 중복 반납이나 이미 교체된 process에 대한 반납을 무시할 수 있게 한다.
    LeaseId,
    LeaseIdSeq,
    "lease"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_start_at_one_and_increase() {
        let seq = OperationIdSeq::new();
        assert_eq!(seq.next_id().get(), 1);
        assert_eq!(seq.next_id().get(), 2);
        assert_ne!(seq.next_id(), OperationId::from_raw(2));
    }

    #[test]
    fn ids_of_different_kinds_are_distinct_types() {
        let process = ProcessIdSeq::new().next_id();
        let lease = LeaseIdSeq::new().next_id();
        assert_eq!(process.get(), lease.get());
        assert_eq!(process.to_string(), "proc#1");
        assert_eq!(lease.to_string(), "lease#1");
    }
}
