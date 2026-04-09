/// Heuristic prompt detector for shells that don't emit OSC 133.
///
/// Scans incoming lines for common prompt shapes (ending in `$`, `#`,
/// `>`, or `%`). Not perfect — users with custom PS1 strings may need
/// to install the shell integration script.
pub struct HeuristicPromptDetector {
    line_buf: Vec<u8>,
}

impl HeuristicPromptDetector {
    pub fn new() -> Self {
        Self {
            line_buf: Vec::new(),
        }
    }

    /// Feed a byte and return `true` if the current line looks like
    /// a prompt (end-of-line on one of `$`, `#`, `>`, `%` followed by
    /// optional whitespace).
    pub fn feed(&mut self, byte: u8) -> bool {
        if byte == b'\n' {
            let looks_like_prompt = looks_like_prompt(&self.line_buf);
            self.line_buf.clear();
            return looks_like_prompt;
        }
        self.line_buf.push(byte);
        false
    }
}

impl Default for HeuristicPromptDetector {
    fn default() -> Self {
        Self::new()
    }
}

fn looks_like_prompt(line: &[u8]) -> bool {
    // Allow trailing whitespace, then one of the prompt sigils.
    let trimmed = trim_end_ascii(line);
    matches!(
        trimmed.last(),
        Some(b'$') | Some(b'#') | Some(b'>') | Some(b'%')
    )
}

fn trim_end_ascii(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    while end > 0 && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t') {
        end -= 1;
    }
    &bytes[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(det: &mut HeuristicPromptDetector, s: &str) -> usize {
        let mut hits = 0;
        for b in s.bytes() {
            if det.feed(b) {
                hits += 1;
            }
        }
        hits
    }

    #[test]
    fn dollar_prompt_detected() {
        let mut d = HeuristicPromptDetector::new();
        assert_eq!(feed_all(&mut d, "user@host:/tmp$ \n"), 1);
    }

    #[test]
    fn hash_root_prompt_detected() {
        let mut d = HeuristicPromptDetector::new();
        assert_eq!(feed_all(&mut d, "root@host:~# \n"), 1);
    }

    #[test]
    fn plain_output_is_not_a_prompt() {
        let mut d = HeuristicPromptDetector::new();
        assert_eq!(feed_all(&mut d, "hello world\n"), 0);
    }

    #[test]
    fn multiple_lines_count_independently() {
        let mut d = HeuristicPromptDetector::new();
        let hits = feed_all(&mut d, "hello\n$ \nworld\n$ \n");
        assert_eq!(hits, 2);
    }
}
