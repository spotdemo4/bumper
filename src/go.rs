//! Go module paths, which name the major version of modules at v2 and above.
//!
//! See <https://go.dev/ref/mod#major-version-suffixes>.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::iter::Peekable;
use std::ops::Range;
use std::path::Path;
use std::str::CharIndices;

type AppResult<T> = Result<T, String>;

/// Moves a Go module to the module path of a new major version.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModuleRename {
    pub old_path: String,
    pub new_path: String,
    /// The version the module releases, without a `v` prefix.
    pub version: String,
}

/// The major versions a Go module path allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMajor {
    /// No suffix, for v0 and v1.
    Unsuffixed,
    /// A `/vN` suffix, for vN where N is at least 2.
    Suffixed(u64),
    /// A `gopkg.in/name.vN` path, for vN only.
    GopkgIn(u64),
}

impl PathMajor {
    fn allows(self, major: u64) -> bool {
        match self {
            Self::Unsuffixed => major <= 1,
            Self::Suffixed(suffix) | Self::GopkgIn(suffix) => suffix == major,
        }
    }
}

/// A rewritten go.mod or .go file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewrite {
    pub content: String,
    /// The old module paths whose references changed.
    pub changed: BTreeSet<String>,
}

/// Splits `module_path` into its prefix and the major versions it allows, like Go's
/// `module.SplitPathVersion`.
///
/// # Errors
///
/// Fails when the path ends in a malformed major version suffix such as `/v1` or `/v02`.
pub fn split_path_major(module_path: &str) -> AppResult<(&str, PathMajor)> {
    let invalid = || format!("invalid major version suffix in Go module path '{module_path}'");

    if module_path.starts_with("gopkg.in/") {
        let (prefix, digits) = module_path.rsplit_once(".v").ok_or_else(invalid)?;
        if digits.is_empty()
            || !digits.chars().all(|character| character.is_ascii_digit())
            || (digits.starts_with('0') && digits != "0")
        {
            return Err(invalid());
        }
        let major = digits.parse().map_err(|_| invalid())?;
        return Ok((prefix, PathMajor::GopkgIn(major)));
    }

    let Some((prefix, digits)) = module_path.rsplit_once("/v").filter(|(_, digits)| {
        !digits.is_empty()
            && digits
                .chars()
                .all(|character| character.is_ascii_digit() || character == '.')
    }) else {
        return Ok((module_path, PathMajor::Unsuffixed));
    };
    if digits.contains('.') || digits.starts_with('0') || digits == "1" {
        return Err(invalid());
    }
    let major = digits.parse().map_err(|_| invalid())?;
    Ok((prefix, PathMajor::Suffixed(major)))
}

/// Returns the module path `module_path` must move to so the module can release `version`,
/// or `None` when it already allows that version.
///
/// A module in a major version subdirectory such as `v2` can't move, because its directory
/// names the major version too.
///
/// # Errors
///
/// Fails when the module path can't name the major version of `version`.
pub fn required_module_path(
    module_path: &str,
    version: &str,
    major_subdirectory: bool,
) -> AppResult<Option<String>> {
    let major = version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u64>().ok())
        .ok_or_else(|| format!("invalid version '{version}'"))?;
    let (prefix, path_major) = split_path_major(module_path)?;
    if path_major.allows(major) {
        return Ok(None);
    }

    match path_major {
        PathMajor::GopkgIn(suffix) | PathMajor::Suffixed(suffix) if major < suffix => Err(format!(
            "Go module path '{module_path}' requires major version {suffix}, but the release is {version}"
        )),
        PathMajor::GopkgIn(_) => Err(format!(
            "Go module path '{module_path}' can't release {version}; gopkg.in paths name their major version, so move the module to a .v{major} path first"
        )),
        _ if major_subdirectory => Err(format!(
            "Go module '{module_path}' is in a major version subdirectory, so it can't release {version}; copy it to a v{major} directory instead"
        )),
        _ => Ok(Some(format!("{prefix}/v{major}"))),
    }
}

/// Reads the module path declared by the go.mod file at `path`.
#[must_use]
pub fn read_module_path(path: &Path) -> Option<String> {
    module_path(&fs::read_to_string(path).ok()?)
}

/// Returns the module path declared by go.mod `source`, which must declare exactly one.
#[must_use]
pub fn module_path(source: &str) -> Option<String> {
    let directives = directives(source)?;
    let mut modules = directives
        .iter()
        .filter(|directive| directive.verb == "module");
    let module = modules.next()?;
    if modules.next().is_some() {
        return None;
    }
    match module.args.as_slice() {
        [path] => path.value().map(str::to_string),
        _ => None,
    }
}

/// Returns the directories `use`d by go.work `source`.
#[must_use]
pub fn workspace_uses(source: &str) -> Vec<String> {
    directives(source)
        .unwrap_or_default()
        .iter()
        .filter(|directive| directive.verb == "use")
        .filter_map(|directive| directive.args.first()?.value().map(str::to_string))
        .collect()
}

/// Returns the module paths go.mod `source` replaces with local directories, with those directories.
#[must_use]
pub fn local_replacements(source: &str) -> Vec<(String, String)> {
    directives(source)
        .unwrap_or_default()
        .iter()
        .filter(|directive| directive.verb == "replace")
        .filter_map(|directive| {
            let path = directive.args.first()?.value()?;
            let arrow = directive.args.iter().position(|token| token.is("=>"))?;
            let target = directive.args.get(arrow.checked_add(1)?)?.value()?;
            let local = target == "."
                || target == ".."
                || target.starts_with("./")
                || target.starts_with("../")
                || target.starts_with('/');
            local.then(|| (path.to_string(), target.to_string()))
        })
        .collect()
}

/// Rewrites the module paths `renames` moves in a go.mod or .go `file`. `module_paths` are the
/// paths of every module in the repository, so imports of nested modules stay unchanged.
///
/// # Errors
///
/// Fails when the file can't be read.
pub fn rewrite_file(
    file: &Path,
    renames: &[ModuleRename],
    module_paths: &[String],
) -> AppResult<Option<Rewrite>> {
    let source = fs::read_to_string(file)
        .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
    Ok(if file.file_name() == Some(OsStr::new("go.mod")) {
        rewrite_go_mod(&source, renames)
    } else {
        rewrite_imports(&source, renames, module_paths)
    })
}

/// Moves the `module`, `require` and `replace` directives of go.mod `source` to the new module
/// paths, requiring the new versions.
#[must_use]
pub fn rewrite_go_mod(source: &str, renames: &[ModuleRename]) -> Option<Rewrite> {
    let mut edits = Vec::new();
    let mut changed = BTreeSet::new();

    for directive in directives(source)? {
        let Some(path) = directive.args.first() else {
            continue;
        };
        let Some(rename) = path
            .value()
            .and_then(|value| renames.iter().find(|rename| rename.old_path == value))
        else {
            continue;
        };
        let version = format!("v{}", rename.version);
        match directive.verb {
            "module" => {}
            "require" => {
                let Some(required) = directive.args.get(1) else {
                    continue;
                };
                edits.push((required.range.clone(), version));
            }
            "replace" => {
                if let Some(replaced) = directive.args.get(1).filter(|token| !token.is("=>")) {
                    edits.push((replaced.range.clone(), version));
                }
            }
            _ => continue,
        }
        edits.push((path.range.clone(), path.replacement(&rename.new_path)));
        changed.insert(rename.old_path.clone());
    }

    apply_edits(source, edits, changed)
}

/// Moves the import declarations of Go `source` to the new module paths.
#[must_use]
pub fn rewrite_imports(
    source: &str,
    renames: &[ModuleRename],
    module_paths: &[String],
) -> Option<Rewrite> {
    let mut edits = Vec::new();
    let mut changed = BTreeSet::new();

    for import in import_paths(source) {
        // the most specific module provides the package, so nested modules keep their paths
        let owner = module_paths
            .iter()
            .chain(renames.iter().map(|rename| &rename.old_path))
            .filter(|module| is_within_module(&import.value, module))
            .max_by_key(|module| module.len());
        let Some(rename) =
            owner.and_then(|owner| renames.iter().find(|rename| rename.old_path == *owner))
        else {
            continue;
        };
        let Some(package) = import.value.strip_prefix(rename.old_path.as_str()) else {
            continue;
        };
        let quote = import.quote;
        edits.push((
            import.range,
            format!("{quote}{}{package}{quote}", rename.new_path),
        ));
        changed.insert(rename.old_path.clone());
    }

    apply_edits(source, edits, changed)
}

fn is_within_module(package: &str, module: &str) -> bool {
    package
        .strip_prefix(module)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn apply_edits(
    source: &str,
    mut edits: Vec<(Range<usize>, String)>,
    changed: BTreeSet<String>,
) -> Option<Rewrite> {
    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|(range, _)| range.start);

    let mut content = String::with_capacity(source.len());
    let mut position = 0;
    for (range, replacement) in edits {
        content.push_str(source.get(position..range.start)?);
        content.push_str(&replacement);
        position = range.end;
    }
    content.push_str(source.get(position..)?);
    (content != source).then_some(Rewrite { content, changed })
}

/// A go.mod or go.work directive, with blocks expanded into one directive per line.
struct Directive<'a> {
    verb: &'a str,
    args: Vec<Token<'a>>,
}

/// A go.mod or go.work token and its byte range in the file.
struct Token<'a> {
    range: Range<usize>,
    raw: &'a str,
}

impl Token<'_> {
    fn is(&self, raw: &str) -> bool {
        self.raw == raw
    }

    fn quote(&self) -> Option<char> {
        self.raw
            .chars()
            .next()
            .filter(|first| matches!(first, '"' | '`'))
    }

    /// The token without quotes, or `None` when it is an empty string.
    fn value(&self) -> Option<&str> {
        match self.quote() {
            Some(quote) => self
                .raw
                .strip_prefix(quote)?
                .strip_suffix(quote)
                .filter(|value| !value.is_empty()),
            None => Some(self.raw),
        }
    }

    fn replacement(&self, value: &str) -> String {
        match self.quote() {
            Some(quote) => format!("{quote}{value}{quote}"),
            None => value.to_string(),
        }
    }
}

/// Parses the directives of go.mod or go.work `source`, or `None` when it is malformed.
fn directives(source: &str) -> Option<Vec<Directive<'_>>> {
    let mut directives = Vec::new();
    let mut block = None;
    let mut offset = 0_usize;

    for line in source.split_inclusive('\n') {
        let start = offset;
        offset = offset.checked_add(line.len())?;
        let mut tokens = tokens(strip_line_comment(line), start)?.into_iter();
        let Some(first) = tokens.next() else {
            continue;
        };
        let rest = tokens.collect::<Vec<_>>();

        if let Some(verb) = block {
            if first.is(")") {
                if !rest.is_empty() {
                    return None;
                }
                block = None;
            } else {
                let mut args = vec![first];
                args.extend(rest);
                directives.push(Directive { verb, args });
            }
            continue;
        }

        if rest.first().is_some_and(|token| token.is("(")) {
            if rest.len() != 1 {
                return None;
            }
            block = Some(first.raw);
        } else {
            directives.push(Directive {
                verb: first.raw,
                args: rest,
            });
        }
    }

    block.is_none().then_some(directives)
}

/// Splits a go.mod line into tokens whose ranges start at `offset`, or `None` when a string
/// is unterminated.
fn tokens(code: &str, offset: usize) -> Option<Vec<Token<'_>>> {
    let mut tokens = Vec::new();
    let mut characters = code.char_indices().peekable();

    while let Some((start, character)) = characters.next() {
        if character.is_whitespace() {
            continue;
        }
        let end = match character {
            '(' | ')' => start.checked_add(1)?,
            '=' if characters.next_if(|(_, next)| *next == '>').is_some() => {
                start.checked_add(2)?
            }
            '"' | '`' => {
                let mut escaped = false;
                let mut end = None;
                for (index, next) in characters.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if character == '"' && next == '\\' {
                        escaped = true;
                    } else if next == character {
                        end = index.checked_add(1);
                        break;
                    }
                }
                end?
            }
            _ => {
                let mut end = code.len();
                while let Some(&(index, next)) = characters.peek() {
                    if next.is_whitespace()
                        || matches!(next, '(' | ')' | '"' | '`')
                        || code.get(index..).is_some_and(|rest| rest.starts_with("=>"))
                    {
                        end = index;
                        break;
                    }
                    characters.next();
                }
                end
            }
        };
        tokens.push(Token {
            range: offset.checked_add(start)?..offset.checked_add(end)?,
            raw: code.get(start..end)?,
        });
    }

    Some(tokens)
}

/// Removes a `//` comment from a go.mod line, ignoring `//` inside strings.
fn strip_line_comment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    let mut characters = line.char_indices().peekable();

    while let Some((index, character)) = characters.next() {
        if let Some(delimiter) = quote {
            if delimiter == '"' && escaped {
                escaped = false;
            } else if delimiter == '"' && character == '\\' {
                escaped = true;
            } else if character == delimiter {
                quote = None;
            }
        } else if matches!(character, '"' | '`') {
            quote = Some(character);
        } else if character == '/' && characters.peek().is_some_and(|(_, next)| *next == '/') {
            return line.get(..index).unwrap_or(line);
        }
    }

    line
}

/// An import path string in a Go file.
struct ImportPath {
    range: Range<usize>,
    value: String,
    quote: char,
}

enum GoToken<'a> {
    Word(&'a str),
    /// A string literal, with its import path when it has no escapes.
    String(Option<ImportPath>),
    Punctuation(char),
}

/// Reads the tokens at the start of a Go file, up to the end of its import declarations.
struct GoLexer<'a> {
    source: &'a str,
    characters: Peekable<CharIndices<'a>>,
}

impl<'a> GoLexer<'a> {
    /// The next token, skipping whitespace, comments and semicolons. `None` at the end of
    /// the file or at a malformed comment or string.
    fn next_token(&mut self) -> Option<GoToken<'a>> {
        loop {
            let (start, character) = self.characters.next()?;
            match character {
                ';' => {}
                '/' if self.characters.next_if(|(_, next)| *next == '/').is_some() => {
                    let _ = self.characters.find(|(_, next)| *next == '\n');
                }
                '/' if self.characters.next_if(|(_, next)| *next == '*').is_some() => {
                    let mut star = false;
                    loop {
                        let (_, next) = self.characters.next()?;
                        if star && next == '/' {
                            break;
                        }
                        star = next == '*';
                    }
                }
                '"' | '`' => return self.string(start, character),
                _ if character.is_whitespace() => {}
                _ if character.is_alphabetic() || character == '_' => {
                    let mut end = self.source.len();
                    while let Some(&(index, next)) = self.characters.peek() {
                        if !next.is_alphanumeric() && next != '_' {
                            end = index;
                            break;
                        }
                        self.characters.next();
                    }
                    return self.source.get(start..end).map(GoToken::Word);
                }
                _ => return Some(GoToken::Punctuation(character)),
            }
        }
    }

    fn string(&mut self, start: usize, quote: char) -> Option<GoToken<'a>> {
        let mut escaped = false;
        let mut has_escape = false;
        loop {
            let (index, next) = self.characters.next()?;
            if escaped {
                escaped = false;
            } else if quote == '"' && next == '\\' {
                escaped = true;
                has_escape = true;
            } else if quote == '"' && next == '\n' {
                return None;
            } else if next == quote {
                let value = self.source.get(start.checked_add(1)?..index)?;
                return Some(GoToken::String((!has_escape).then(|| ImportPath {
                    range: start..index.saturating_add(1),
                    value: value.to_string(),
                    quote,
                })));
            }
        }
    }
}

/// Returns the import paths of Go `source`, or none when its imports can't be parsed.
fn import_paths(source: &str) -> Vec<ImportPath> {
    let mut lexer = GoLexer {
        source,
        characters: source.char_indices().peekable(),
    };
    let mut paths = Vec::new();
    if !matches!(lexer.next_token(), Some(GoToken::Word("package")))
        || !matches!(lexer.next_token(), Some(GoToken::Word(_)))
    {
        return paths;
    }

    while let Some(GoToken::Word("import")) = lexer.next_token() {
        let complete = match lexer.next_token() {
            Some(GoToken::Punctuation('(')) => loop {
                match lexer.next_token() {
                    Some(GoToken::Punctuation(')')) => break true,
                    Some(token) => {
                        if !import_spec(token, &mut lexer, &mut paths) {
                            break false;
                        }
                    }
                    None => break false,
                }
            },
            Some(token) => import_spec(token, &mut lexer, &mut paths),
            None => false,
        };
        if !complete {
            return Vec::new();
        }
    }

    paths
}

/// Reads one import spec starting at `token`, such as `"fmt"` or `f "fmt"`.
fn import_spec(token: GoToken<'_>, lexer: &mut GoLexer<'_>, paths: &mut Vec<ImportPath>) -> bool {
    let token = match token {
        GoToken::Word(_) | GoToken::Punctuation('.') => lexer.next_token(),
        token => Some(token),
    };
    match token {
        Some(GoToken::String(path)) => {
            paths.extend(path);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rename(old_path: &str, new_path: &str, version: &str) -> ModuleRename {
        ModuleRename {
            old_path: old_path.to_string(),
            new_path: new_path.to_string(),
            version: version.to_string(),
        }
    }

    #[test]
    fn splits_major_version_suffixes_like_go() {
        for (path, prefix, major) in [
            ("example.com/lib", "example.com/lib", PathMajor::Unsuffixed),
            (
                "example.com/vendor",
                "example.com/vendor",
                PathMajor::Unsuffixed,
            ),
            ("example.com/v2x", "example.com/v2x", PathMajor::Unsuffixed),
            (
                "example.com/lib/v2",
                "example.com/lib",
                PathMajor::Suffixed(2),
            ),
            (
                "example.com/lib/v10",
                "example.com/lib",
                PathMajor::Suffixed(10),
            ),
            ("gopkg.in/yaml.v3", "gopkg.in/yaml", PathMajor::GopkgIn(3)),
            ("gopkg.in/check.v1", "gopkg.in/check", PathMajor::GopkgIn(1)),
        ] {
            assert_eq!(split_path_major(path), Ok((prefix, major)), "{path}");
        }
        for path in [
            "example.com/lib/v1",
            "example.com/lib/v0",
            "example.com/lib/v02",
            "example.com/lib/v2.1",
            "gopkg.in/yaml",
            "gopkg.in/yaml.v03",
        ] {
            assert!(split_path_major(path).is_err(), "{path}");
        }
    }

    #[test]
    fn module_paths_follow_the_release_major_version() {
        assert_eq!(
            required_module_path("example.com/lib", "1.9.0", false),
            Ok(None)
        );
        assert_eq!(
            required_module_path("example.com/lib", "0.3.0", false),
            Ok(None)
        );
        assert_eq!(
            required_module_path("example.com/lib", "2.0.0", false),
            Ok(Some("example.com/lib/v2".to_string()))
        );
        assert_eq!(
            required_module_path("example.com/lib/v2", "2.4.1", false),
            Ok(None)
        );
        assert_eq!(
            required_module_path("example.com/lib/v2", "3.0.0", false),
            Ok(Some("example.com/lib/v3".to_string()))
        );
        assert_eq!(
            required_module_path("gopkg.in/yaml.v3", "3.1.0", false),
            Ok(None)
        );

        let behind = required_module_path("example.com/lib/v3", "2.1.0", false).unwrap_err();
        assert!(behind.contains("requires major version 3"), "{behind}");
        let gopkg = required_module_path("gopkg.in/yaml.v3", "4.0.0", false).unwrap_err();
        assert!(gopkg.contains(".v4 path"), "{gopkg}");
        let subdirectory = required_module_path("example.com/lib/v2", "3.0.0", true).unwrap_err();
        assert!(subdirectory.contains("v3 directory"), "{subdirectory}");
    }

    #[test]
    fn reads_module_paths_from_go_mod() {
        assert_eq!(
            module_path("module example.com/lib // the library\n\ngo 1.24\n").as_deref(),
            Some("example.com/lib")
        );
        assert_eq!(
            module_path("module (\n\t\"example.com/quoted\"\n)\n").as_deref(),
            Some("example.com/quoted")
        );
        assert_eq!(module_path("module (\n)\n"), None);
        assert_eq!(module_path("module \"\"\n"), None);
        assert_eq!(module_path("module (\n\texample.com/lib\n"), None);
    }

    #[test]
    fn go_mod_rewrite_moves_module_requires_and_replaces() {
        let source = concat!(
            "module example.com/app // the app\n",
            "\n",
            "go 1.24\n",
            "\n",
            "require example.com/lib v1.4.0\n",
            "\n",
            "require (\n",
            "\texample.com/lib/extra v0.1.0 // indirect\n",
            "\t\"example.com/other\" v1.0.0\n",
            ")\n",
            "\n",
            "replace example.com/lib => ../lib\n",
            "replace example.com/other v1.0.0 => ../other\n",
            "exclude example.com/lib v1.3.0\n",
        );
        let renames = [
            rename("example.com/lib", "example.com/lib/v2", "2.0.0"),
            rename("example.com/other", "example.com/other/v3", "3.0.0"),
        ];

        let rewrite = rewrite_go_mod(source, &renames).expect("rewrite go.mod");

        assert_eq!(
            rewrite.content,
            concat!(
                "module example.com/app // the app\n",
                "\n",
                "go 1.24\n",
                "\n",
                "require example.com/lib/v2 v2.0.0\n",
                "\n",
                "require (\n",
                "\texample.com/lib/extra v0.1.0 // indirect\n",
                "\t\"example.com/other/v3\" v3.0.0\n",
                ")\n",
                "\n",
                "replace example.com/lib/v2 => ../lib\n",
                "replace example.com/other/v3 v3.0.0 => ../other\n",
                "exclude example.com/lib v1.3.0\n",
            )
        );
        assert_eq!(
            rewrite.changed,
            BTreeSet::from([
                "example.com/lib".to_string(),
                "example.com/other".to_string()
            ])
        );

        let own = rewrite_go_mod(
            "module example.com/lib\n\ngo 1.24\n",
            &[rename("example.com/lib", "example.com/lib/v2", "2.0.0")],
        )
        .expect("rewrite own module");
        assert_eq!(own.content, "module example.com/lib/v2\n\ngo 1.24\n");
        assert_eq!(
            rewrite_go_mod("module example.com/app\n", &renames),
            None,
            "unrelated modules stay unchanged"
        );
    }

    #[test]
    fn import_rewrite_only_moves_import_declarations() {
        let source = concat!(
            "//go:build linux\n",
            "\n",
            "// Package app imports \"example.com/lib\".\n",
            "package app\n",
            "\n",
            "/* import \"example.com/lib\" */\n",
            "import \"example.com/lib\"\n",
            "\n",
            "import (\n",
            "\t\"fmt\"; lib2 \"example.com/lib/sub\"\n",
            "\t. `example.com/lib/dot`\n",
            "\t_ \"example.com/lib/tools/cmd\"\n",
            "\t\"example.com/library\"\n",
            ")\n",
            "\n",
            "// #include <stdio.h>\n",
            "import \"C\"\n",
            "\n",
            "const path = \"example.com/lib\"\n",
            "\n",
            "func main() { fmt.Println(path, lib.Name, lib2.Name) }\n",
        );
        let renames = [rename("example.com/lib", "example.com/lib/v2", "2.0.0")];
        let modules = [
            "example.com/lib".to_string(),
            "example.com/lib/tools".to_string(),
        ];

        let rewrite = rewrite_imports(source, &renames, &modules).expect("rewrite imports");

        assert_eq!(
            rewrite.content,
            source
                .replace(
                    "import \"example.com/lib\"\n",
                    "import \"example.com/lib/v2\"\n"
                )
                .replace("\"example.com/lib/sub\"", "\"example.com/lib/v2/sub\"")
                .replace("`example.com/lib/dot`", "`example.com/lib/v2/dot`")
        );
        assert!(rewrite.content.contains("/* import \"example.com/lib\" */"));
        assert!(rewrite.content.contains("const path = \"example.com/lib\""));
        assert_eq!(
            rewrite.changed,
            BTreeSet::from(["example.com/lib".to_string()])
        );
    }

    #[test]
    fn import_rewrite_skips_files_without_matching_imports() {
        let renames = [rename("example.com/lib", "example.com/lib/v2", "2.0.0")];

        assert_eq!(
            rewrite_imports(
                "package app\n\nimport \"fmt\"\n",
                &renames,
                &["example.com/lib".to_string()]
            ),
            None
        );
        assert_eq!(
            rewrite_imports(
                "package app\n\nimport (\n\t\"example.com/lib\"\n",
                &renames,
                &["example.com/lib".to_string()]
            ),
            None,
            "unterminated import blocks are left alone"
        );
    }

    #[test]
    fn reads_workspace_uses_and_local_replacements() {
        assert_eq!(
            workspace_uses("go 1.24\n\nuse (\n\t.\n\t./app // app\n)\nuse ./lib\n"),
            vec![".".to_string(), "./app".to_string(), "./lib".to_string()]
        );
        assert_eq!(
            local_replacements(concat!(
                "replace example.com/lib => ../lib\n",
                "replace (\n",
                "\texample.com/remote => example.com/fork v1.0.0\n",
                "\texample.com/pinned v1.0.0 => ./pinned\n",
                ")\n",
            )),
            vec![
                ("example.com/lib".to_string(), "../lib".to_string()),
                ("example.com/pinned".to_string(), "./pinned".to_string()),
            ]
        );
    }
}
