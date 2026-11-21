//! the native lens: structural checks and syntax navigation, in this process (spec 9.1).
//!
//! nothing here starts a language server. the rules are ast-grep's own matcher over
//! tree-sitter grammars, and the symbol walk reads the same trees, so a check costs
//! milliseconds and works whether or not the project has an editor configured.
//!
//! three things live here:
//!
//! - the read-before-edit guard: a file the model has not read is not one it may edit.
//! - the structural scan: a project's ast-grep rules, run against a file it just wrote.
//! - the outline: the symbols a file declares, so the model reads a range instead of a file.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ast_grep_config::{GlobalRules, RuleConfig, SerializableRuleConfig};
use ast_grep_core::matcher::Matcher;
use ast_grep_core::meta_var::MetaVarEnv;
use ast_grep_core::Language;
use ast_grep_language::{LanguageExt, SupportLang};
use serde::Deserialize;

/// how serious a finding is. ordered, so a caller can report the worst one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Hint,
    Info,
    Warning,
    Error,
}

impl Severity {
    /// the word a report uses for this tier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Hint => "hint",
        }
    }

    fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("error") => Self::Error,
            Some("info") => Self::Info,
            Some("hint") => Self::Hint,
            _ => Self::Warning,
        }
    }
}

/// one structural finding against one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule: String,
    pub severity: Severity,
    pub message: String,
    /// 1-based, as every editor and compiler reports it.
    pub line: usize,
    pub column: usize,
    /// the matched source, on one line.
    pub snippet: String,
}

impl Finding {
    /// the one-line form the tool result carries, so the model can act on it without
    /// parsing anything.
    #[must_use]
    pub fn render(&self, path: &str) -> String {
        format!(
            "{path}:{}:{}: {}: {} [{}]",
            self.line,
            self.column,
            self.severity.as_str(),
            self.message,
            self.rule
        )
    }
}

/// a symbol a file declares, as the outline reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    /// 1-based, inclusive.
    pub start: usize,
    /// 1-based, inclusive.
    pub end: usize,
    /// how deep in the outline this sits: a method is one level inside its impl.
    pub depth: usize,
}

/// the kinds a grammar uses for a declaration, by the words they are built from. every
/// tree-sitter grammar names its own nodes, but a definition is recognisable by what its
/// kind says it is, and every one of them carries the name in a `name` field.
const DEFINITION_WORDS: [&str; 22] = [
    "class",
    "const",
    "constructor",
    "enum",
    "function",
    "impl",
    "interface",
    "macro",
    "method",
    "mod",
    "module",
    "namespace",
    "object",
    "protocol",
    "record",
    "static",
    "struct",
    "trait",
    "type",
    "union",
    "var",
    "variant",
];

/// the kinds that hold other declarations, so their members belong in the outline.
const CONTAINER_WORDS: [&str; 10] = [
    "class",
    "enum",
    "impl",
    "interface",
    "mod",
    "module",
    "namespace",
    "object",
    "struct",
    "trait",
];

fn kind_says(kind: &str, words: &[&str]) -> bool {
    words.iter().any(|word| kind.contains(word))
}

/// the read-before-edit guard: the files this session has actually read.
///
/// an edit to a file the model has not seen is an edit to text it is guessing at, which is
/// how a rewrite silently drops everything the file held. the guard is per session and
/// lives in the daemon, so it survives across the turns of one conversation.
#[derive(Debug, Default)]
pub struct ReadGuard {
    read: Mutex<HashSet<PathBuf>>,
}

impl ReadGuard {
    /// records that the model has read this file.
    pub fn record(&self, path: &Path) {
        let key = key(path);
        if let Ok(mut read) = self.read.lock() {
            read.insert(key);
        }
    }

    /// whether the model may edit this path without reading it first. a file that does not
    /// exist yet is not one it could have read, so creating it is always allowed.
    #[must_use]
    pub fn may_edit(&self, path: &Path) -> bool {
        if !path.exists() {
            return true;
        }
        self.read
            .lock()
            .map_or(true, |read| read.contains(&key(path)))
    }

    /// forgets everything, so a new session starts without inherited coverage.
    pub fn clear(&self) {
        if let Ok(mut read) = self.read.lock() {
            read.clear();
        }
    }
}

/// the path a guard entry is keyed by: the real file, so a symlink and its target are one.
fn key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// one session's lens.
///
/// it holds the guard that licenses an edit, and the rules of the workspace it works in.
/// the daemon keeps one per session, so the guard covers the whole conversation and the
/// rules are compiled once rather than per turn.
pub struct Lens {
    pub guard: ReadGuard,
    pub rules: Rules,
}

impl std::fmt::Debug for Lens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lens")
            .field("guard", &self.guard)
            .field("rules", &self.rules)
            .finish()
    }
}

impl Lens {
    /// the lens for a workspace: no file read yet, and that workspace's rules loaded.
    #[must_use]
    pub fn for_workspace(workspace: &Path) -> Self {
        Self {
            guard: ReadGuard::default(),
            rules: Rules::load(workspace),
        }
    }
}

/// what a rule says about itself, read from the same yaml the matcher is built from.
#[derive(Debug, Deserialize)]
struct RuleMeta {
    id: String,
    language: String,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    message: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    ignores: Vec<String>,
}

/// one compiled rule: what it reports, and the matcher that finds it.
struct Rule {
    id: String,
    severity: Severity,
    message: String,
    note: Option<String>,
    language: SupportLang,
    matcher: ast_grep_config::RuleCore,
    files: Option<globset::GlobSet>,
    ignores: Option<globset::GlobSet>,
}

impl Rule {
    /// whether this rule is meant for this path at all.
    ///
    /// the globs are matched against the path below the workspace, which is what a rule
    /// like `**/tests/**` describes. matching the bare file name would make every
    /// directory component invisible, and such a rule could never fire.
    fn applies(&self, relative: &str) -> bool {
        if let Some(ignores) = &self.ignores {
            if ignores.is_match(relative) {
                return false;
            }
        }
        self.files
            .as_ref()
            .is_none_or(|files| files.is_match(relative))
    }
}

/// every rule a workspace declares, compiled once.
#[derive(Default)]
pub struct Rules {
    /// the workspace the rule globs are resolved against.
    root: PathBuf,
    compiled: Vec<Rule>,
    /// what went wrong loading a rule file, so a broken rule is reported rather than
    /// silently skipped.
    problems: Vec<String>,
}

impl std::fmt::Debug for Rules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // a compiled matcher has no useful debug form; the rule ids are what identifies it.
        f.debug_struct("Rules")
            .field("root", &self.root)
            .field(
                "rules",
                &self
                    .compiled
                    .iter()
                    .map(|rule| &rule.id)
                    .collect::<Vec<_>>(),
            )
            .field("problems", &self.problems)
            .finish()
    }
}

impl Rules {
    /// loads the ast-grep project config a workspace carries (`sgconfig.yml` and the rule
    /// directories it names). a workspace with none has no rules, which is not an error.
    #[must_use]
    pub fn load(workspace: &Path) -> Self {
        let mut rules = Vec::new();
        let mut problems = Vec::new();
        for dir in rule_dirs(workspace, &mut problems) {
            for file in rule_files(&dir) {
                match load_rule_file(&file) {
                    Ok(mut loaded) => rules.append(&mut loaded),
                    Err(err) => problems.push(format!("{}: {err}", file.display())),
                }
            }
        }
        // a rule is reported by its id, so the order they load in must not depend on the
        // filesystem's.
        rules.sort_by(|left, right| left.id.cmp(&right.id));
        Self {
            root: workspace.to_path_buf(),
            compiled: rules,
            problems,
        }
    }

    /// whether this workspace has any rules to run.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.compiled.is_empty()
    }

    /// the rule files that could not be read or compiled.
    #[must_use]
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// every finding the rules make against this source, worst first.
    #[must_use]
    pub fn scan(&self, path: &Path, source: &str) -> Vec<Finding> {
        let Some(language) = SupportLang::from_path(path) else {
            return Vec::new();
        };
        let grep = language.ast_grep(source);
        let root = grep.root();
        let relative = path
            .strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        let mut findings = Vec::new();
        for rule in &self.compiled {
            if rule.language != language || !rule.applies(&relative) {
                continue;
            }
            for node in root.dfs() {
                let mut env = Cow::Owned(MetaVarEnv::new());
                let Some(matched) = rule.matcher.match_node_with_env(node, &mut env) else {
                    continue;
                };
                let position = matched.start_pos();
                findings.push(Finding {
                    rule: rule.id.clone(),
                    severity: rule.severity,
                    message: rule.note.as_ref().map_or_else(
                        || rule.message.clone(),
                        |note| format!("{} ({note})", rule.message),
                    ),
                    line: position.line() + 1,
                    column: position.column(&matched) + 1,
                    snippet: one_line(&matched.text()),
                });
            }
        }
        findings.sort_by(|left, right| {
            right
                .severity
                .cmp(&left.severity)
                .then(left.line.cmp(&right.line))
                .then(left.column.cmp(&right.column))
                .then(left.rule.cmp(&right.rule))
        });
        findings
    }

    /// the findings as the tool result renders them, or nothing when the file is clean.
    #[must_use]
    pub fn report(&self, path: &str, findings: &[Finding]) -> String {
        if findings.is_empty() {
            return String::new();
        }
        let worst = findings
            .iter()
            .map(|finding| finding.severity)
            .max()
            .unwrap_or(Severity::Warning);
        let mut out = format!(
            "\n\nlens: {} finding(s), worst {}",
            findings.len(),
            worst.as_str()
        );
        for finding in findings {
            out.push('\n');
            out.push_str(&finding.render(path));
        }
        out
    }
}

/// the ast-grep project config a workspace may carry.
#[derive(Debug, Deserialize)]
struct ProjectConfig {
    #[serde(default, rename = "ruleDirs")]
    rule_dirs: Vec<String>,
}

/// the rule directories a workspace's project config names.
fn rule_dirs(workspace: &Path, problems: &mut Vec<String>) -> Vec<PathBuf> {
    let config = workspace.join("sgconfig.yml");
    let Ok(body) = std::fs::read_to_string(&config) else {
        return Vec::new();
    };
    match serde_saphyr::from_str::<ProjectConfig>(&body) {
        Ok(parsed) => parsed
            .rule_dirs
            .into_iter()
            .map(|dir| workspace.join(dir))
            .collect(),
        Err(err) => {
            problems.push(format!("{}: {err}", config.display()));
            Vec::new()
        }
    }
}

/// the rule files in one directory, in a stable order.
fn rule_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension == "yml" || extension == "yaml")
        })
        .collect();
    files.sort();
    files
}

/// compiles every rule in one file. the metadata and the matcher come from the same yaml,
/// read twice because ast-grep keeps the matcher's own copy of it private.
fn load_rule_file(path: &Path) -> Result<Vec<Rule>, String> {
    let body = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    let meta: Vec<RuleMeta> = serde_saphyr::from_multiple(&body).map_err(|err| err.to_string())?;
    let configs: Vec<SerializableRuleConfig<SupportLang>> =
        serde_saphyr::from_multiple(&body).map_err(|err| err.to_string())?;
    if meta.len() != configs.len() {
        return Err(format!(
            "{} rule(s) but {} matcher(s)",
            meta.len(),
            configs.len()
        ));
    }
    let globals = GlobalRules::default();
    let mut rules = Vec::new();
    for (meta, config) in meta.into_iter().zip(configs) {
        let language = meta
            .language
            .parse::<SupportLang>()
            .map_err(|_| format!("unknown language: {}", meta.language))?;
        let compiled = RuleConfig::try_from(config, &globals).map_err(|err| err.to_string())?;
        rules.push(Rule {
            id: meta.id,
            severity: Severity::parse(meta.severity.as_deref()),
            message: meta.message,
            note: meta.note,
            language,
            matcher: compiled.matcher,
            files: glob_set(&meta.files)?,
            ignores: glob_set(&meta.ignores)?,
        });
    }
    Ok(rules)
}

fn glob_set(patterns: &[String]) -> Result<Option<globset::GlobSet>, String> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(globset::Glob::new(pattern).map_err(|err| err.to_string())?);
    }
    builder.build().map(Some).map_err(|err| err.to_string())
}

/// the matched source on one line, so a finding never breaks a report's shape.
fn one_line(text: &str) -> String {
    const MAX: usize = 120;
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= MAX {
        return joined;
    }
    let mut cut: String = joined.chars().take(MAX).collect();
    cut.push('…');
    cut
}

/// the symbols a file declares, in source order.
///
/// this is the syntax tree's own view: no index, no cross-file semantics, no server. a
/// declaration is a node whose kind names a definition and that carries its name in a
/// `name` field, which is the one shape tree-sitter grammars share.
#[must_use]
pub fn outline(path: &Path, source: &str) -> Vec<Symbol> {
    let Some(language) = SupportLang::from_path(path) else {
        return Vec::new();
    };
    let grep = language.ast_grep(source);
    let mut symbols = Vec::new();
    walk(&grep.root(), 0, &mut symbols);
    symbols
}

/// walks declarations, keeping the nesting that says a method belongs to its impl.
fn walk(
    node: &ast_grep_core::Node<'_, impl ast_grep_core::Doc>,
    depth: usize,
    out: &mut Vec<Symbol>,
) {
    for child in node.children() {
        let kind = child.kind().to_string();
        let declares = kind_says(&kind, &DEFINITION_WORDS);
        if declares {
            let container = kind_says(&kind, &CONTAINER_WORDS);
            // a rust `impl` names the type it implements in `type`, not `name`; a container
            // that names itself some other way still belongs in the outline.
            let named = child
                .field("name")
                .or_else(|| container.then(|| child.field("type")).flatten())
                .map(|name| name.text().to_string());
            if let Some(name) = named.filter(|name| !name.is_empty()) {
                out.push(Symbol {
                    name,
                    kind: kind.clone(),
                    start: child.start_pos().line() + 1,
                    end: child.end_pos().line() + 1,
                    depth,
                });
                // a container's members are part of the outline; a function's locals are
                // not, so only a container is walked into.
                if container {
                    walk(&child, depth + 1, out);
                }
                continue;
            }
        }
        walk(&child, depth, out);
    }
}

/// the language name a path is parsed as, for a report.
#[must_use]
pub fn language_of(path: &Path) -> Option<String> {
    SupportLang::from_path(path).map(|language| language.to_string())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn rules(body: &str) -> Rules {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("rules")).expect("rules dir");
        std::fs::write(dir.path().join("sgconfig.yml"), "ruleDirs:\n  - rules\n")
            .expect("sgconfig");
        std::fs::write(dir.path().join("rules/rule.yml"), body).expect("rule");
        Rules::load(dir.path())
    }

    #[test]
    fn a_rule_fires_on_the_node_it_describes() {
        let rules = rules(
            "id: no-unwrap\nlanguage: rust\nseverity: error\nmessage: unwrap panics\nrule:\n  pattern: $X.unwrap()\n",
        );
        assert!(rules.problems().is_empty(), "{:?}", rules.problems());

        let findings = rules.scan(Path::new("a.rs"), "fn a() {\n    let x = y.unwrap();\n}\n");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule, "no-unwrap");
        assert_eq!(findings[0].severity, Severity::Error);
        assert_eq!(findings[0].line, 2);
        assert_eq!(findings[0].message, "unwrap panics");
        assert_eq!(findings[0].snippet, "y.unwrap()");
    }

    #[test]
    fn a_rule_only_runs_on_its_own_language() {
        let rules = rules(
            "id: no-unwrap\nlanguage: rust\nseverity: error\nmessage: unwrap\nrule:\n  pattern: $X.unwrap()\n",
        );
        assert!(
            rules.scan(Path::new("a.py"), "x = y.unwrap()\n").is_empty(),
            "a python file is not a rust rule's business"
        );
        assert_eq!(rules.scan(Path::new("a.rs"), "y.unwrap();\n").len(), 1);
    }

    #[test]
    fn an_ignored_path_is_skipped() {
        let rules = rules(
            "id: no-unwrap\nlanguage: rust\nseverity: warning\nmessage: unwrap\nignores:\n  - \"**/tests/**\"\nrule:\n  pattern: $X.unwrap()\n",
        );
        // the glob is matched against the file name, which is what ast-grep's own project
        // config resolves against for a single file.
        assert_eq!(
            rules.scan(Path::new("src/lib.rs"), "y.unwrap();\n").len(),
            1
        );
    }

    #[test]
    fn findings_are_ordered_worst_first() {
        let rules = rules(
            "id: two\nlanguage: rust\nseverity: warning\nmessage: w\nrule:\n  pattern: $X.unwrap()\n",
        );
        let findings = rules.scan(Path::new("a.rs"), "a.unwrap();\nb.unwrap();\n");
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].line, 1, "source order within a tier");
    }

    #[test]
    fn a_clean_file_reports_nothing() {
        let rules = rules(
            "id: no-unwrap\nlanguage: rust\nseverity: error\nmessage: unwrap\nrule:\n  pattern: $X.unwrap()\n",
        );
        assert!(
            rules.scan(Path::new("a.rs"), "fn a() {}\n").is_empty(),
            "a clean file has no findings"
        );
        assert_eq!(rules.report("a.rs", &[]), "");
    }

    #[test]
    fn a_broken_rule_is_reported_rather_than_dropped() {
        // a rule file that is not a rule at all must say so, not vanish.
        let bad = rules("this: is: not: a rule\n");
        assert!(bad.is_empty());
        assert!(!bad.problems().is_empty(), "the problem must be reported");
        assert!(
            bad.problems()[0].contains("rule.yml"),
            "the report names the file: {:?}",
            bad.problems()
        );
    }

    #[test]
    fn a_workspace_without_a_config_has_no_rules() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rules = Rules::load(dir.path());
        assert!(rules.is_empty());
        assert!(rules.problems().is_empty(), "no config is not a problem");
    }

    #[test]
    fn the_outline_lists_the_declarations_a_file_makes() {
        let source = "fn one() {}\nstruct Two { a: u8 }\nimpl Two {\n    fn three(&self) {}\n}\n";
        let symbols = outline(Path::new("a.rs"), source);
        let names: Vec<&str> = symbols.iter().map(|symbol| symbol.name.as_str()).collect();
        assert_eq!(names, ["one", "Two", "Two", "three"], "{symbols:?}");
        assert_eq!(symbols[0].start, 1);
        assert_eq!(symbols[0].end, 1);
        assert_eq!(
            symbols[3].depth, 1,
            "a method sits inside its impl: {symbols:?}"
        );
    }

    #[test]
    fn a_function_body_is_not_part_of_the_outline() {
        let source = "fn outer() {\n    let inner = 1;\n}\n";
        let symbols = outline(Path::new("a.rs"), source);
        assert_eq!(
            symbols
                .iter()
                .map(|symbol| symbol.name.as_str())
                .collect::<Vec<_>>(),
            ["outer"],
            "a local binding is not a declaration: {symbols:?}"
        );
    }

    #[test]
    fn an_unknown_language_has_no_outline_and_no_findings() {
        let path = Path::new("notes.unknownext");
        assert!(
            outline(path, "anything at all").is_empty(),
            "an unparsed file has no outline"
        );
    }

    #[test]
    fn the_guard_refuses_an_edit_to_a_file_that_was_never_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "one").expect("write");

        let guard = ReadGuard::default();
        assert!(!guard.may_edit(&file), "an unread file may not be edited");

        guard.record(&file);
        assert!(guard.may_edit(&file), "a read file may be edited");

        // a file that does not exist yet could not have been read, so creating it is fine.
        assert!(guard.may_edit(&dir.path().join("new.rs")));

        guard.clear();
        assert!(!guard.may_edit(&file));
    }
}
