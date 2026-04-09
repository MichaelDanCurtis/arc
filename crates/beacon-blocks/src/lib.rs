//! Command block detection from PTY byte streams.
//!
//! Detects OSC 133 semantic prompts (primary) with a heuristic
//! prompt pattern fallback (Task 14) and emits `BlockEvent`s
//! consumed by `beacon-session`.

pub mod detector;
pub mod heuristic;
pub mod osc133;
pub use detector::{BlockDetector, BlockEvent};
pub use heuristic::HeuristicPromptDetector;
pub use osc133::Osc133Event;
