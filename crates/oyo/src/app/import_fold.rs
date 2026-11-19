//! Detection of import blocks, so `ui.fold_imports` can collapse unchanged ones.
//!
//! Detection reads line text only. It recognises the common statement shapes of each
//! language, including statements that span several lines, and stops at anything else.

/// Import statement syntax, chosen from the file extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImportSyntax {
    Python,
    JavaScript,
    Rust,
    Go,
}

impl ImportSyntax {
    pub(crate) fn from_path(path: &str) -> Option<Self> {
        let ext = path.rsplit_once('.')?.1;
        match ext {
            "py" | "pyi" => Some(Self::Python),
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" => Some(Self::JavaScript),
            "rs" => Some(Self::Rust),
            "go" => Some(Self::Go),
            _ => None,
        }
    }
}

/// Number of lines the import statement starting at `lines[0]` spans, or 0 when
/// `lines[0]` does not start an import statement.
pub(crate) fn import_statement_len(lines: &[&str], syntax: ImportSyntax) -> usize {
    let Some(first) = lines.first().map(|line| line.trim()) else {
        return 0;
    };
    // Lines up to and including the first that satisfies `ends`, or 0 if none does.
    let until = |ends: &dyn Fn(&str) -> bool| -> usize {
        lines
            .iter()
            .position(|line| ends(line.trim()))
            .map_or(0, |idx| idx + 1)
    };
    match syntax {
        ImportSyntax::Python => {
            let is_import = first.starts_with("import ")
                || (first.starts_with("from ") && first.contains(" import"));
            if !is_import {
                0
            } else if first.ends_with('(') || (first.contains('(') && !first.contains(')')) {
                until(&|line| line.contains(')'))
            } else if first.ends_with('\\') {
                until(&|line| !line.ends_with('\\'))
            } else {
                1
            }
        }
        ImportSyntax::JavaScript => {
            let is_import = first.starts_with("import ")
                || first.starts_with("import{")
                || first.starts_with("import type ");
            if !is_import {
                0
            } else if first.contains('{') && !first.contains('}') {
                until(&|line| line.contains('}'))
            } else {
                1
            }
        }
        ImportSyntax::Rust => {
            let is_import = first.starts_with("use ")
                || first.starts_with("pub use ")
                || first.starts_with("pub(crate) use ");
            if is_import {
                until(&|line| line.ends_with(';'))
            } else {
                0
            }
        }
        ImportSyntax::Go => {
            if first == "import (" || (first.starts_with("import (") && !first.contains(')')) {
                until(&|line| line == ")")
            } else if first.starts_with("import ") {
                1
            } else {
                0
            }
        }
    }
}

/// Number of lines in the run of import statements starting at `lines[0]`. Blank lines
/// between statements belong to the run; blank lines after the last one do not.
pub(crate) fn import_run_len(lines: &[&str], syntax: ImportSyntax) -> usize {
    let mut end = 0;
    let mut idx = 0;
    while idx < lines.len() {
        if lines[idx].trim().is_empty() {
            idx += 1;
            continue;
        }
        let len = import_statement_len(&lines[idx..], syntax);
        if len == 0 {
            break;
        }
        idx += len;
        end = idx;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_follows_the_file_extension() {
        assert_eq!(
            ImportSyntax::from_path("rowspace/core/x.py"),
            Some(ImportSyntax::Python)
        );
        assert_eq!(
            ImportSyntax::from_path("frontend/src/a.tsx"),
            Some(ImportSyntax::JavaScript)
        );
        assert_eq!(
            ImportSyntax::from_path("src/main.rs"),
            Some(ImportSyntax::Rust)
        );
        assert_eq!(ImportSyntax::from_path("cmd/x.go"), Some(ImportSyntax::Go));
        assert_eq!(ImportSyntax::from_path("README.md"), None);
        assert_eq!(ImportSyntax::from_path("Makefile"), None);
    }

    #[test]
    fn python_imports_including_parenthesised_and_continued_ones() {
        let lines = [
            "import os",
            "from typing import (",
            "    Any,",
            "    cast,",
            ")",
            "",
            "from x.y import z \\",
            "    as w",
            "",
            "",
            "def main():",
        ];
        assert_eq!(import_statement_len(&lines, ImportSyntax::Python), 1);
        assert_eq!(import_statement_len(&lines[1..], ImportSyntax::Python), 4);
        assert_eq!(import_statement_len(&lines[6..], ImportSyntax::Python), 2);
        assert_eq!(
            import_run_len(&lines, ImportSyntax::Python),
            8,
            "stops before trailing blanks"
        );
        assert_eq!(import_statement_len(&lines[10..], ImportSyntax::Python), 0);
    }

    #[test]
    fn python_from_without_import_is_not_an_import() {
        assert_eq!(
            import_statement_len(&["from_value = 3"], ImportSyntax::Python),
            0
        );
    }

    #[test]
    fn javascript_imports_including_multi_line_braces() {
        let lines = [
            "import React from \"react\";",
            "import {",
            "  useState,",
            "  useEffect,",
            "} from \"react\";",
            "import type { Foo } from \"./foo\";",
            "export const x = 1;",
        ];
        assert_eq!(import_run_len(&lines, ImportSyntax::JavaScript), 6);
        assert_eq!(
            import_statement_len(&lines[1..], ImportSyntax::JavaScript),
            4
        );
    }

    #[test]
    fn rust_use_statements_end_at_the_semicolon() {
        let lines = [
            "use std::fs;",
            "use crate::{",
            "    a,",
            "    b,",
            "};",
            "",
            "fn main() {}",
        ];
        assert_eq!(import_run_len(&lines, ImportSyntax::Rust), 5);
    }

    #[test]
    fn go_import_blocks() {
        let lines = [
            "import (",
            "\t\"fmt\"",
            "\t\"os\"",
            ")",
            "",
            "func main() {}",
        ];
        assert_eq!(import_run_len(&lines, ImportSyntax::Go), 4);
        assert_eq!(
            import_statement_len(&["import \"fmt\""], ImportSyntax::Go),
            1
        );
    }

    #[test]
    fn an_unterminated_statement_is_not_counted() {
        assert_eq!(
            import_statement_len(&["from x import (", "    a,"], ImportSyntax::Python),
            0
        );
    }
}
