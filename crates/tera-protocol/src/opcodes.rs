use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum OpcodeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("line {line}: `{text}` is not `NAME NUMBER`")]
    Malformed { line: usize, text: String },
}

#[derive(Default, Clone)]
pub struct OpcodeMap {
    pub revision: Option<u32>,
    by_name: HashMap<String, u16>,
    by_code: HashMap<u16, String>,
}

impl OpcodeMap {
    pub fn parse(text: &str) -> Result<Self, OpcodeError> {
        let mut map = Self::default();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (Some(name), Some(code)) = (parts.next(), parts.next()) else {
                return Err(OpcodeError::Malformed {
                    line: index + 1,
                    text: line.to_string(),
                });
            };
            let code: u16 = code.parse().map_err(|_| OpcodeError::Malformed {
                line: index + 1,
                text: line.to_string(),
            })?;
            map.by_name.insert(name.to_string(), code);
            map.by_code.insert(code, name.to_string());
        }
        Ok(map)
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self, OpcodeError> {
        let path = path.as_ref();
        let mut map = Self::parse(&std::fs::read_to_string(path)?)?;
        map.revision = path
            .file_stem()
            .and_then(|stem| stem.to_string_lossy().rsplit('.').next().map(str::to_owned))
            .and_then(|value| value.parse().ok());
        Ok(map)
    }

    pub fn code(&self, name: &str) -> Option<u16> {
        self.by_name.get(name).copied()
    }

    pub fn name(&self, code: u16) -> Option<&str> {
        self.by_code.get(&code).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = (&str, u16)> {
        self.by_name.iter().map(|(name, code)| (name.as_str(), *code))
    }
}
