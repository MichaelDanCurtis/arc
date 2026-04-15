//! wasm-bindgen integration tests for `WasmBlockDetector`.
//!
//! Run with: `wasm-pack test --node beacon-core/crates/beacon-blocks-wasm`
//!
//! These tests exercise the full `JsValue` serialisation boundary so they
//! catch serde mismatches that the native unit test cannot.

#![cfg(target_arch = "wasm32")]

use beacon_blocks_wasm::{BlockEventJs, DetectorMode, WasmBlockDetector};
use wasm_bindgen_test::*;

// No wasm_bindgen_test_configure! call needed — the default runner is node,
// which matches `wasm-pack test --node`.

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Call `det.feed(bytes)` and deserialise the result back to `Vec<BlockEventJs>`.
fn feed(det: &mut WasmBlockDetector, bytes: &[u8]) -> Vec<BlockEventJs> {
    let js_val = det.feed(bytes).expect("feed should not error");
    serde_wasm_bindgen::from_value(js_val).expect("deserialisation should succeed")
}

// ---------------------------------------------------------------------------
// Test a
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
fn osc133_full_sequence_passes_through() {
    let mut det = WasmBlockDetector::new();

    let mut stream = Vec::new();
    stream.extend_from_slice(b"\x1b]133;A\x1b\\"); // PromptStart
    stream.extend_from_slice(b"\x1b]133;B\x1b\\"); // CommandStart
    stream.extend_from_slice(b"ls");
    stream.extend_from_slice(b"\x1b]133;C\x1b\\"); // OutputStart
    stream.extend_from_slice(b"a\nb\n");
    stream.extend_from_slice(b"\x1b]133;D;0\x1b\\"); // CommandEnd exit=0

    let events = feed(&mut det, &stream);

    assert!(
        events
            .iter()
            .any(|e| *e == BlockEventJs::PromptStart { mode: DetectorMode::Osc133 }),
        "missing PromptStart(osc133); events: {:?}",
        events
    );
    assert!(
        events.iter().any(|e| *e
            == BlockEventJs::CommandStart {
                mode: DetectorMode::Osc133,
                command: None,
            }),
        "missing CommandStart(osc133); events: {:?}",
        events
    );
    assert!(
        events
            .iter()
            .any(|e| *e == BlockEventJs::OutputStart { mode: DetectorMode::Osc133 }),
        "missing OutputStart(osc133); events: {:?}",
        events
    );
    assert!(
        events.iter().any(|e| *e
            == BlockEventJs::CommandEnd {
                mode: DetectorMode::Osc133,
                exit_code: Some(0),
            }),
        "missing CommandEnd(osc133, exit=0); events: {:?}",
        events
    );

    // Collect output bytes and verify they include "a\nb"
    let out_bytes: Vec<u8> = events
        .iter()
        .filter_map(|e| {
            if let BlockEventJs::OutputByte { b } = e {
                Some(*b)
            } else {
                None
            }
        })
        .collect();
    let out_str = String::from_utf8_lossy(&out_bytes);
    assert!(
        out_str.contains("a\nb"),
        "output bytes should contain 'a\\nb'; got {:?}",
        out_str
    );
}

// ---------------------------------------------------------------------------
// Test b
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
fn heuristic_only_stream_synthesizes_boundaries() {
    let mut det = WasmBlockDetector::new();

    // Feed a stream that has two prompt lines but NO OSC 133.
    let stream = b"user@host:/tmp$ \nls\nfile1  file2\nuser@host:/tmp$ \n";
    let events = feed(&mut det, stream);

    // Find indices for the first PromptStart(heuristic) and second one.
    let prompt_indices: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            if *e == (BlockEventJs::PromptStart { mode: DetectorMode::Heuristic }) {
                Some(i)
            } else {
                None
            }
        })
        .collect();

    assert!(
        prompt_indices.len() >= 2,
        "expected at least 2 heuristic PromptStart events; got {:?} in {:?}",
        prompt_indices.len(),
        events
    );

    // First prompt hit: PromptStart → CommandStart → OutputStart (three-event tuple)
    let first = prompt_indices[0];
    assert_eq!(
        events.get(first),
        Some(&BlockEventJs::PromptStart {
            mode: DetectorMode::Heuristic
        }),
        "first event after first prompt should be PromptStart"
    );
    assert_eq!(
        events.get(first + 1),
        Some(&BlockEventJs::CommandStart {
            mode: DetectorMode::Heuristic,
            command: None,
        }),
        "second event should be CommandStart"
    );
    assert_eq!(
        events.get(first + 2),
        Some(&BlockEventJs::OutputStart {
            mode: DetectorMode::Heuristic
        }),
        "third event should be OutputStart"
    );

    // Second prompt hit: the event immediately before the second PromptStart
    // should be CommandEnd(heuristic) that closes the first block.
    let second = prompt_indices[1];
    assert!(
        second > 0,
        "second PromptStart must not be the very first event"
    );
    let before_second = events.get(second - 1);
    assert_eq!(
        before_second,
        Some(&BlockEventJs::CommandEnd {
            mode: DetectorMode::Heuristic,
            exit_code: None,
        }),
        "event before second PromptStart must be heuristic CommandEnd; got {:?}",
        before_second
    );
}

// ---------------------------------------------------------------------------
// Test c
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
fn sticky_mode_locks_after_first_osc133() {
    let mut det = WasmBlockDetector::new();

    // Phase 1: heuristic bytes only → heuristic events emitted.
    let events1 = feed(&mut det, b"$ \n");
    assert!(
        events1
            .iter()
            .any(|e| *e == BlockEventJs::PromptStart { mode: DetectorMode::Heuristic }),
        "expected heuristic PromptStart before any OSC 133; got {:?}",
        events1
    );

    // Phase 2: first OSC 133 PromptStart.
    // Expect: synthetic heuristic CommandEnd THEN OSC 133 PromptStart.
    let osc_prompt = b"\x1b]133;A\x1b\\";
    let events2 = feed(&mut det, osc_prompt);

    let synthetic_idx = events2.iter().position(|e| {
        *e == BlockEventJs::CommandEnd {
            mode: DetectorMode::Heuristic,
            exit_code: None,
        }
    });
    assert!(
        synthetic_idx.is_some(),
        "expected synthetic heuristic CommandEnd before OSC 133 PromptStart; got {:?}",
        events2
    );

    let osc_idx = events2.iter().position(|e| {
        *e == BlockEventJs::PromptStart {
            mode: DetectorMode::Osc133,
        }
    });
    assert!(
        osc_idx.is_some(),
        "expected OSC 133 PromptStart in events2; got {:?}",
        events2
    );

    assert!(
        synthetic_idx.unwrap() < osc_idx.unwrap(),
        "synthetic heuristic CommandEnd must precede OSC 133 PromptStart"
    );

    // Phase 3: bytes the heuristic would normally fire on — mode locked → ZERO heuristic events.
    let events3 = feed(&mut det, b"$ \n");
    let any_heuristic = events3.iter().any(|e| {
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
        "ZERO heuristic events expected after mode is locked; got {:?}",
        events3
    );
}

// ---------------------------------------------------------------------------
// Test d
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
fn plain_output_emits_only_outputbytes() {
    let mut det = WasmBlockDetector::new();

    let events = feed(&mut det, b"hello\n");

    let non_output: Vec<&BlockEventJs> = events
        .iter()
        .filter(|e| !matches!(e, BlockEventJs::OutputByte { .. }))
        .collect();

    assert!(
        non_output.is_empty(),
        "expected only OutputByte events for plain output; got {:?}",
        non_output
    );
}
