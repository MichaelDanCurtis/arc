/// Events emitted by the OSC 133 parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Osc133Event {
    /// OSC 133 ; A — prompt start
    PromptStart,
    /// OSC 133 ; B — command input start
    CommandStart,
    /// OSC 133 ; C — command output start
    OutputStart,
    /// OSC 133 ; D[; exit] — command end (optional exit code)
    CommandEnd { exit_code: Option<i32> },
}

/// Parse a single OSC 133 params slice (the body between `ESC ] 133 ;`
/// and the terminator).
pub(crate) fn parse_osc133_params(params: &[&[u8]]) -> Option<Osc133Event> {
    // params[0] should be b"133"
    let kind = params.get(1)?;
    match *kind {
        b"A" => Some(Osc133Event::PromptStart),
        b"B" => Some(Osc133Event::CommandStart),
        b"C" => Some(Osc133Event::OutputStart),
        b"D" => {
            let exit = params
                .get(2)
                .and_then(|b| std::str::from_utf8(b).ok())
                .and_then(|s| s.parse::<i32>().ok());
            Some(Osc133Event::CommandEnd { exit_code: exit })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_start() {
        let ev = parse_osc133_params(&[b"133", b"A"]);
        assert_eq!(ev, Some(Osc133Event::PromptStart));
    }

    #[test]
    fn command_end_with_exit_code() {
        let ev = parse_osc133_params(&[b"133", b"D", b"0"]);
        assert_eq!(ev, Some(Osc133Event::CommandEnd { exit_code: Some(0) }));
    }

    #[test]
    fn command_end_without_exit_code() {
        let ev = parse_osc133_params(&[b"133", b"D"]);
        assert_eq!(ev, Some(Osc133Event::CommandEnd { exit_code: None }));
    }

    #[test]
    fn unknown_kind_returns_none() {
        let ev = parse_osc133_params(&[b"133", b"Z"]);
        assert_eq!(ev, None);
    }
}
