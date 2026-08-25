pub trait Named {
    fn name(&self) -> &str;
}

pub fn find<'a, T: Named>(items: &'a [T], fragment: &str) -> Option<&'a T> {
    ranked(items, fragment).into_iter().next()
}

pub fn search<'a, T: Named>(items: &'a [T], fragment: &str, limit: usize) -> Vec<&'a T> {
    let mut found = ranked(items, fragment);
    found.truncate(limit);
    found
}

fn ranked<'a, T: Named>(items: &'a [T], fragment: &str) -> Vec<&'a T> {
    let mut matches: Vec<(u8, &T)> = items
        .iter()
        .filter_map(|item| {
            let name = item.name();
            if name.eq_ignore_ascii_case(fragment) {
                Some((0, item))
            } else if starts_ignore_case(name, fragment) {
                Some((1, item))
            } else if contains_ignore_case(name, fragment) {
                Some((2, item))
            } else {
                None
            }
        })
        .collect();
    matches.sort_by_key(|(rank, _)| *rank);
    matches.into_iter().map(|(_, item)| item).collect()
}

fn starts_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.len() >= needle.len() && haystack.as_bytes()[..needle.len()].eq_ignore_ascii_case(needle.as_bytes())
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let (haystack, needle) = (haystack.as_bytes(), needle.as_bytes());
    needle.len() <= haystack.len()
        && haystack
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Row(&'static str);

    impl Named for Row {
        fn name(&self) -> &str {
            self.0
        }
    }

    fn rows() -> Vec<Row> {
        vec![
            Row("Sickly Basilisk"),
            Row("Basilisk Hatchling"),
            Row("Basilisk"),
            Row("Brutal Basilisk"),
        ]
    }

    #[test]
    fn exact_beats_prefix_beats_substring() {
        let rows = rows();
        let found = search(&rows, "basilisk", 4);
        assert_eq!(found[0].0, "Basilisk");
        assert_eq!(found[1].0, "Basilisk Hatchling");
        assert_eq!(found[2].0, "Sickly Basilisk");
        assert_eq!(found[3].0, "Brutal Basilisk");
    }

    #[test]
    fn matching_is_case_insensitive() {
        let rows = rows();
        assert_eq!(find(&rows, "SICKLY").map(|row| row.0), Some("Sickly Basilisk"));
    }

    #[test]
    fn nothing_matches_a_missing_name() {
        assert!(find(&rows(), "dragon").is_none());
    }
}
