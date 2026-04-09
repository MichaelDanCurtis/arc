use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use ulid::Ulid;

macro_rules! ulid_newtype {
    ($name:ident) => {
        #[derive(
            Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Ulid);

        impl $name {
            pub fn new() -> Self {
                Self(Ulid::new())
            }
            pub fn from_ulid(u: Ulid) -> Self {
                Self(u)
            }
            pub fn as_ulid(&self) -> Ulid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl FromStr for $name {
            type Err = ulid::DecodeError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ulid::from_str(s).map(Self)
            }
        }
    };
}

ulid_newtype!(BlockId);
ulid_newtype!(SessionId);
ulid_newtype!(TabId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_id_roundtrips_through_string() {
        let id = BlockId::new();
        let s = id.to_string();
        let parsed: BlockId = s.parse().expect("parse round-trip");
        assert_eq!(id, parsed);
    }

    #[test]
    fn session_and_tab_ids_are_distinct_types() {
        let s = SessionId::new();
        let t = TabId::new();
        assert_ne!(s.to_string(), t.to_string());
    }

    #[test]
    fn block_id_serializes_as_string() {
        let id = BlockId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert!(json.starts_with('"') && json.ends_with('"'));
        let back: BlockId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
