use crate::osc133::{parse_osc133_params, Osc133Event};
use vte::{Params, Parser, Perform};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockEvent {
    PromptStart,
    CommandStart,
    OutputStart,
    CommandEnd { exit_code: Option<i32> },
    OutputByte(u8),
    /// OSC 7 — shell reported a new current working directory.
    CwdChanged(String),
}

/// Extract the absolute path from an OSC 7 `file://host/path` payload.
/// Returns `None` if the payload isn't a recognized `file://` URL.
fn parse_osc7_cwd(payload: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(payload).ok()?;
    let rest = s.strip_prefix("file://")?;
    // Skip the hostname — rest is `host/path`. Path starts at the first `/`.
    let slash = rest.find('/')?;
    let raw = &rest[slash..];
    // Percent-decode just the common cases (%20 etc.). Keep it minimal.
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(hi) = (bytes[i + 1] as char).to_digit(16) {
                if let Some(lo) = (bytes[i + 2] as char).to_digit(16) {
                    out.push(((hi * 16 + lo) as u8) as char);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    Some(out)
}

#[derive(Default)]
struct DetectorPerform {
    events: Vec<BlockEvent>,
}

impl Perform for DetectorPerform {
    fn print(&mut self, c: char) {
        let mut tmp = [0u8; 4];
        for b in c.encode_utf8(&mut tmp).as_bytes() {
            self.events.push(BlockEvent::OutputByte(*b));
        }
    }

    fn execute(&mut self, byte: u8) {
        if matches!(byte, b'\n' | b'\r' | b'\t' | 0x07) {
            self.events.push(BlockEvent::OutputByte(byte));
        }
    }

    fn hook(&mut self, _: &Params, _: &[u8], _: bool, _: char) {}
    fn put(&mut self, _: u8) {}
    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.first().map(|p| *p == b"133").unwrap_or(false) {
            if let Some(ev) = parse_osc133_params(params) {
                self.events.push(match ev {
                    Osc133Event::PromptStart => BlockEvent::PromptStart,
                    Osc133Event::CommandStart => BlockEvent::CommandStart,
                    Osc133Event::OutputStart => BlockEvent::OutputStart,
                    Osc133Event::CommandEnd { exit_code } => BlockEvent::CommandEnd { exit_code },
                });
            }
            return;
        }
        if params.first().map(|p| *p == b"7").unwrap_or(false) {
            if let Some(payload) = params.get(1) {
                if let Some(path) = parse_osc7_cwd(payload) {
                    self.events.push(BlockEvent::CwdChanged(path));
                }
            }
        }
    }

    fn csi_dispatch(&mut self, _: &Params, _: &[u8], _: bool, _: char) {}
    fn esc_dispatch(&mut self, _: &[u8], _: bool, _: u8) {}
}

pub struct BlockDetector {
    parser: Parser,
    perform: DetectorPerform,
}

impl BlockDetector {
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
            perform: DetectorPerform::default(),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<BlockEvent> {
        for b in bytes {
            self.parser.advance(&mut self.perform, *b);
        }
        std::mem::take(&mut self.perform.events)
    }
}

impl Default for BlockDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(bytes: &[u8]) -> Vec<BlockEvent> {
        let mut d = BlockDetector::new();
        d.feed(bytes)
    }

    #[test]
    fn osc133_full_sequence_emits_all_events() {
        let mut stream = Vec::new();
        stream.extend_from_slice(b"\x1b]133;A\x1b\\");
        stream.extend_from_slice(b"\x1b]133;B\x1b\\");
        stream.extend_from_slice(b"ls");
        stream.extend_from_slice(b"\x1b]133;C\x1b\\");
        stream.extend_from_slice(b"a\nb\n");
        stream.extend_from_slice(b"\x1b]133;D;0\x1b\\");

        let events = run(&stream);
        assert!(events.contains(&BlockEvent::PromptStart));
        assert!(events.contains(&BlockEvent::CommandStart));
        assert!(events.contains(&BlockEvent::OutputStart));
        assert!(events.contains(&BlockEvent::CommandEnd { exit_code: Some(0) }));
        let out: Vec<u8> = events
            .iter()
            .filter_map(|e| {
                if let BlockEvent::OutputByte(b) = e {
                    Some(*b)
                } else {
                    None
                }
            })
            .collect();
        assert!(
            String::from_utf8_lossy(&out).contains("a\nb"),
            "missing output, got {:?}",
            String::from_utf8_lossy(&out)
        );
    }

    #[test]
    fn osc7_file_url_emits_cwd_changed() {
        let events = run(b"\x1b]7;file://myhost/Users/alice/proj\x1b\\");
        let cwd = events.iter().find_map(|e| {
            if let BlockEvent::CwdChanged(p) = e { Some(p.clone()) } else { None }
        });
        assert_eq!(cwd, Some("/Users/alice/proj".to_string()));
    }

    #[test]
    fn osc7_percent_encoded_spaces_decoded() {
        let events = run(b"\x1b]7;file://host/tmp/a%20b\x1b\\");
        let cwd = events.iter().find_map(|e| {
            if let BlockEvent::CwdChanged(p) = e { Some(p.clone()) } else { None }
        });
        assert_eq!(cwd, Some("/tmp/a b".to_string()));
    }

    #[test]
    fn plain_bytes_produce_only_output_events() {
        let events = run(b"hello\n");
        assert!(events
            .iter()
            .all(|e| matches!(e, BlockEvent::OutputByte(_))));
    }
}
