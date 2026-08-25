use crate::error::{DataCenterError, Result};
use crate::node::Node;

#[derive(Clone, Debug)]
pub struct QueryStep {
    pub name: Option<String>,
    pub descendant: bool,
    pub index: Option<usize>,
    pub filters: Vec<Filter>,
}

#[derive(Clone, Debug)]
pub struct Filter {
    pub attribute: String,
    pub expected: Option<String>,
}

pub fn parse_query(path: &str) -> Result<Vec<QueryStep>> {
    let mut steps = Vec::new();
    let mut rest = path.trim();
    if rest.is_empty() {
        return Ok(steps);
    }
    let mut descendant = false;
    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix("//") {
            descendant = true;
            rest = stripped;
            continue;
        }
        if let Some(stripped) = rest.strip_prefix('/') {
            rest = stripped;
            continue;
        }
        let end = rest
            .find('/')
            .filter(|position| !inside_brackets(rest, *position))
            .unwrap_or(rest.len());
        let (segment, remainder) = rest.split_at(end);
        rest = remainder;
        if segment.is_empty() {
            continue;
        }
        steps.push(parse_step(segment, descendant)?);
        descendant = false;
    }
    Ok(steps)
}

fn inside_brackets(text: &str, position: usize) -> bool {
    let mut depth = 0usize;
    for (index, character) in text.char_indices() {
        if index >= position {
            break;
        }
        match character {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth > 0
}

fn parse_step(segment: &str, descendant: bool) -> Result<QueryStep> {
    let bracket = segment.find('[').unwrap_or(segment.len());
    let (name, predicates) = segment.split_at(bracket);
    let mut step = QueryStep {
        name: (name != "*" && !name.is_empty()).then(|| name.to_string()),
        descendant,
        index: None,
        filters: Vec::new(),
    };
    let mut rest = predicates;
    while let Some(open) = rest.find('[') {
        let close = rest[open..]
            .find(']')
            .ok_or_else(|| DataCenterError::Query(format!("unclosed predicate in `{segment}`")))?
            + open;
        let predicate = &rest[open + 1..close];
        rest = &rest[close + 1..];
        if let Some(attribute) = predicate.strip_prefix('@') {
            let (attribute, expected) = match attribute.split_once('=') {
                Some((left, right)) => (left, Some(unquote(right).to_string())),
                None => (attribute, None),
            };
            step.filters.push(Filter {
                attribute: attribute.to_string(),
                expected,
            });
        } else if let Ok(index) = predicate.parse::<usize>() {
            step.index = Some(index);
        } else {
            return Err(DataCenterError::Query(format!(
                "unsupported predicate `{predicate}`"
            )));
        }
    }
    Ok(step)
}

fn unquote(text: &str) -> &str {
    let trimmed = text.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(trimmed)
}

pub fn query_builder(builder: &crate::build::Builder, path: &str) -> Result<Vec<u32>> {
    let steps = parse_query(path)?;
    let mut current = vec![builder.root];
    for step in &steps {
        let mut next = Vec::new();
        for id in &current {
            if step.descendant {
                let mut stack = vec![*id];
                while let Some(current_id) = stack.pop() {
                    for child in &builder.node(current_id).children {
                        if matches_builder(builder, *child, step) {
                            next.push(*child);
                        }
                        stack.push(*child);
                    }
                }
            } else {
                for child in &builder.node(*id).children {
                    if matches_builder(builder, *child, step) {
                        next.push(*child);
                    }
                }
            }
        }
        if let Some(index) = step.index {
            next = next.into_iter().nth(index).into_iter().collect();
        }
        current = next;
    }
    Ok(current)
}

fn matches_builder(builder: &crate::build::Builder, id: u32, step: &QueryStep) -> bool {
    let node = builder.node(id);
    if let Some(name) = &step.name {
        if builder.names.get(node.name) != name {
            return false;
        }
    }
    for filter in &step.filters {
        let found = node.attributes.iter().find(|(name, _)| {
            builder.names.get(*name) == filter.attribute
        });
        let Some((_, value)) = found else {
            return false;
        };
        if let Some(expected) = &filter.expected {
            if builder.value_text(*value) != *expected {
                return false;
            }
        }
    }
    true
}

pub fn query<'a>(root: Node<'a>, path: &str) -> Result<Vec<Node<'a>>> {
    let steps = parse_query(path)?;
    let mut current = vec![root];
    for step in &steps {
        let mut next = Vec::new();
        for node in &current {
            if step.descendant {
                collect_descendants(node, step, &mut next)?;
            } else {
                for child in node.children() {
                    if matches(&child, step)? {
                        next.push(child);
                    }
                }
            }
        }
        if let Some(index) = step.index {
            let selected = next.into_iter().nth(index);
            next = selected.into_iter().collect();
        }
        current = next;
    }
    Ok(current)
}

fn collect_descendants<'a>(
    node: &Node<'a>,
    step: &QueryStep,
    out: &mut Vec<Node<'a>>,
) -> Result<()> {
    let mut stack = vec![*node];
    while let Some(current) = stack.pop() {
        for child in current.children() {
            if matches(&child, step)? {
                out.push(child);
            }
            stack.push(child);
        }
    }
    Ok(())
}

fn matches(node: &Node<'_>, step: &QueryStep) -> Result<bool> {
    if let Some(name) = &step.name {
        if node.name()? != name {
            return Ok(false);
        }
    }
    for filter in &step.filters {
        let Some(attribute) = node.attribute(&filter.attribute) else {
            return Ok(false);
        };
        if let Some(expected) = &filter.expected {
            if attribute.value()?.to_text() != *expected {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
