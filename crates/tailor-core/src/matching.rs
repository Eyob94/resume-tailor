use crate::search::WorkMode;

pub fn contains_term(haystack: &str, term: &str) -> bool {
    let haystack = haystack.to_lowercase();
    let term = term.trim().to_lowercase();
    let Some(first) = term.chars().next() else {
        return false;
    };

    let mut from = 0;
    while let Some(offset) = haystack[from..].find(&term) {
        let begin = from + offset;
        let end = begin + term.len();
        let clear_before = haystack[..begin].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let clear_after = haystack[end..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if clear_before && clear_after {
            return true;
        }
        from = begin + first.len_utf8();
    }
    false
}

pub fn title_matches(job_title: &str, titles: &[String]) -> bool {
    titles.iter().any(|title| title.split_whitespace().all(|word| contains_term(job_title, word)))
}

pub fn keyword_hits(text: &str, keywords: &[String]) -> Vec<String> {
    keywords.iter().filter(|k| contains_term(text, k)).cloned().collect()
}

const TITLE_SIGNALS: &[(WorkMode, &[&str])] = &[
    (WorkMode::Remote, &["remote", "work from home", "wfh"]),
    (WorkMode::Hybrid, &["hybrid"]),
    (WorkMode::Onsite, &["on-site", "onsite", "on site", "in-office", "office-based"]),
];

const DESCRIPTION_SIGNALS: &[(WorkMode, &[&str])] = &[
    (
        WorkMode::Remote,
        &["fully remote", "100% remote", "remote-first", "remote first", "remote position", "remote role", "work remotely", "work from home", "remote work"],
    ),
    (WorkMode::Hybrid, &["hybrid"]),
    (WorkMode::Onsite, &["on-site", "onsite", "in-office", "office-based", "work from office", "work from the office"]),
];

fn modes_in(text: &str, signals: &[(WorkMode, &[&str])]) -> Vec<WorkMode> {
    signals
        .iter()
        .filter(|(_, phrases)| phrases.iter().any(|p| contains_term(text, p)))
        .map(|(mode, _)| *mode)
        .collect()
}

pub fn infer_work_mode(title: &str, location: &str, description: Option<&str>) -> Option<WorkMode> {
    let pick = |modes: Vec<WorkMode>| match modes.as_slice() {
        [only] => Some(*only),
        many if many.contains(&WorkMode::Hybrid) => Some(WorkMode::Hybrid),
        _ => None,
    };

    let headline = modes_in(&format!("{title} {location}"), TITLE_SIGNALS);
    if !headline.is_empty() {
        return pick(headline);
    }
    description.and_then(|d| pick(modes_in(d, DESCRIPTION_SIGNALS)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_term_matching() {
        assert!(contains_term("Senior Java Developer", "java"));
        assert!(!contains_term("JavaScript Developer", "Java"));
        assert!(contains_term("Senior C++ Engineer", "C++"));
        assert!(contains_term("Sr .NET Developer", ".net"));
        assert!(!contains_term("anything", "  "));
    }

    #[test]
    fn titles_match_all_words_any_order() {
        let titles = vec!["Backend Engineer".to_string()];
        assert!(title_matches("Senior Backend Engineer (IOT & Messaging)", &titles));
        assert!(title_matches("Engineer, Backend", &titles));
        assert!(!title_matches("Senior Backend Developer", &titles));
    }

    #[test]
    fn keyword_hits_in_order() {
        let keywords = vec!["Rust".to_string(), "Kafka".to_string(), "Go".to_string()];
        assert_eq!(keyword_hits("We use Kafka and Rust", &keywords), vec!["Rust", "Kafka"]);
    }

    #[test]
    fn work_mode_from_title_or_location() {
        assert_eq!(infer_work_mode("Fullstack Engineer (100% Remote)", "UAE", None), Some(WorkMode::Remote));
        assert_eq!(infer_work_mode("Engineer", "Dubai (Hybrid)", None), Some(WorkMode::Hybrid));
    }

    #[test]
    fn work_mode_from_description_needs_strong_phrasing() {
        assert_eq!(infer_work_mode("Engineer", "Dubai", Some("You'll work with remote teams.")), None);
        assert_eq!(infer_work_mode("Engineer", "Dubai", Some("This is a fully remote role.")), Some(WorkMode::Remote));
        assert_eq!(infer_work_mode("Engineer", "Dubai", Some("Hybrid: 3 days in-office.")), Some(WorkMode::Hybrid));
        assert_eq!(infer_work_mode("Engineer", "Dubai", Some("Based on-site in Abu Dhabi.")), Some(WorkMode::Onsite));
        assert_eq!(infer_work_mode("Engineer", "Dubai", None), None);
    }
}
