use tera_protocol::OpcodeMap;

pub fn describe(message: &str, map: &OpcodeMap) -> String {
    let body = message.strip_prefix('@').unwrap_or(message);
    let tokens: Vec<&str> = body.split('\u{b}').collect();
    let Some((id_token, rest)) = tokens.split_first() else {
        return message.to_string();
    };
    let name = id_token
        .parse::<u16>()
        .ok()
        .and_then(|id| map.name(id))
        .map(str::to_string)
        .unwrap_or_else(|| format!("SYSMSG_{id_token}"));
    if rest.is_empty() {
        return name;
    }
    let params: Vec<String> = rest
        .chunks(2)
        .map(|pair| match pair {
            [key, value] => format!("{key}={value}"),
            [key] => format!("{key}=?"),
            _ => String::new(),
        })
        .collect();
    format!("{name} {{{}}}", params.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_id_and_params() {
        let map = OpcodeMap::parse("SMT_YOU_DIE 100\nSMT_GOLD_EARNED 200\n").unwrap();
        assert_eq!(
            describe("@100\u{b}name\u{b}Meow\u{b}count\u{b}5", &map),
            "SMT_YOU_DIE {name=Meow, count=5}"
        );
        assert_eq!(describe("@200", &map), "SMT_GOLD_EARNED");
        assert_eq!(describe("@999", &map), "SYSMSG_999");
    }
}
