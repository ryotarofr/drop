//! Parses what the user typed into the floating input.
//!
//! Rules:
//!   * `/dev on`  and `/dev off` — toggle dev mode (always recognised).
//!   * `/search <query>` — apply a substring filter (always recognised).
//!     `/search` with no argument clears the filter.
//!   * `/del <ID>` — soft-delete (dev mode only). `#` prefix on ID is OK.
//!   * Slash-prefixed text that isn't a recognised command is saved as a
//!     regular entry — no silent command behaviour in normal mode.
//!   * Anything else: save as an entry.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Save this text as a new entry.
    Insert(String),
    /// Enter dev mode.
    DevOn,
    /// Leave dev mode.
    DevOff,
    /// Soft-delete entry by ID (dev mode only).
    Delete(i64),
    /// `/del` typed without an argument while in dev mode.
    DelUsage,
    /// `/del foo` — argument couldn't be parsed as a positive integer.
    DelBadId,
    /// `/search <query>` — apply a substring filter.
    Search(String),
    /// `/search` with no argument — clear any active filter.
    SearchClear,
}

pub fn parse(input: &str, dev_mode: bool) -> Command {
    let trimmed = input.trim();

    // Exact-match commands (gateways or argument-less forms).
    match trimmed {
        "/dev on" => return Command::DevOn,
        "/dev off" => return Command::DevOff,
        "/search" => return Command::SearchClear,
        _ => {}
    }

    // `/search <query>` — works in any mode (read-only).
    if let Some(rest) = trimmed.strip_prefix("/search ") {
        let q = rest.trim();
        if q.is_empty() {
            return Command::SearchClear;
        }
        return Command::Search(q.to_string());
    }

    if dev_mode {
        if trimmed == "/del" {
            return Command::DelUsage;
        }
        if let Some(rest) = trimmed.strip_prefix("/del ") {
            let arg = rest.trim().trim_start_matches('#').trim();
            if arg.is_empty() {
                return Command::DelUsage;
            }
            return match arg.parse::<i64>() {
                Ok(id) if id > 0 => Command::Delete(id),
                _ => Command::DelBadId,
            };
        }
    }

    Command::Insert(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_on_off_always_work() {
        assert_eq!(parse("/dev on", false), Command::DevOn);
        assert_eq!(parse("/dev on", true), Command::DevOn);
        assert_eq!(parse("/dev off", false), Command::DevOff);
        assert_eq!(parse("/dev off", true), Command::DevOff);
    }

    #[test]
    fn dev_with_surrounding_whitespace() {
        assert_eq!(parse("  /dev on  ", false), Command::DevOn);
    }

    #[test]
    fn unknown_slash_text_in_normal_mode_is_an_entry() {
        assert_eq!(
            parse("/del 5", false),
            Command::Insert("/del 5".to_string())
        );
        assert_eq!(
            parse("/dev xyz", false),
            Command::Insert("/dev xyz".to_string())
        );
        assert_eq!(parse("/foo", true), Command::Insert("/foo".to_string()));
    }

    #[test]
    fn del_requires_dev_mode() {
        assert_eq!(parse("/del 5", true), Command::Delete(5));
        assert_eq!(
            parse("/del 5", false),
            Command::Insert("/del 5".to_string())
        );
    }

    #[test]
    fn del_accepts_hash_prefix() {
        assert_eq!(parse("/del #42", true), Command::Delete(42));
        assert_eq!(parse("/del  #42 ", true), Command::Delete(42));
    }

    #[test]
    fn del_usage_when_no_argument() {
        assert_eq!(parse("/del", true), Command::DelUsage);
        assert_eq!(parse("/del   ", true), Command::DelUsage);
        assert_eq!(parse("/del #", true), Command::DelUsage);
    }

    #[test]
    fn del_bad_id_for_non_numeric_or_non_positive() {
        assert_eq!(parse("/del abc", true), Command::DelBadId);
        assert_eq!(parse("/del 0", true), Command::DelBadId);
        assert_eq!(parse("/del -3", true), Command::DelBadId);
        assert_eq!(parse("/del 1.5", true), Command::DelBadId);
    }

    #[test]
    fn plain_text_is_an_entry() {
        assert_eq!(parse("牛乳買う", false), Command::Insert("牛乳買う".into()));
        assert_eq!(parse("牛乳買う", true), Command::Insert("牛乳買う".into()));
    }

    #[test]
    fn search_with_query_in_any_mode() {
        assert_eq!(parse("/search 牛乳", false), Command::Search("牛乳".into()));
        assert_eq!(parse("/search 牛乳", true), Command::Search("牛乳".into()));
    }

    #[test]
    fn search_trims_query() {
        assert_eq!(
            parse("/search   牛乳  ", false),
            Command::Search("牛乳".into())
        );
    }

    #[test]
    fn search_without_argument_clears_filter() {
        assert_eq!(parse("/search", false), Command::SearchClear);
        assert_eq!(parse("/search ", false), Command::SearchClear);
        assert_eq!(parse("/search    ", false), Command::SearchClear);
    }
}
