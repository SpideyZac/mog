//! Finding errors and warnings in the output of compilers, linters and test runners.

use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
};

use regex::Regex;

use crate::diagnostic::Severity;

/// A problem a task reported, in a file that may not be open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskProblem {
    /// The file it is in.
    pub path: PathBuf,
    /// The line, from 0.
    pub line: usize,
    /// The column, from 0.
    pub column: usize,
    /// How bad it is.
    pub severity: Severity,
    /// What is wrong.
    pub message: String,
}

/// A header like `error[E0308]: mismatched types` from rustc and cargo.
static RUST_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(error|warning)(?:\[\w+\])?: (.+)$").expect("valid regex"));

/// The ` --> src/main.rs:4:18` line that follows a rustc header.
static RUST_LOCATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*--> (.+?):(\d+):(\d+)$").expect("valid regex"));

/// `file:line:col: severity: message`, used by gcc, clang, go, mypy, ruff, eslint and others.
static COLON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^([^\s:][^:]*?|[A-Za-z]:[^:]+?):(\d+)(?::(\d+))?:\s*(?:(fatal error|error|warning|note|info)\s*:\s*)?(.+)$",
    )
    .expect("valid regex")
});

/// `file(line,col): error TS1234: message` from the TypeScript compiler and MSVC style tools.
static PAREN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(.+?)\((\d+),(\d+)\): (error|warning)(?: \w+)?: (.+)$").expect("valid regex")
});

/// `  File "app.py", line 12, in main` from a Python traceback.
static PYTHON_FRAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*File "(.+?)", line (\d+)"#).expect("valid regex"));

/// The `ValueError: bad value` line that ends a Python traceback.
static PYTHON_ERROR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\w+(?:Error|Exception)\b.*)$").expect("valid regex"));

/// Returns the severity a tool's word stands for.
fn severity(word: Option<&str>) -> Severity {
    match word {
        Some("warning") => Severity::Warning,
        Some("note" | "info") => Severity::Info,
        _ => Severity::Error,
    }
}

/// Parses a 1 based number into a 0 based one.
fn index(text: &str) -> usize {
    text.parse::<usize>().unwrap_or(1).saturating_sub(1)
}

/// Finds the problems in `output` of a task that ran in `cwd`.
///
/// Only places in files that `exists` agrees are there count, which keeps timestamps and urls
/// from looking like file locations.
pub fn parse_problems(
    output: &str,
    cwd: &Path,
    exists: impl Fn(&Path) -> bool,
) -> Vec<TaskProblem> {
    let resolve = |text: &str| {
        let path = Path::new(text.trim());
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            cwd.join(path)
        };
        exists(&path).then_some(path)
    };
    let mut problems = Vec::new();
    let mut header: Option<(Severity, String)> = None;
    let mut frame: Option<(PathBuf, usize)> = None;
    for line in output.lines() {
        let line = line.trim_end();
        if let Some(found) = RUST_HEADER.captures(line) {
            header = Some((severity(Some(&found[1])), found[2].to_owned()));
            continue;
        }
        if let Some(found) = RUST_LOCATION.captures(line) {
            if let (Some((severity, message)), Some(path)) = (header.take(), resolve(&found[1])) {
                problems.push(TaskProblem {
                    path,
                    line: index(&found[2]),
                    column: index(&found[3]),
                    severity,
                    message,
                });
            }
            continue;
        }
        if let Some(found) = PYTHON_FRAME.captures(line) {
            frame = resolve(&found[1]).map(|path| (path, index(&found[2])));
            continue;
        }
        if let Some(found) = PYTHON_ERROR.captures(line)
            && let Some((path, at)) = frame.take()
        {
            problems.push(TaskProblem {
                path,
                line: at,
                column: 0,
                severity: Severity::Error,
                message: found[1].to_owned(),
            });
            continue;
        }
        if let Some(found) = PAREN.captures(line)
            && let Some(path) = resolve(&found[1])
        {
            problems.push(TaskProblem {
                path,
                line: index(&found[2]),
                column: index(&found[3]),
                severity: severity(Some(&found[4])),
                message: found[5].to_owned(),
            });
            continue;
        }
        if let Some(found) = COLON.captures(line)
            && let Some(path) = resolve(&found[1])
        {
            problems.push(TaskProblem {
                path,
                line: index(&found[2]),
                column: found.get(3).map_or(0, |col| index(col.as_str())),
                severity: severity(found.get(4).map(|word| word.as_str())),
                message: found[5].to_owned(),
            });
        }
    }
    problems
}

#[cfg(test)]
/// Tests for reading task output.
mod tests {
    use std::path::{Path, PathBuf};

    use super::parse_problems;
    use crate::diagnostic::Severity;

    /// Parses `output` as if every file under `/p` with an extension exists.
    fn parse(output: &str) -> Vec<(String, usize, usize, Severity, String)> {
        let exists = |path: &Path| path.starts_with("/p") && path.extension().is_some();
        parse_problems(output, Path::new("/p"), exists)
            .into_iter()
            .map(|problem| {
                let path = problem
                    .path
                    .strip_prefix("/p")
                    .map(PathBuf::from)
                    .unwrap_or(problem.path);
                let path = path.to_string_lossy().replace('\\', "/");
                (
                    path,
                    problem.line,
                    problem.column,
                    problem.severity,
                    problem.message,
                )
            })
            .collect()
    }

    /// Cargo errors take the header message and the arrow location.
    #[test]
    fn reads_rustc() {
        let output = "   Compiling app v0.1.0\nerror[E0308]: mismatched types\n --> src/main.rs:4:18\n  |\nwarning: unused variable: `x`\n  --> src/lib.rs:2:9\n";
        assert_eq!(
            parse(output),
            [
                (
                    "src/main.rs".into(),
                    3,
                    17,
                    Severity::Error,
                    "mismatched types".into()
                ),
                (
                    "src/lib.rs".into(),
                    1,
                    8,
                    Severity::Warning,
                    "unused variable: `x`".into()
                ),
            ]
        );
    }

    /// gcc, go and mypy style lines are read, with or without a column or a severity.
    #[test]
    fn reads_colon_style() {
        let output = "main.c:10:5: error: expected ';'\n./cmd/main.go:7:2: undefined: x\napp.py:3: note: here\n12:30:01 build started\n";
        assert_eq!(
            parse(output),
            [
                (
                    "main.c".into(),
                    9,
                    4,
                    Severity::Error,
                    "expected ';'".into()
                ),
                (
                    "cmd/main.go".into(),
                    6,
                    1,
                    Severity::Error,
                    "undefined: x".into()
                ),
                ("app.py".into(), 2, 0, Severity::Info, "here".into()),
            ]
        );
    }

    /// TypeScript errors use parentheses.
    #[test]
    fn reads_typescript() {
        let output = "src/app.ts(3,7): error TS2322: Type 'string' is not assignable.\n";
        assert_eq!(
            parse(output),
            [(
                "src/app.ts".into(),
                2,
                6,
                Severity::Error,
                "Type 'string' is not assignable.".into()
            )]
        );
    }

    /// A Python traceback points at its innermost frame with the exception.
    #[test]
    fn reads_python_tracebacks() {
        let output = "Traceback (most recent call last):\n  File \"app.py\", line 9, in <module>\n    main()\n  File \"lib/util.py\", line 3, in main\n    raise ValueError(\"bad\")\nValueError: bad\n";
        assert_eq!(
            parse(output),
            [(
                "lib/util.py".into(),
                2,
                0,
                Severity::Error,
                "ValueError: bad".into()
            )]
        );
    }

    /// Files that do not exist are not problems.
    #[test]
    fn skips_missing_files() {
        let found = parse_problems("nope.c:1:1: error: x\n", Path::new("/p"), |_| false);
        assert!(found.is_empty());
    }
}
