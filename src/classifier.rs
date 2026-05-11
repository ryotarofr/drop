//! Rule-based classifier for the `今日中` (due-today) flag.
//!
//! A small set of regexes scans the raw entry text. No LLM, no ML — by
//! design, so the classification is offline and predictable. The flag
//! itself is sticky in the database; whether an entry actually appears in
//! the `今日中` section is also gated on `created_at` falling on the local
//! current day (see `Entry::is_due_today_now`).
//!
//! Patterns:
//!   * 今日 / 今夜 / 夕方 / 明朝 / 明日
//!   * digits + 時 (e.g. `18時`, `18時まで`) — both ASCII and full-width
//!
//! "明日" is intentionally treated as due-today: users often write
//! "明日までに" while meaning "before they go to bed tonight", and the
//! flag naturally expires when the local date rolls over anyway.

use once_cell::sync::Lazy;
use regex::Regex;

static PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        // Keyword set. `今日中` is intentionally omitted — `今日` already
        // matches that substring.
        r"今日",
        r"今夜",
        r"夕方",
        r"明朝",
        r"明日",
        // "18時" / "18時まで" / "18時半". Accepts full-width digits.
        r"[0-9０-９]{1,2}\s*時",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("classifier regex compiles"))
    .collect()
});

/// Returns `true` when the text looks like something the user wants done today.
pub fn is_due_today(text: &str) -> bool {
    PATTERNS.iter().any(|re| re.is_match(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_today() {
        assert!(is_due_today("今日中に牛乳買う"));
        assert!(is_due_today("今日 PR を出す"));
    }

    #[test]
    fn tomorrow_counts() {
        assert!(is_due_today("明日 PR レビュー"));
    }

    #[test]
    fn time_of_day() {
        assert!(is_due_today("18時までに連絡"));
        assert!(is_due_today("9時 ミーティング"));
        assert!(is_due_today("１８時 集合")); // full-width digits
    }

    #[test]
    fn dusk_and_dawn() {
        assert!(is_due_today("夕方に散歩"));
        assert!(is_due_today("今夜映画"));
        assert!(is_due_today("明朝ジョグ"));
    }

    #[test]
    fn non_urgent() {
        assert!(!is_due_today("ベルトの斜め配置できたら面白そう"));
        assert!(!is_due_today("factory ゲームのチュートリアル"));
        assert!(!is_due_today("Pxlot のパレット選択、HSL 空間で"));
    }

    #[test]
    fn does_not_flag_random_digits() {
        // "12" alone shouldn't trigger; need "時" suffix.
        assert!(!is_due_today("12 個ほしい"));
    }
}
