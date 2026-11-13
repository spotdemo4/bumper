//! Erlang/OTP application resource files (`src/<app>.app.src`).

use std::ops::Range;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token<'a> {
    Punct(u8),
    Atom(&'a str),
    Str(Range<usize>),
    Other,
}

/// Returns whether `name` is the file name of an application resource file.
#[must_use]
pub fn is_app_src_name(name: &str) -> bool {
    name.strip_suffix(".app.src")
        .is_some_and(|app| !app.is_empty())
}

/// Returns whether `path` is an application resource file in an OTP
/// application's `src` directory, so its package is the directory above it.
#[must_use]
pub fn is_app_src_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_app_src_name)
        && path.parent().and_then(Path::file_name) == Some("src".as_ref())
}

/// Returns the byte range of the `vsn` string in an application resource file.
///
/// Only a string `vsn` in the top-level property list of
/// `{application, Name, [...]}.` counts, so values such as rebar3's
/// `{vsn, git}` and `vsn` entries nested in other properties are ignored.
#[must_use]
pub fn app_src_version_range(source: &str) -> Option<Range<usize>> {
    let tokens = tokenize(source)?;
    let [
        Token::Punct(b'{'),
        Token::Atom("application"),
        Token::Punct(b','),
        Token::Atom(name),
        Token::Punct(b','),
        Token::Punct(b'['),
        properties @ ..,
    ] = tokens.as_slice()
    else {
        return None;
    };
    if name.is_empty() {
        return None;
    }

    let mut rest = properties;
    loop {
        if let [
            Token::Punct(b'{'),
            Token::Atom("vsn"),
            Token::Punct(b','),
            Token::Str(range),
            Token::Punct(b'}'),
            ..,
        ] = rest
        {
            return Some(range.clone());
        }

        // skip to the next property of the list
        let mut depth = 0_usize;
        loop {
            let (token, remaining) = rest.split_first()?;
            rest = remaining;
            match token {
                Token::Punct(b'{' | b'[' | b'(') => depth = depth.checked_add(1)?,
                Token::Punct(b'}' | b']' | b')') if depth == 0 => return None,
                Token::Punct(b'}' | b']' | b')') => depth = depth.checked_sub(1)?,
                Token::Punct(b',') if depth == 0 => break,
                _ => {}
            }
        }
    }
}

/// Splits Erlang source into the tokens needed to find the version, skipping
/// whitespace and `%` comments. Returns `None` for unterminated quotes.
fn tokenize(source: &str) -> Option<Vec<Token<'_>>> {
    let bytes = source.as_bytes();
    let is_word =
        |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'@') || byte >= 0x80;
    let mut tokens = Vec::new();
    let mut index = 0;

    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'%' => {
                while bytes.get(index).is_some_and(|&byte| byte != b'\n') {
                    index = index.checked_add(1)?;
                }
            }
            b'"' | b'\'' => {
                let start = index.checked_add(1)?;
                index = start;
                loop {
                    match *bytes.get(index)? {
                        b'\\' => index = index.checked_add(2)?,
                        quote if quote == byte => break,
                        _ => index = index.checked_add(1)?,
                    }
                }
                let content = start..index;
                index = index.checked_add(1)?;
                tokens.push(if byte == b'"' {
                    Token::Str(content)
                } else {
                    Token::Atom(source.get(content)?)
                });
            }
            // character literals such as `$"` or `$\n`
            b'$' => {
                index = index.checked_add(1)?;
                if bytes.get(index) == Some(&b'\\') {
                    index = index.checked_add(1)?;
                }
                index = index.checked_add(1)?;
                tokens.push(Token::Other);
            }
            byte if byte.is_ascii_whitespace() => index = index.checked_add(1)?,
            byte if is_word(byte) => {
                let start = index;
                while bytes.get(index).copied().is_some_and(is_word) {
                    index = index.checked_add(1)?;
                }
                tokens.push(if byte.is_ascii_lowercase() {
                    Token::Atom(source.get(start..index)?)
                } else {
                    Token::Other
                });
            }
            byte => {
                tokens.push(Token::Punct(byte));
                index = index.checked_add(1)?;
            }
        }
    }

    Some(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(source: &str) -> Option<&str> {
        app_src_version_range(source).and_then(|range| source.get(range))
    }

    #[test]
    fn finds_the_application_version() {
        let source = r#"%% -*- erlang -*-
{application, my_app,
 [{description, "An OTP application, {vsn, \"0.0.0\"}"},
  {registered, []},
  {applications, [kernel, stdlib]},
  {env, [{vsn, "9.9.9"}]},
  {vsn, "1.11.1"}, % {vsn, "2.0.0"}
  {licenses, ["Apache-2.0"]}
 ]}.
"#;
        assert_eq!(version(source), Some("1.11.1"));
    }

    #[test]
    fn accepts_quoted_application_names_and_character_literals() {
        let source = "{application, 'my-app', [{sep, $,}, {quote, $\"}, {vsn, \"0.1.0\"}]}.";
        assert_eq!(version(source), Some("0.1.0"));
    }

    #[test]
    fn ignores_non_string_nested_and_commented_versions() {
        for source in [
            "{application, my_app, [{vsn, git}]}.",
            "{application, my_app, [{env, [{vsn, \"1.0.0\"}]}]}.",
            "{application, my_app, [\n% {vsn, \"1.0.0\"}\n]}.",
            "{application, '', [{vsn, \"1.0.0\"}]}.",
            "{library, my_app, [{vsn, \"1.0.0\"}]}.",
            "{application, my_app, [{vsn, \"1.0.0}]}.",
        ] {
            assert_eq!(version(source), None, "{source}");
        }
    }

    #[test]
    fn recognizes_resource_files_in_src_directories() {
        assert!(is_app_src_path(Path::new("apps/my_app/src/my_app.app.src")));
        assert!(!is_app_src_path(Path::new("apps/my_app/my_app.app.src")));
        assert!(!is_app_src_path(Path::new("src/.app.src")));
        assert!(!is_app_src_path(Path::new("src/my_app.app")));
    }
}
