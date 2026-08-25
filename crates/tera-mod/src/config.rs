#[derive(Clone, Debug)]
struct Line {
    text: String,
    ending: &'static str,
}

#[derive(Clone, Debug)]
pub struct Config {
    lines: Vec<Line>,
    ending: &'static str,
    bom: bool,
}

fn section_name(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
}

fn split_entry(line: &str) -> Option<(&str, &str, &str)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with(';') || trimmed.starts_with('#') {
        return None;
    }
    let (prefix, rest) = match trimmed.chars().next() {
        Some(marker @ ('+' | '-' | '.' | '!')) => (&trimmed[..marker.len_utf8()], &trimmed[1..]),
        _ => ("", trimmed),
    };
    let (key, value) = rest.split_once('=')?;
    Some((prefix, key.trim_end(), value))
}

impl Config {
    pub fn parse(text: &str) -> Self {
        let bom = text.starts_with('\u{feff}');
        let body = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut lines = Vec::new();
        let mut rest = body;
        while !rest.is_empty() {
            match rest.find('\n') {
                Some(at) => {
                    let raw = &rest[..at];
                    let (text, ending) = match raw.strip_suffix('\r') {
                        Some(trimmed) => (trimmed, "\r\n"),
                        None => (raw, "\n"),
                    };
                    lines.push(Line {
                        text: text.to_string(),
                        ending,
                    });
                    rest = &rest[at + 1..];
                }
                None => {
                    lines.push(Line {
                        text: rest.to_string(),
                        ending: "",
                    });
                    rest = "";
                }
            }
        }
        let ending = lines
            .iter()
            .map(|line| line.ending)
            .find(|ending| !ending.is_empty())
            .unwrap_or("\n");
        Self { lines, ending, bom }
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        if self.bom {
            out.push('\u{feff}');
        }
        for line in &self.lines {
            out.push_str(&line.text);
            out.push_str(line.ending);
        }
        out
    }

    fn push_line(&mut self, at: usize, text: String) {
        let ending = match self.lines.get(at.wrapping_sub(1)) {
            Some(previous) if previous.ending.is_empty() => {
                let ending = self.ending;
                self.lines[at - 1].ending = ending;
                ""
            }
            _ => self.ending,
        };
        self.lines.insert(at, Line { text, ending });
    }

    fn section_span(&self, section: &str) -> Option<(usize, usize)> {
        let start = self.lines.iter().position(|line| {
            section_name(&line.text)
                .map(|name| name.eq_ignore_ascii_case(section))
                .unwrap_or(false)
        })?;
        let end = self.lines[start + 1..]
            .iter()
            .position(|line| section_name(&line.text).is_some())
            .map(|offset| start + 1 + offset)
            .unwrap_or(self.lines.len());
        Some((start + 1, end))
    }

    fn open_section(&mut self, section: &str) -> (usize, usize) {
        if let Some(span) = self.section_span(section) {
            return span;
        }
        if self
            .lines
            .last()
            .map(|line| !line.text.trim().is_empty())
            .unwrap_or(false)
        {
            let at = self.lines.len();
            self.push_line(at, String::new());
        }
        let at = self.lines.len();
        self.push_line(at, format!("[{section}]"));
        (self.lines.len(), self.lines.len())
    }

    fn find(&self, span: (usize, usize), prefix: &str, key: &str) -> Option<usize> {
        (span.0..span.1).find(|index| {
            split_entry(&self.lines[*index].text)
                .map(|(mark, name, _)| mark == prefix && name.eq_ignore_ascii_case(key))
                .unwrap_or(false)
        })
    }

    fn insert_at(&mut self, span: (usize, usize), line: String) {
        let at = (span.0..span.1)
            .rev()
            .find(|index| !self.lines[*index].text.trim().is_empty())
            .map(|index| index + 1)
            .unwrap_or(span.1);
        self.push_line(at, line);
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        let span = self.section_span(section)?;
        let index = self.find(span, "", key)?;
        split_entry(&self.lines[index].text).map(|(_, _, value)| value)
    }

    pub fn values(&self, section: &str, key: &str) -> Vec<&str> {
        let Some(span) = self.section_span(section) else {
            return Vec::new();
        };
        (span.0..span.1)
            .filter_map(|index| split_entry(&self.lines[index].text))
            .filter(|(mark, name, _)| *mark == "+" && name.eq_ignore_ascii_case(key))
            .map(|(_, _, value)| value)
            .collect()
    }

    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        let span = self.open_section(section);
        match self.find(span, "", key) {
            Some(index) => {
                let (_, name, _) = split_entry(&self.lines[index].text).expect("entry");
                let name = name.to_string();
                let indent: String = self.lines[index]
                    .text
                    .chars()
                    .take_while(|character| character.is_whitespace())
                    .collect();
                self.lines[index].text = format!("{indent}{name}={value}");
                let duplicates: Vec<usize> = (index + 1..span.1)
                    .filter(|other| {
                        split_entry(&self.lines[*other].text)
                            .map(|(mark, existing, _)| {
                                mark.is_empty() && existing.eq_ignore_ascii_case(key)
                            })
                            .unwrap_or(false)
                    })
                    .collect();
                for other in duplicates.iter().rev() {
                    self.lines.remove(*other);
                }
            }
            None => self.insert_at(span, format!("{key}={value}")),
        }
    }

    pub fn remove(&mut self, section: &str, key: &str) -> bool {
        let Some(span) = self.section_span(section) else {
            return false;
        };
        let doomed: Vec<usize> = (span.0..span.1)
            .filter(|index| {
                split_entry(&self.lines[*index].text)
                    .map(|(_, name, _)| name.eq_ignore_ascii_case(key))
                    .unwrap_or(false)
            })
            .collect();
        for index in doomed.iter().rev() {
            self.lines.remove(*index);
        }
        !doomed.is_empty()
    }

    pub fn push(&mut self, section: &str, key: &str, value: &str) {
        let span = self.open_section(section);
        let present = (span.0..span.1).any(|index| {
            split_entry(&self.lines[index].text)
                .map(|(mark, name, existing)| {
                    mark == "+" && name.eq_ignore_ascii_case(key) && existing == value
                })
                .unwrap_or(false)
        });
        if !present {
            self.insert_at(span, format!("+{key}={value}"));
        }
    }

    pub fn pull(&mut self, section: &str, key: &str, value: &str) -> bool {
        let Some(span) = self.section_span(section) else {
            return false;
        };
        let doomed: Vec<usize> = (span.0..span.1)
            .filter(|index| {
                split_entry(&self.lines[*index].text)
                    .map(|(mark, name, existing)| {
                        mark == "+" && name.eq_ignore_ascii_case(key) && existing == value
                    })
                    .unwrap_or(false)
            })
            .collect();
        for index in doomed.iter().rev() {
            self.lines.remove(*index);
        }
        !doomed.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "; a comment\r\n[SystemSettings]\r\nMotionBlur=True\r\nShadowFilterQualityBias=0\r\n\r\n[Engine.Engine]\r\n+Paths=..\\Content\r\n+Paths=..\\Extra\r\n";

    #[test]
    fn an_untouched_file_renders_byte_for_byte() {
        assert_eq!(Config::parse(SAMPLE).render(), SAMPLE);
    }

    #[test]
    fn a_value_is_read_from_its_section() {
        let config = Config::parse(SAMPLE);
        assert_eq!(config.get("SystemSettings", "MotionBlur"), Some("True"));
        assert_eq!(config.get("Engine.Engine", "MotionBlur"), None);
    }

    #[test]
    fn setting_an_existing_key_keeps_everything_else() {
        let mut config = Config::parse(SAMPLE);
        config.set("SystemSettings", "MotionBlur", "False");
        let out = config.render();
        assert!(out.contains("MotionBlur=False"));
        assert!(out.starts_with("; a comment"));
        assert!(out.contains("+Paths=..\\Extra"));
        assert_eq!(out.lines().count(), SAMPLE.lines().count());
    }

    #[test]
    fn a_new_key_lands_inside_its_section() {
        let mut config = Config::parse(SAMPLE);
        config.set("SystemSettings", "MaxAnisotropy", "16");
        let out = config.render();
        let system = out.find("[SystemSettings]").unwrap();
        let engine = out.find("[Engine.Engine]").unwrap();
        let added = out.find("MaxAnisotropy=16").unwrap();
        assert!(system < added && added < engine);
    }

    #[test]
    fn a_missing_section_is_created() {
        let mut config = Config::parse(SAMPLE);
        config.set("S1.Custom", "Enabled", "True");
        let out = config.render();
        assert!(out.contains("[S1.Custom]"));
        assert!(out.contains("Enabled=True"));
        assert!(out.contains("MotionBlur=True"));
    }

    #[test]
    fn array_entries_are_added_once_and_removed_by_value() {
        let mut config = Config::parse(SAMPLE);
        config.push("Engine.Engine", "Paths", "..\\Extra");
        assert_eq!(config.values("Engine.Engine", "Paths").len(), 2);
        config.push("Engine.Engine", "Paths", "..\\Mine");
        assert_eq!(config.values("Engine.Engine", "Paths").len(), 3);
        assert!(config.pull("Engine.Engine", "Paths", "..\\Extra"));
        assert_eq!(
            config.values("Engine.Engine", "Paths"),
            vec!["..\\Content", "..\\Mine"]
        );
    }

    #[test]
    fn removing_a_key_reports_whether_it_was_there() {
        let mut config = Config::parse(SAMPLE);
        assert!(config.remove("SystemSettings", "MotionBlur"));
        assert!(!config.remove("SystemSettings", "MotionBlur"));
        assert!(!config.render().contains("MotionBlur"));
    }

    #[test]
    fn a_byte_order_mark_does_not_hide_the_first_section() {
        let text = "\u{feff}[SystemSettings]\r\nMotionBlur=True\r\n";
        let mut config = Config::parse(text);
        assert_eq!(config.get("SystemSettings", "MotionBlur"), Some("True"));
        assert_eq!(config.render(), text);
        config.set("SystemSettings", "MotionBlur", "False");
        let out = config.render();
        assert!(out.starts_with('\u{feff}'));
        assert_eq!(out.matches("[SystemSettings]").count(), 1);
    }

    #[test]
    fn mixed_line_endings_survive_untouched() {
        let text = "[A]\r\nKey=1\nOther=2\r\n";
        assert_eq!(Config::parse(text).render(), text);
    }

    #[test]
    fn setting_a_duplicated_key_leaves_no_contradiction() {
        let text = "[A]\nKey=1\nOther=9\nKey=2\n";
        let mut config = Config::parse(text);
        config.set("A", "Key", "7");
        let out = config.render();
        assert_eq!(out.matches("Key=").count(), 1, "{out}");
        assert!(out.contains("Key=7"));
        assert!(out.contains("Other=9"));
    }

    #[test]
    fn a_file_without_a_trailing_newline_keeps_it_that_way() {
        let text = "[A]\nKey=1";
        assert_eq!(Config::parse(text).render(), text);
    }
}
