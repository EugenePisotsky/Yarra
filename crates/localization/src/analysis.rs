//! Walk every Fluent branch before publication or rendering. No sample-value validation.
use crate::*;
use fluent_syntax::ast::*;

pub(crate) type Patterns = BTreeMap<String, Pattern<String>>;
pub(crate) fn parse(source: &str, patterns: &mut Patterns) -> Result<()> {
    // Bound recursion before invoking Fluent's recursive parser. Braces inside quoted
    // expression literals/comments do not count; quotes in ordinary prose are plain text.
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for line in source.lines() {
        if depth == 0 && line.trim_start().starts_with('#') {
            continue;
        }
        for byte in line.bytes() {
            if quoted {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    quoted = false;
                }
            } else {
                match byte {
                    b'"' if depth > 0 => quoted = true,
                    b'{' => {
                        depth += 1;
                        if depth > 64 {
                            return Err(resource_error("Fluent expression nesting exceeds 64"));
                        }
                    }
                    b'}' => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
        }
    }
    let resource = fluent_syntax::parser::parse(source.to_owned())
        .map_err(|(_, e)| LocalizationError::Resource(format!("Fluent syntax: {e:?}")))?;
    for entry in resource.body {
        let (id, value, attributes) = match entry {
            Entry::Message(m) => (m.id.name, m.value, m.attributes),
            Entry::Term(t) => (format!("-{}", t.id.name), Some(t.value), t.attributes),
            Entry::Comment(_) | Entry::GroupComment(_) | Entry::ResourceComment(_) => continue,
            Entry::Junk { .. } => return Err(resource_error("Fluent junk")),
        };
        if let Some(value) = value {
            insert(patterns, id.clone(), value)?;
        }
        for attr in attributes {
            insert(patterns, format!("{id}.{}", attr.id.name), attr.value)?;
        }
    }
    Ok(())
}
fn insert(patterns: &mut Patterns, id: String, value: Pattern<String>) -> Result<()> {
    if patterns.insert(id.clone(), value).is_some() {
        return Err(resource_error(format!("duplicate Fluent key {id}")));
    }
    Ok(())
}
fn reference(id: &str, attribute: &Option<Identifier<String>>) -> String {
    attribute
        .as_ref()
        .map_or_else(|| id.to_owned(), |a| format!("{id}.{}", a.name))
}
#[derive(Clone)]
enum ValueType {
    Text,
    Number,
    Select(BTreeSet<String>),
    LiteralText(String),
}
impl ValueType {
    fn declared(kind: &ArgumentType) -> Self {
        match kind {
            ArgumentType::Text => Self::Text,
            ArgumentType::Number => Self::Number,
            ArgumentType::Select(values) => Self::Select(values.clone()),
        }
    }
    fn compatible(&self, expected: &ArgumentType) -> bool {
        match (self, expected) {
            (Self::Number, ArgumentType::Number) => true,
            (Self::Text | Self::Select(_) | Self::LiteralText(_), ArgumentType::Text) => true,
            (Self::Select(values), ArgumentType::Select(allowed)) => values.is_subset(allowed),
            (Self::LiteralText(value), ArgumentType::Select(allowed)) => allowed.contains(value),
            _ => false,
        }
    }
}
struct Walk<'a> {
    patterns: &'a Patterns,
    contracts: &'a BTreeMap<TextKey, MessageContract>,
    active: BTreeSet<String>,
    used: BTreeSet<String>,
    remaining: usize,
}
impl Walk<'_> {
    fn node(&mut self, id: &str, args: &BTreeMap<String, ValueType>) -> Result<()> {
        if self.active.len() >= 64 || self.remaining == 0 {
            return Err(resource_error("Fluent expression budget exceeded"));
        }
        if !self.active.insert(id.into()) {
            return Err(resource_error(format!("Fluent reference cycle: {id}")));
        }
        self.used.insert(id.into());
        if !id.starts_with('-') {
            let key = TextKey::new(id.split('.').next().unwrap())
                .map_err(|e| resource_error(e.to_string()))?;
            let contract = self
                .contracts
                .get(&key)
                .ok_or_else(|| resource_error(format!("undeclared message {id}")))?;
            for (name, expected) in &contract.arguments {
                if !args
                    .get(name)
                    .is_some_and(|actual| actual.compatible(expected))
                {
                    return Err(resource_error(format!(
                        "argument ${name} is incompatible with referenced message {id}"
                    )));
                }
            }
        }
        let pattern = self
            .patterns
            .get(id)
            .ok_or_else(|| LocalizationError::Missing(id.into()))?;
        self.pattern(pattern, args)?;
        self.active.remove(id);
        Ok(())
    }
    fn pattern(
        &mut self,
        pattern: &Pattern<String>,
        args: &BTreeMap<String, ValueType>,
    ) -> Result<()> {
        for element in &pattern.elements {
            self.remaining = self
                .remaining
                .checked_sub(1)
                .ok_or_else(|| resource_error("Fluent expression budget exceeded"))?;
            if let PatternElement::Placeable { expression } = element {
                self.expression(expression, args)?;
            }
        }
        Ok(())
    }
    fn expression(
        &mut self,
        expr: &Expression<String>,
        args: &BTreeMap<String, ValueType>,
    ) -> Result<ValueType> {
        match expr {
            Expression::Inline(inline) => self.inline(inline, args),
            Expression::Select { selector, variants } => {
                let kind = self.inline(selector, args)?;
                for variant in variants {
                    let valid = match (&kind, &variant.key) {
                        (ValueType::Number, VariantKey::NumberLiteral { .. }) => true,
                        (ValueType::Number, VariantKey::Identifier { name }) => {
                            ["zero", "one", "two", "few", "many", "other"].contains(&name.as_str())
                        }
                        (ValueType::Select(values), VariantKey::Identifier { name }) => {
                            values.contains(name) || (variant.default && name == "other")
                        }
                        (ValueType::LiteralText(_), VariantKey::Identifier { .. }) => true,
                        _ => false,
                    };
                    if !valid {
                        return Err(resource_error(
                            "selector variant does not match argument contract",
                        ));
                    }
                    self.pattern(&variant.value, args)?;
                }
                Ok(ValueType::Text)
            }
        }
    }
    fn inline(
        &mut self,
        expr: &InlineExpression<String>,
        args: &BTreeMap<String, ValueType>,
    ) -> Result<ValueType> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or_else(|| resource_error("Fluent expression budget exceeded"))?;
        match expr {
            InlineExpression::StringLiteral { value } => Ok(ValueType::LiteralText(value.clone())),
            InlineExpression::NumberLiteral { .. } => Ok(ValueType::Number),
            InlineExpression::VariableReference { id } => args
                .get(&id.name)
                .cloned()
                .ok_or_else(|| resource_error(format!("undeclared argument ${}", id.name))),
            InlineExpression::MessageReference { id, attribute } => {
                self.node(&reference(&id.name, attribute), args)?;
                Ok(ValueType::Text)
            }
            InlineExpression::TermReference {
                id,
                attribute,
                arguments,
            } => {
                let mut local = BTreeMap::new();
                if let Some(arguments) = arguments {
                    if !arguments.positional.is_empty() {
                        return Err(resource_error("positional term arguments are unsupported"));
                    }
                    for named in &arguments.named {
                        let kind = self.inline(&named.value, args)?;
                        local.insert(named.name.name.clone(), kind);
                    }
                }
                // Term parameters have local scope; ambient caller variables cannot leak in.
                self.node(&reference(&format!("-{}", id.name), attribute), &local)?;
                Ok(ValueType::Text)
            }
            InlineExpression::FunctionReference { id, .. } => Err(resource_error(format!(
                "unsupported Fluent function {}",
                id.name
            ))),
            InlineExpression::Placeable { expression } => self.expression(expression, args),
        }
    }
}
pub(crate) fn dependencies(
    patterns: &Patterns,
    contracts: &BTreeMap<TextKey, MessageContract>,
    key: &str,
    args: &BTreeMap<String, ArgumentType>,
) -> Result<BTreeSet<String>> {
    let mut walk = Walk {
        patterns,
        contracts,
        active: BTreeSet::new(),
        used: BTreeSet::new(),
        remaining: 100_000,
    };
    let args = args
        .iter()
        .map(|(key, kind)| (key.clone(), ValueType::declared(kind)))
        .collect();
    walk.node(key, &args)?;
    Ok(walk.used)
}
pub(crate) fn revision(
    patterns: &Patterns,
    contracts: &BTreeMap<TextKey, MessageContract>,
    key: &str,
    args: &BTreeMap<String, ArgumentType>,
) -> Result<String> {
    let used = dependencies(patterns, contracts, key, args)?;
    let mut hash = blake3::Hasher::new();
    for key in used {
        let value = serde_json::to_vec(&(&key, &patterns[&key]))
            .map_err(|e| resource_error(e.to_string()))?;
        hash.update(&(value.len() as u64).to_le_bytes());
        hash.update(&value);
    }
    Ok(hash.finalize().to_hex().to_string())
}

/// Structural validation includes unused terms/attributes and every branch. Missing translated
/// dependencies are allowed only when the source scope defines them (whole-message fallback).
pub(crate) fn validate_graph(patterns: &Patterns, source: Option<&Patterns>) -> Result<()> {
    fn inline(
        expr: &InlineExpression<String>,
        edges: &mut BTreeSet<String>,
        budget: &mut usize,
    ) -> Result<()> {
        *budget = budget
            .checked_sub(1)
            .ok_or_else(|| resource_error("Fluent graph budget exceeded"))?;
        match expr {
            InlineExpression::FunctionReference { id, .. } => {
                return Err(resource_error(format!(
                    "unsupported Fluent function {}",
                    id.name
                )));
            }
            InlineExpression::MessageReference { id, attribute } => {
                edges.insert(reference(&id.name, attribute));
            }
            InlineExpression::TermReference {
                id,
                attribute,
                arguments,
            } => {
                edges.insert(reference(&format!("-{}", id.name), attribute));
                if let Some(args) = arguments {
                    if !args.positional.is_empty() {
                        return Err(resource_error("positional term arguments are unsupported"));
                    }
                    let mut names = BTreeSet::new();
                    for arg in &args.named {
                        if !names.insert(&arg.name.name) {
                            return Err(resource_error("duplicate term argument"));
                        }
                        inline(&arg.value, edges, budget)?;
                    }
                }
            }
            InlineExpression::Placeable { expression: value } => expression(value, edges, budget)?,
            _ => {}
        }
        Ok(())
    }
    fn expression(
        expr: &Expression<String>,
        edges: &mut BTreeSet<String>,
        budget: &mut usize,
    ) -> Result<()> {
        match expr {
            Expression::Inline(value) => inline(value, edges, budget),
            Expression::Select { selector, variants } => {
                inline(selector, edges, budget)?;
                let mut keys = BTreeSet::new();
                for variant in variants {
                    let key = match &variant.key {
                        VariantKey::Identifier { name } => format!("s:{name}"),
                        VariantKey::NumberLiteral { value } => format!("n:{value}"),
                    };
                    if !keys.insert(key) {
                        return Err(resource_error("duplicate selector variant"));
                    }
                    pattern(&variant.value, edges, budget)?;
                }
                Ok(())
            }
        }
    }
    fn pattern(
        value: &Pattern<String>,
        edges: &mut BTreeSet<String>,
        budget: &mut usize,
    ) -> Result<()> {
        for part in &value.elements {
            if let PatternElement::Placeable { expression: value } = part {
                expression(value, edges, budget)?;
            }
        }
        Ok(())
    }
    fn visit<'a>(
        key: &'a str,
        graph: &'a BTreeMap<String, BTreeSet<String>>,
        active: &mut BTreeSet<&'a str>,
        done: &mut BTreeSet<&'a str>,
    ) -> Result<()> {
        if active.contains(key) {
            return Err(resource_error(format!("Fluent reference cycle: {key}")));
        }
        if done.contains(key) {
            return Ok(());
        }
        if active.len() >= 64 {
            return Err(resource_error("Fluent reference depth exceeded"));
        }
        active.insert(key);
        if let Some(edges) = graph.get(key) {
            for edge in edges {
                visit(edge, graph, active, done)?;
            }
        }
        active.remove(key);
        done.insert(key);
        Ok(())
    }
    let mut graph = BTreeMap::new();
    let mut budget = 100_000;
    for (key, value) in patterns {
        let mut edges = BTreeSet::new();
        pattern(value, &mut edges, &mut budget)?;
        for edge in &edges {
            if !patterns.contains_key(edge) && !source.is_some_and(|p| p.contains_key(edge)) {
                return Err(resource_error(format!("unknown Fluent dependency {edge}")));
            }
        }
        graph.insert(key.clone(), edges);
    }
    let mut done = BTreeSet::new();
    for key in graph.keys() {
        visit(key, &graph, &mut BTreeSet::new(), &mut done)?;
    }
    Ok(())
}
