//! Stylesheets with every reference checked.
//!
//! The tokenizer is the one a browser engine uses, so escapes (`u\72l(...)`), comments and odd
//! spacing hide nothing: every `url()`, every string in `image-set()` and every `@import` goes
//! through the caller, who rewrites what points into the book and drops the rest. What survives is
//! written back token by token, so the result is CSS a browser reads the same way, minus comments.

use cssparser::{
    ParseError, Parser, ParserInput, ToCss, Token, serialize_identifier, serialize_string,
};

/// What a reference in a stylesheet is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlUse {
    /// `@import`: another stylesheet.
    Import,
    /// Anything else: an image, a font, a cursor.
    Resource,
}

/// What was taken out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CssReport {
    /// References dropped because they lead outside the book or nowhere.
    pub dropped_urls: usize,
    /// `@import` rules dropped.
    pub dropped_imports: usize,
    /// `expression()` and the like, from old Internet Explorer.
    pub dropped_expressions: usize,
}

/// Nesting deeper than this is not CSS anyone wrote by hand; its content is left out.
const MAX_NESTING: usize = 64;

struct Cx<'r> {
    rewrite: &'r mut dyn FnMut(&str, UrlUse) -> Option<String>,
    report: CssReport,
}

impl Cx<'_> {
    fn write_url(&mut self, url: &str, usage: UrlUse, out: &mut String) {
        match (self.rewrite)(url, usage) {
            Some(rewritten) => {
                out.push_str("url(");
                // A string serializes with the escapes it needs.
                let _ = serialize_string(&rewritten, out);
                out.push(')');
            }
            None => {
                self.report.dropped_urls += 1;
                out.push_str("none");
            }
        }
    }
}

type Nested<'i> = Result<(), ParseError<'i, ()>>;

/// The first string or URL in a block, which is a `url("...")` or `src("...")` argument.
fn first_string<'i>(parser: &mut Parser<'i, '_>) -> Result<Option<String>, ParseError<'i, ()>> {
    let mut found = None;
    while let Ok(token) = parser.next_including_whitespace_and_comments() {
        match token {
            Token::QuotedString(value) | Token::UnquotedUrl(value) if found.is_none() => {
                found = Some(value.to_string());
            }
            _ => {}
        }
    }
    Ok(found)
}

fn walk(
    parser: &mut Parser<'_, '_>,
    out: &mut String,
    cx: &mut Cx<'_>,
    depth: usize,
    strings_are_urls: bool,
) {
    let top_level = depth == 0;
    loop {
        let token = match parser.next_including_whitespace_and_comments() {
            Ok(token) => token.clone(),
            Err(_) => break,
        };
        match token {
            Token::AtKeyword(ref name) if top_level && name.eq_ignore_ascii_case("import") => {
                import(parser, out, cx);
            }
            Token::AtKeyword(ref name) if name.eq_ignore_ascii_case("charset") => {
                // The output is UTF-8 whatever the source said.
                skip_rule(parser);
            }
            Token::AtKeyword(ref name) if name.eq_ignore_ascii_case("namespace") => {
                // Its URL names a namespace and is never fetched, so it stays as it is.
                out.push_str("@namespace");
                copy_until_semicolon(parser, out);
            }
            Token::UnquotedUrl(ref url) => cx.write_url(url, UrlUse::Resource, out),
            Token::BadUrl(_) => {
                cx.report.dropped_urls += 1;
                out.push_str("none");
            }
            Token::QuotedString(ref value) if strings_are_urls => {
                cx.write_url(value, UrlUse::Resource, out);
            }
            Token::Function(ref name) => {
                let lower = name.to_ascii_lowercase();
                match lower.as_str() {
                    "url" | "src" => {
                        let url: Result<Option<String>, ParseError<'_, ()>> =
                            parser.parse_nested_block(|nested| first_string(nested));
                        match url {
                            Ok(Some(url)) => cx.write_url(&url, UrlUse::Resource, out),
                            _ => {
                                cx.report.dropped_urls += 1;
                                out.push_str("none");
                            }
                        }
                    }
                    "expression" | "-moz-binding" | "behavior" | "javascript" => {
                        let _: Nested<'_> = parser.parse_nested_block(|_| Ok(()));
                        cx.report.dropped_expressions += 1;
                        out.push_str("none");
                    }
                    _ => {
                        let _ = serialize_identifier(name, out);
                        out.push('(');
                        let urls = matches!(
                            lower.as_str(),
                            "image-set"
                                | "-webkit-image-set"
                                | "image"
                                | "cross-fade"
                                | "-webkit-cross-fade"
                        );
                        nested(parser, out, cx, depth, urls);
                        out.push(')');
                    }
                }
            }
            Token::ParenthesisBlock => {
                out.push('(');
                nested(parser, out, cx, depth, false);
                out.push(')');
            }
            Token::SquareBracketBlock => {
                out.push('[');
                nested(parser, out, cx, depth, false);
                out.push(']');
            }
            Token::CurlyBracketBlock => {
                out.push('{');
                nested(parser, out, cx, depth, false);
                out.push('}');
            }
            // An empty comment keeps two tokens apart without carrying anything along.
            Token::Comment(_) => out.push_str("/**/"),
            Token::BadString(_) => {}
            ref other => {
                let _ = other.to_css(out);
            }
        }
    }
}

fn nested(
    parser: &mut Parser<'_, '_>,
    out: &mut String,
    cx: &mut Cx<'_>,
    depth: usize,
    urls: bool,
) {
    let _: Nested<'_> = parser.parse_nested_block(|inner| {
        if depth < MAX_NESTING {
            walk(inner, out, cx, depth + 1, urls);
        }
        Ok(())
    });
}

fn skip_rule(parser: &mut Parser<'_, '_>) {
    while let Ok(token) = parser.next_including_whitespace_and_comments() {
        if matches!(token, Token::Semicolon) {
            break;
        }
        if matches!(token, Token::CurlyBracketBlock) {
            let _: Nested<'_> = parser.parse_nested_block(|_| Ok(()));
            break;
        }
    }
}

fn copy_until_semicolon(parser: &mut Parser<'_, '_>, out: &mut String) {
    while let Ok(token) = parser.next_including_whitespace_and_comments() {
        let token = token.clone();
        match token {
            Token::Semicolon => {
                out.push(';');
                break;
            }
            Token::Comment(_) => out.push_str("/**/"),
            Token::Function(_)
            | Token::ParenthesisBlock
            | Token::SquareBracketBlock
            | Token::CurlyBracketBlock => {
                // Nothing that belongs in an @namespace rule.
                let _: Nested<'_> = parser.parse_nested_block(|_| Ok(()));
                break;
            }
            other => {
                let _ = other.to_css(out);
            }
        }
    }
}

/// `@import`, rewritten to a stylesheet in the book or dropped whole.
fn import(parser: &mut Parser<'_, '_>, out: &mut String, cx: &mut Cx<'_>) {
    let mut url: Option<String> = None;
    let mut rest = String::new();
    while let Ok(token) = parser.next_including_whitespace_and_comments() {
        let token = token.clone();
        match token {
            Token::Semicolon => break,
            Token::QuotedString(ref value) | Token::UnquotedUrl(ref value) if url.is_none() => {
                url = Some(value.to_string());
            }
            Token::Function(ref name) if url.is_none() && name.eq_ignore_ascii_case("url") => {
                let found: Result<Option<String>, ParseError<'_, ()>> =
                    parser.parse_nested_block(|nested| first_string(nested));
                url = found.ok().flatten();
            }
            Token::Comment(_) => {}
            Token::WhiteSpace(_) if url.is_none() => {}
            Token::Function(_) | Token::ParenthesisBlock | Token::SquareBracketBlock => {
                // A media query with parentheses: copied through the same walk as everything else.
                let mut inner = String::new();
                if let Token::Function(ref name) = token {
                    let _ = serialize_identifier(name, &mut inner);
                }
                inner.push(if matches!(token, Token::SquareBracketBlock) {
                    '['
                } else {
                    '('
                });
                nested(parser, &mut inner, cx, MAX_NESTING - 1, false);
                inner.push(if matches!(token, Token::SquareBracketBlock) {
                    ']'
                } else {
                    ')'
                });
                rest.push_str(&inner);
            }
            Token::CurlyBracketBlock => {
                let _: Nested<'_> = parser.parse_nested_block(|_| Ok(()));
                break;
            }
            other => {
                let _ = other.to_css(&mut rest);
            }
        }
    }
    let rewritten = url
        .as_deref()
        .and_then(|url| (cx.rewrite)(url, UrlUse::Import));
    match rewritten {
        Some(rewritten) => {
            out.push_str("@import url(");
            let _ = serialize_string(&rewritten, out);
            out.push(')');
            out.push_str(&rest);
            out.push(';');
        }
        None => cx.report.dropped_imports += 1,
    }
}

fn run(css: &str, rewrite: &mut dyn FnMut(&str, UrlUse) -> Option<String>) -> (String, CssReport) {
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    let mut cx = Cx {
        rewrite,
        report: CssReport::default(),
    };
    let mut out = String::with_capacity(css.len());
    walk(&mut parser, &mut out, &mut cx, 0, false);
    (out, cx.report)
}

/// A whole stylesheet, from a file or a `<style>` element.
pub fn sanitize_stylesheet(
    css: &str,
    rewrite: &mut dyn FnMut(&str, UrlUse) -> Option<String>,
) -> (String, CssReport) {
    run(css, rewrite)
}

/// The declarations of a `style` attribute. `@import` means nothing there, so none is honored.
pub fn sanitize_declarations(
    css: &str,
    rewrite: &mut dyn FnMut(&str, UrlUse) -> Option<String>,
) -> (String, CssReport) {
    let mut only_resources = |url: &str, usage: UrlUse| match usage {
        UrlUse::Import => None,
        UrlUse::Resource => rewrite(url, usage),
    };
    // Wrapped in a block, so an @import at the start is not at the top level of a stylesheet.
    let (out, report) = run(&format!("{{{css}}}"), &mut only_resources);
    let inner = out
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or("")
        .to_owned();
    (inner, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keeps references to `ok/…` and rewrites them to `new/…`; drops everything else.
    fn rewrite(url: &str, _: UrlUse) -> Option<String> {
        url.strip_prefix("ok/").map(|rest| format!("new/{rest}"))
    }

    fn clean(css: &str) -> (String, CssReport) {
        sanitize_stylesheet(css, &mut rewrite)
    }

    #[test]
    fn every_way_of_writing_a_url_is_seen() {
        let (out, report) = clean(
            "a { background: url(http://evil/a.png); }\n\
             b { background: url( 'ok/b.png' ) no-repeat; }\n\
             c { background: u\\72l(http://evil/c.png); }\n\
             d { background: URL(\"//evil/d.png\"); }\n\
             e { background-image: image-set(\"http://evil/e.png\" 1x, 'ok/e.png' 2x); }\n\
             f { width: expression(alert(1)); }\n\
             @font-face { font-family: x; src: url(ok/f.woff) format('woff'), url(https://evil/f.woff); }",
        );
        assert!(!out.contains("evil"), "{out}");
        assert!(out.contains("url(\"new/b.png\")"), "{out}");
        assert!(out.contains("url(\"new/e.png\")"), "{out}");
        assert!(out.contains("url(\"new/f.woff\")"), "{out}");
        assert!(!out.to_ascii_lowercase().contains("expression"), "{out}");
        assert_eq!(report.dropped_urls, 5);
        assert_eq!(report.dropped_expressions, 1);
    }

    #[test]
    fn imports_into_the_book_stay_and_others_go() {
        let (out, report) = clean(
            "@charset \"iso-8859-1\";\n@import url(http://evil/x.css);\n@import 'ok/y.css' screen and (min-width: 10px);\n\
             @import \"evil.css\";\n@namespace svg url(http://www.w3.org/2000/svg);\np { color: red }",
        );
        assert!(!out.contains("charset"), "{out}");
        assert!(
            out.contains("@import url(\"new/y.css\") screen and (min-width: 10px);"),
            "{out}"
        );
        assert!(
            out.contains("@namespace svg url(http://www.w3.org/2000/svg);"),
            "{out}"
        );
        assert!(out.contains("p { color: red }"), "{out}");
        assert_eq!(report.dropped_imports, 2);
    }

    #[test]
    fn declarations_are_cleaned_and_imports_mean_nothing() {
        let (out, report) = sanitize_declarations(
            "color: red; background: url(javascript:alert(1)); /* c */ list-style: url(ok/i.png)",
            &mut rewrite,
        );
        // The bad URL ends at its first parenthesis, so the second is left over, as in a browser,
        // which then ignores that declaration.
        assert_eq!(
            out,
            "color: red; background: none); /**/ list-style: url(\"new/i.png\")"
        );
        assert_eq!(report.dropped_urls, 1);
    }

    #[test]
    fn deep_nesting_is_cut_and_does_not_overflow() {
        let css = format!("a {{ x: {}1{} }}", "(".repeat(100_000), ")".repeat(100_000));
        let (out, _) = clean(&css);
        assert!(out.len() < 1000);
    }

    #[test]
    fn the_same_input_gives_the_same_output() {
        let css = "a{color:red}/**/b > c::after { content: \"\\201C\" ; margin: -1.50em 0 }";
        assert_eq!(clean(css).0, clean(css).0);
        assert_eq!(
            clean(&clean(css).0).0,
            clean(css).0,
            "cleaning twice changes nothing"
        );
    }
}
