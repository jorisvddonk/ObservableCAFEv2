/// Parsed narrator command from a user message.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `!load <path>` — load an EPUB from a filesystem path.
    Load(String),
    /// `!next` / `!n` — read the next chapter aloud.
    Next,
    /// `!prev` / `!p` — read the previous chapter aloud.
    Prev,
    /// `!chapter <n>` / `!ch <n>` — jump to and read chapter n.
    Chapter(usize),
    /// `!list` / `!l` — list all chapters.
    List,
    /// `!current` / `!c` — show the current chapter.
    Current,
    /// `!help` / `!h` — show available commands.
    Help,
}

/// Parse a user message into a `Command`. Returns `None` if the message is not
/// a narrator command (or is malformed).
pub fn parse(text: &str) -> Option<Command> {
    let text = text.trim();
    let rest = text.strip_prefix('!')?;
    let (verb, arg) = match rest.split_once(char::is_whitespace) {
        Some((v, a)) => (v.to_ascii_lowercase(), a.trim()),
        None => (rest.to_ascii_lowercase(), ""),
    };

    match verb.as_str() {
        "load" => {
            if arg.is_empty() {
                None
            } else {
                Some(Command::Load(arg.to_string()))
            }
        }
        "next" | "n" => Some(Command::Next),
        "prev" | "p" => Some(Command::Prev),
        "chapter" | "ch" => arg.parse::<usize>().ok().map(Command::Chapter),
        "list" | "l" => Some(Command::List),
        "current" | "c" => Some(Command::Current),
        "help" | "h" => Some(Command::Help),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_load() {
        assert_eq!(
            parse("!load /books/moby.epub"),
            Some(Command::Load("/books/moby.epub".to_string()))
        );
        assert_eq!(parse("!load"), None);
    }

    #[test]
    fn parses_next_prev() {
        assert_eq!(parse("!next"), Some(Command::Next));
        assert_eq!(parse("!n"), Some(Command::Next));
        assert_eq!(parse("!prev"), Some(Command::Prev));
        assert_eq!(parse("!p"), Some(Command::Prev));
    }

    #[test]
    fn parses_chapter() {
        assert_eq!(parse("!chapter 7"), Some(Command::Chapter(7)));
        assert_eq!(parse("!ch 12"), Some(Command::Chapter(12)));
        assert_eq!(parse("!chapter abc"), None);
        assert_eq!(parse("!chapter"), None);
    }

    #[test]
    fn parses_list_current_help() {
        assert_eq!(parse("!list"), Some(Command::List));
        assert_eq!(parse("!l"), Some(Command::List));
        assert_eq!(parse("!current"), Some(Command::Current));
        assert_eq!(parse("!c"), Some(Command::Current));
        assert_eq!(parse("!help"), Some(Command::Help));
        assert_eq!(parse("!h"), Some(Command::Help));
    }

    #[test]
    fn rejects_non_commands() {
        assert_eq!(parse("hello"), None);
        assert_eq!(parse("!unknown"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(parse("!NEXT"), Some(Command::Next));
        assert_eq!(parse("!LIST"), Some(Command::List));
    }
}
