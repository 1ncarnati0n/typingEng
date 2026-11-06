#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CharState {
    Correct,
    Wrong,
    Pending,
}

/// 목표 문장의 글자마다 상태. typed가 더 길면 초과분은 무시(줄 끝에서 Enter로 넘어감).
pub fn char_states(target: &str, typed: &str) -> Vec<CharState> {
    let mut t = typed.chars();
    target
        .chars()
        .map(|c| match t.next() {
            Some(x) if x == c => CharState::Correct,
            Some(_) => CharState::Wrong,
            None => CharState::Pending,
        })
        .collect()
}

pub fn count_correct(target: &str, typed: &str) -> usize {
    char_states(target, typed).iter().filter(|s| **s == CharState::Correct).count()
}

/// 표준 WPM: 맞은 글자 5개 = 1단어.
pub fn wpm(correct_chars: usize, secs: f64) -> f64 {
    if secs <= 0.0 {
        return 0.0;
    }
    correct_chars as f64 / 5.0 / (secs / 60.0)
}

/// 틀린 글자가 하나라도 있는 단어 (구두점 제거, 소문자).
pub fn missed_words(target: &str, typed: &str) -> Vec<String> {
    let states = char_states(target, typed);
    let chars: Vec<char> = target.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..=chars.len() {
        if i == chars.len() || chars[i].is_whitespace() {
            if states[start..i].contains(&CharState::Wrong) {
                let w: String = chars[start..i]
                    .iter()
                    .filter(|c| c.is_alphanumeric() || **c == '\'')
                    .collect::<String>()
                    .to_lowercase();
                if !w.is_empty() {
                    out.push(w);
                }
            }
            start = i + 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use CharState::*;

    #[test]
    fn states() {
        assert_eq!(char_states("abc", "abc"), vec![Correct, Correct, Correct]);
        assert_eq!(char_states("abc", "axc"), vec![Correct, Wrong, Correct]);
        assert_eq!(char_states("abc", ""), vec![Pending, Pending, Pending]);
        assert_eq!(char_states("ab", "abzz"), vec![Correct, Correct]);
    }

    #[test]
    fn wpm_and_missed() {
        assert_eq!(wpm(50, 60.0), 10.0);
        assert_eq!(wpm(10, 0.0), 0.0);
        assert_eq!(missed_words("I like coffee.", "I lika coffee,"), vec!["like", "coffee"]);
        assert!(missed_words("Hi there", "Hi").is_empty());
        assert_eq!(count_correct("hello", "help"), 3);
    }
}
