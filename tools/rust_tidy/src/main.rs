//! Adds the blank lines rustfmt leaves to the author: one between adjacent items, and one
//! around a function's multi-line steps. It only ever inserts empty lines, so
//! `git diff --ignore-blank-lines` after `--write` is empty by construction.
//!
//! Rules, applied where no blank line separates the two already:
//! - Items in a file, an inline module, an impl or a trait are separated, except runs of
//!   imports and of `mod x;` declarations, and runs of one kind of one-line declaration
//!   (consts, statics, type aliases).
//! - In a block of three or more statements, a multi-line `if`, `match`, `for`, `while`,
//!   `loop` or block is separated from its neighbours, and a comment that opens a statement
//!   starts a paragraph.
//!
//! Usage: `cargo run -p yarra-tidy -- [--write] [PATH...]`. Without `--write` it lists the
//! missing lines and fails if there are any. Paths default to `crates` and `tools`.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use syn::{Expr, ImplItem, Item, Stmt, TraitItem, spanned::Spanned, visit::Visit};

/// What kind of one-line declaration an item is, for keeping runs of them together.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Use,
    ModDeclaration,
    Const,
    Static,
    Type,
    ExternCrate,
    Associated,
    Other,
}

/// An item's lines, 1-based, its outer attributes and doc comments included.
struct Lines {
    first: usize,
    last: usize,
    kind: Kind,
}

impl Lines {
    fn of(span: proc_macro2::Span, kind: Kind) -> Self {
        Self {
            first: span.start().line,
            last: span.end().line,
            kind,
        }
    }
}

/// Collects "a blank line belongs after line N" for one file.
struct Gaps<'a> {
    source: &'a [&'a str],
    after: BTreeSet<usize>,
}

impl Gaps<'_> {
    /// Whether a blank line already separates two line ranges.
    fn separated(&self, last: usize, first: usize) -> bool {
        self.source[last..first - 1]
            .iter()
            .any(|line| line.trim().is_empty())
    }

    fn items(&mut self, items: &[Item]) {
        let lines: Vec<_> = items
            .iter()
            .map(|item| Lines::of(item.span(), item_kind(item)))
            .collect();
        self.separate(&lines);

        for item in items {
            match item {
                Item::Mod(module) => {
                    if let Some((_, inner)) = &module.content {
                        self.items(inner);
                    }
                }
                Item::Impl(block) => {
                    let lines: Vec<_> = block
                        .items
                        .iter()
                        .map(|item| {
                            let one_line = matches!(item, ImplItem::Const(_) | ImplItem::Type(_));
                            Lines::of(item.span(), associated(one_line))
                        })
                        .collect();
                    self.separate(&lines);
                }
                Item::Trait(block) => {
                    let lines: Vec<_> = block
                        .items
                        .iter()
                        .map(|item| {
                            let one_line = matches!(item, TraitItem::Const(_) | TraitItem::Type(_));
                            Lines::of(item.span(), associated(one_line))
                        })
                        .collect();
                    self.separate(&lines);
                }
                _ => {}
            }
        }
    }

    fn separate(&mut self, items: &[Lines]) {
        for pair in items.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);

            if b.first <= a.last || self.separated(a.last, b.first) {
                continue;
            }

            let one_line = a.first == a.last && b.first == b.last;
            let declarations =
                matches!(a.kind, Kind::Use | Kind::ModDeclaration | Kind::ExternCrate);

            if declarations && a.kind == b.kind {
                continue;
            }

            if one_line && a.kind == b.kind && a.kind != Kind::Other {
                continue;
            }

            self.after.insert(a.last);
        }
    }
}

impl<'ast> Visit<'ast> for Gaps<'_> {
    fn visit_block(&mut self, block: &'ast syn::Block) {
        if block.stmts.len() >= 3 {
            for pair in block.stmts.windows(2) {
                let (a, b) = (&pair[0], &pair[1]);

                if matches!(a, Stmt::Item(_)) || matches!(b, Stmt::Item(_)) {
                    continue;
                }

                let (a_span, b_span) = (a.span(), b.span());
                let (a_first, a_last) = (a_span.start().line, a_span.end().line);
                let (b_first, b_last) = (b_span.start().line, b_span.end().line);

                if b_first <= a_last || self.separated(a_last, b_first) {
                    continue;
                }

                let commented = self.source[a_last..b_first - 1]
                    .iter()
                    .any(|line| line.trim_start().starts_with("//"));
                let step = |stmt: &Stmt, first: usize, last: usize| compound(stmt) && last > first;

                if commented || step(a, a_first, a_last) || step(b, b_first, b_last) {
                    self.after.insert(a_last);
                }
            }
        }
        syn::visit::visit_block(self, block);
    }
}

fn item_kind(item: &Item) -> Kind {
    match item {
        Item::Use(_) => Kind::Use,
        Item::Mod(module) if module.content.is_none() => Kind::ModDeclaration,
        Item::Const(_) => Kind::Const,
        Item::Static(_) => Kind::Static,
        Item::Type(_) => Kind::Type,
        Item::ExternCrate(_) => Kind::ExternCrate,
        _ => Kind::Other,
    }
}

fn associated(one_line: bool) -> Kind {
    if one_line {
        Kind::Associated
    } else {
        Kind::Other
    }
}

/// A statement with a body of its own.
fn compound(stmt: &Stmt) -> bool {
    let Stmt::Expr(expr, _) = stmt else {
        return false;
    };
    matches!(
        expr,
        Expr::If(_)
            | Expr::ForLoop(_)
            | Expr::While(_)
            | Expr::Loop(_)
            | Expr::Match(_)
            | Expr::Block(_)
            | Expr::Unsafe(_)
    )
}

/// The lines a blank line belongs after, or `None` when the file does not parse.
fn missing(source: &str) -> Option<BTreeSet<usize>> {
    let file = syn::parse_file(source).ok()?;
    let lines: Vec<&str> = source.split('\n').collect();
    let mut gaps = Gaps {
        source: &lines,
        after: BTreeSet::new(),
    };
    gaps.items(&file.items);
    gaps.visit_file(&file);
    Some(gaps.after)
}

fn insert(source: &str, after: &BTreeSet<usize>) -> String {
    let lines: Vec<&str> = source.split('\n').collect();
    let mut out = String::with_capacity(source.len() + after.len());

    for (index, line) in lines.iter().enumerate() {
        out.push_str(line);

        if index + 1 < lines.len() {
            out.push('\n');
        }

        if after.contains(&(index + 1)) {
            out.push('\n');
        }
    }

    out
}

fn rust_files(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_file() {
        if path.extension().is_some_and(|e| e == "rs") {
            files.push(path.to_owned());
        }
        return;
    }

    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();

    for entry in entries {
        let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name != "target" && !name.starts_with('.') {
            rust_files(&entry, files);
        }
    }
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let write = args.first().is_some_and(|a| a == "--write");

    if write {
        args.remove(0);
    }

    if args.is_empty() {
        // Run from anywhere in the workspace: paths are relative to its root.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        args = ["crates", "tools"]
            .map(|p| root.join(p).to_string_lossy().into_owned())
            .into();
    }

    let mut files = Vec::new();

    for arg in &args {
        rust_files(Path::new(arg), &mut files);
    }

    let mut total = 0;

    for path in &files {
        let Ok(source) = fs::read_to_string(path) else {
            continue;
        };
        let Some(after) = missing(&source) else {
            eprintln!("{}: does not parse, skipped", path.display());
            continue;
        };

        if after.is_empty() {
            continue;
        }

        total += after.len();

        if write {
            if let Err(error) = fs::write(path, insert(&source, &after)) {
                eprintln!("{}: {error}", path.display());
                return ExitCode::FAILURE;
            }
        } else {
            for line in &after {
                println!(
                    "{}:{line}: blank line missing after this line",
                    path.display()
                );
            }
        }
    }

    if write {
        println!("{total} blank lines added");
        ExitCode::SUCCESS
    } else if total > 0 {
        eprintln!("{total} blank lines missing; run `cargo run -p yarra-tidy -- --write`");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tidy(source: &str) -> String {
        insert(source, &missing(source).unwrap())
    }

    #[test]
    fn items_are_separated_but_one_line_runs_stay_together() {
        let source = "use a::{\n    b,\n};\nuse c::d;\nconst A: u8 = 1;\nconst B: u8 = 2;\n/// Doc.\nfn f() {}\nstruct S;\nimpl S {\n    const C: u8 = 3;\n    fn g() {}\n    // Why.\n    fn h() {}\n}\n";
        let expected = "use a::{\n    b,\n};\nuse c::d;\n\nconst A: u8 = 1;\nconst B: u8 = 2;\n\n/// Doc.\nfn f() {}\n\nstruct S;\n\nimpl S {\n    const C: u8 = 3;\n\n    fn g() {}\n\n    // Why.\n    fn h() {}\n}\n";
        assert_eq!(tidy(source), expected);
    }

    #[test]
    fn multi_line_steps_and_commented_steps_get_room() {
        let source = "fn f(v: &[u8]) -> u8 {\n    let mut n = 0;\n    for x in v {\n        n += x;\n    }\n    let m = n;\n    // Why.\n    let k = m;\n    if k > 1 { return 1; }\n    k\n}\n";
        let expected = "fn f(v: &[u8]) -> u8 {\n    let mut n = 0;\n\n    for x in v {\n        n += x;\n    }\n\n    let m = n;\n\n    // Why.\n    let k = m;\n    if k > 1 { return 1; }\n    k\n}\n";
        assert_eq!(tidy(source), expected);
    }

    #[test]
    fn short_blocks_and_existing_blank_lines_are_left_alone() {
        let source = "fn f(x: bool) -> u8 {\n    if x {\n        1\n    } else {\n        2\n    }\n}\n\nfn g() {}\n";
        assert_eq!(tidy(source), source);
        let tidied = tidy("fn a() {}\nfn b() {}\n");
        assert_eq!(tidy(&tidied), tidied);
    }
}
