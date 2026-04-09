use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub beacon_version: String,
}

impl HostInfo {
    pub fn current() -> Self {
        Self {
            hostname: hostname_or_unknown(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            beacon_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

fn hostname_or_unknown() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_info_json_round_trip() {
        let h = HostInfo {
            hostname: "mbp.local".to_string(),
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            beacon_version: env!("CARGO_PKG_VERSION").to_string(),
        };
        let s = serde_json::to_string(&h).unwrap();
        let back: HostInfo = serde_json::from_str(&s).unwrap();
        assert_eq!(h, back);
    }

    #[test]
    fn host_info_current_fills_all_fields() {
        let h = HostInfo::current();
        assert!(!h.hostname.is_empty());
        assert!(!h.os.is_empty());
        assert!(!h.arch.is_empty());
    }
}
