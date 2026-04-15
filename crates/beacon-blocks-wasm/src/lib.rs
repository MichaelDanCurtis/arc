//! WASM wrapper around `beacon-blocks` for browser-side PTY block detection.
//!
//! Exposes a session-sticky dual-mode detector that combines OSC 133
//! (`BlockDetector`) and a heuristic prompt detector
//! (`HeuristicPromptDetector`). Once any OSC 133 event is seen in a session,
//! the OSC 133 mode becomes authoritative and heuristic emission is suppressed
//! permanently ("mode-locked"). If OSC 133 is never seen, the heuristic
//! provides approximate block boundaries.
//!
//! This crate is the parser for the `/watch` mirror stream in arc-web (C4.2+).

use beacon_blocks::{BlockDetector, BlockEvent, HeuristicPromptDetector};
use wasm_bindgen::prelude::*;

// ---------------------------------------------------------------------------
// Panic hook initialisation
// ---------------------------------------------------------------------------

/// Initialise the WASM module. Installs `console_error_panic_hook` when the
/// feature is enabled so Rust panics surface as readable JS console errors.
#[wasm_bindgen(start)]
pub fn init() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

// ---------------------------------------------------------------------------
// Public JS-visible types
// ---------------------------------------------------------------------------

/// Which detector produced a given block event.
///
/// - `osc133`   — OSC 133 semantic prompt sequences (authoritative).
/// - `heuristic` — line-based prompt heuristic (best-effort fallback).
#[derive(serde::Serialize, serde::Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DetectorMode {
    Osc133,
    Heuristic,
}

/// Block boundary / byte events emitted by [`WasmBlockDetector`] to JS.
///
/// The `kind` discriminant is included in JSON via `#[serde(tag = "kind")]`
/// so JS can switch on `event.kind`.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind")]
pub enum BlockEventJs {
    /// A prompt line was detected; a new block is starting.
    PromptStart { mode: DetectorMode },
    /// The user has started typing a command.
    ///
    /// `command` is always `None` at this milestone (C4.1). C4.4 will
    /// extract command text from `OutputByte` events between `PromptStart`
    /// and `CommandStart` and populate this field.
    CommandStart {
        mode: DetectorMode,
        command: Option<String>,
    },
    /// Command output has begun.
    OutputStart { mode: DetectorMode },
    /// Command finished (OSC 133 D).
    ///
    /// For heuristic-synthesized events `exit_code` is always `None`.
    CommandEnd {
        mode: DetectorMode,
        exit_code: Option<i32>,
    },
    /// A single raw output byte. No `mode` field — output bytes are
    /// mode-agnostic.
    OutputByte { b: u8 },
}

// ---------------------------------------------------------------------------
// WasmBlockDetector
// ---------------------------------------------------------------------------

/// Session-sticky dual-mode block detector for browser-side PTY streams.
///
/// Feed raw PTY bytes via [`feed`][WasmBlockDetector::feed]. Each call returns
/// a (possibly empty) array of [`BlockEventJs`] objects serialised as a
/// `JsValue`.
///
/// ### Mode-locking
/// The first time any OSC 133 structural event is observed (PromptStart,
/// CommandStart, OutputStart, or CommandEnd) `osc133_seen` is set to `true`
/// and heuristic emission is suppressed for the lifetime of this detector
/// instance.
#[wasm_bindgen]
pub struct WasmBlockDetector {
    /// OSC 133 VTE-based detector.
    block_detector: BlockDetector,
    /// Line-based heuristic fallback detector.
    heuristic_detector: HeuristicPromptDetector,
    /// `true` once the first OSC 133 structural event has been seen.
    osc133_seen: bool,
    /// `true` while a heuristic-synthesized block is open (i.e. we emitted
    /// a heuristic `PromptStart` but not yet a matching `CommandEnd`).
    heuristic_block_open: bool,
}

#[wasm_bindgen]
impl WasmBlockDetector {
    /// Create a new detector for a fresh session.
    #[wasm_bindgen(constructor)]
    pub fn new() -> WasmBlockDetector {
        WasmBlockDetector {
            block_detector: BlockDetector::new(),
            heuristic_detector: HeuristicPromptDetector::new(),
            osc133_seen: false,
            heuristic_block_open: false,
        }
    }

    /// Feed a slice of raw PTY bytes and return an array of `BlockEventJs`
    /// objects serialised as a `JsValue` (a JS `Array`).
    ///
    /// Returns `Err(JsValue)` only if serialisation fails (should never
    /// happen in practice).
    pub fn feed(&mut self, bytes: &[u8]) -> Result<JsValue, JsValue> {
        let events = self.feed_inner(bytes);
        serde_wasm_bindgen::to_value(&events).map_err(|e| JsValue::from_str(&e.to_string()))
    }
}

impl WasmBlockDetector {
    /// Core per-byte processing, returning native `Vec<BlockEventJs>`.
    /// Extracted from the `#[wasm_bindgen]` impl so the non-wasm unit test
    /// can call it directly without going through `JsValue`.
    ///
    /// Do NOT make this `pub` — the `#[wasm_bindgen]` `feed()` is the
    /// supported entry point. `feed_inner` is an internal testability
    /// shim for the native unit test only.
    pub(crate) fn feed_inner(&mut self, bytes: &[u8]) -> Vec<BlockEventJs> {
        let mut out: Vec<BlockEventJs> = Vec::new();

        for &byte in bytes {
            // ----------------------------------------------------------------
            // Step 1: Feed the byte into the OSC 133 detector.
            // ----------------------------------------------------------------
            let osc_events = self.block_detector.feed(&[byte]);

            for ev in osc_events {
                match ev {
                    // Structural OSC 133 events lock mode.
                    BlockEvent::PromptStart => {
                        // If a heuristic block was open, close it cleanly
                        // before switching to authoritative OSC 133 mode.
                        if self.heuristic_block_open {
                            out.push(BlockEventJs::CommandEnd {
                                mode: DetectorMode::Heuristic,
                                exit_code: None,
                            });
                            self.heuristic_block_open = false;
                        }
                        self.osc133_seen = true;
                        out.push(BlockEventJs::PromptStart {
                            mode: DetectorMode::Osc133,
                        });
                    }
                    BlockEvent::CommandStart => {
                        if self.heuristic_block_open {
                            out.push(BlockEventJs::CommandEnd {
                                mode: DetectorMode::Heuristic,
                                exit_code: None,
                            });
                            self.heuristic_block_open = false;
                        }
                        self.osc133_seen = true;
                        out.push(BlockEventJs::CommandStart {
                            mode: DetectorMode::Osc133,
                            command: None,
                        });
                    }
                    BlockEvent::OutputStart => {
                        if self.heuristic_block_open {
                            out.push(BlockEventJs::CommandEnd {
                                mode: DetectorMode::Heuristic,
                                exit_code: None,
                            });
                            self.heuristic_block_open = false;
                        }
                        self.osc133_seen = true;
                        out.push(BlockEventJs::OutputStart {
                            mode: DetectorMode::Osc133,
                        });
                    }
                    BlockEvent::CommandEnd { exit_code } => {
                        if self.heuristic_block_open {
                            out.push(BlockEventJs::CommandEnd {
                                mode: DetectorMode::Heuristic,
                                exit_code: None,
                            });
                            self.heuristic_block_open = false;
                        }
                        self.osc133_seen = true;
                        out.push(BlockEventJs::CommandEnd {
                            mode: DetectorMode::Osc133,
                            exit_code,
                        });
                    }

                    // Output bytes are mode-agnostic — pass through always.
                    BlockEvent::OutputByte(b) => {
                        out.push(BlockEventJs::OutputByte { b });
                    }
                }
            }

            // ----------------------------------------------------------------
            // Step 2: Feed the SAME byte into the heuristic detector.
            // The heuristic is always fed (so its line buffer stays warm in
            // case we ever want diagnostics), but emission is gated on
            // `osc133_seen`.
            // ----------------------------------------------------------------
            let heuristic_fired = self.heuristic_detector.feed(byte);

            if heuristic_fired && !self.osc133_seen {
                // Close any existing heuristic block before opening a new one.
                if self.heuristic_block_open {
                    out.push(BlockEventJs::CommandEnd {
                        mode: DetectorMode::Heuristic,
                        exit_code: None,
                    });
                }

                // Emit the heuristic block-start triple.
                out.push(BlockEventJs::PromptStart {
                    mode: DetectorMode::Heuristic,
                });
                out.push(BlockEventJs::CommandStart {
                    mode: DetectorMode::Heuristic,
                    command: None,
                });
                out.push(BlockEventJs::OutputStart {
                    mode: DetectorMode::Heuristic,
                });

                self.heuristic_block_open = true;
            }
        }

        out
    }
}

impl Default for WasmBlockDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Non-wasm unit tests (fast signal during development without wasm-pack)
// ---------------------------------------------------------------------------

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;

    /// Verifies the critical sticky-mode invariant without going through the
    /// JsValue serialisation boundary.
    #[test]
    fn sticky_mode_locks_after_first_osc133_native() {
        let mut det = WasmBlockDetector::new();

        // ---- Phase 1: heuristic-only bytes → heuristic events emitted ----
        let events = det.feed_inner(b"$ \n");
        let has_heuristic = events
            .iter()
            .any(|e| matches!(e, BlockEventJs::PromptStart { mode: DetectorMode::Heuristic }));
        assert!(
            has_heuristic,
            "expected heuristic PromptStart before OSC 133 arrived; got {:?}",
            events
        );
        assert!(det.heuristic_block_open, "heuristic block should be open");
        assert!(!det.osc133_seen, "should not be locked yet");

        // ---- Phase 2: first OSC 133 PromptStart ----
        // Should emit a synthetic heuristic CommandEnd, then OSC 133 PromptStart.
        let osc_prompt = b"\x1b]133;A\x1b\\";
        let events2 = det.feed_inner(osc_prompt);

        let synthetic_close = events2.iter().position(|e| {
            matches!(
                e,
                BlockEventJs::CommandEnd {
                    mode: DetectorMode::Heuristic,
                    exit_code: None,
                }
            )
        });
        assert!(
            synthetic_close.is_some(),
            "expected synthetic heuristic CommandEnd before OSC 133 PromptStart; got {:?}",
            events2
        );

        let osc_start = events2.iter().position(|e| {
            matches!(
                e,
                BlockEventJs::PromptStart {
                    mode: DetectorMode::Osc133,
                }
            )
        });
        assert!(
            osc_start.is_some(),
            "expected OSC 133 PromptStart; got {:?}",
            events2
        );

        // synthetic close MUST come BEFORE the OSC 133 start
        assert!(
            synthetic_close.unwrap() < osc_start.unwrap(),
            "synthetic heuristic CommandEnd must precede OSC 133 PromptStart"
        );

        assert!(det.osc133_seen, "mode should be locked after first OSC 133");
        assert!(!det.heuristic_block_open, "heuristic block should be closed");

        // ---- Phase 3: multiple heuristic-firing patterns → ZERO heuristic events ----
        // Mode is locked → heuristic events must never leak for any of these.
        let heuristic_patterns: &[&[u8]] = &[
            b"$ \n",
            b"user@host:~$ \n",
            b"root# \n",
            b"PS> \n",
            b"40% \n",
        ];
        for pattern in heuristic_patterns {
            let events = det.feed_inner(pattern);
            let any_heuristic = events.iter().any(|e| {
                matches!(
                    e,
                    BlockEventJs::PromptStart {
                        mode: DetectorMode::Heuristic,
                    }
                    | BlockEventJs::CommandStart {
                        mode: DetectorMode::Heuristic,
                        ..
                    }
                    | BlockEventJs::OutputStart {
                        mode: DetectorMode::Heuristic,
                    }
                    | BlockEventJs::CommandEnd {
                        mode: DetectorMode::Heuristic,
                        ..
                    }
                )
            });
            assert!(
                !any_heuristic,
                "heuristic leaked after lock for {:?}",
                pattern
            );
        }
    }
}
