//! Zero-dependency derive macros for `binary`.
//!
//! Scope (phase 1): structs and enums that carry at most **one lifetime
//! parameter**, in four flavours:
//!
//! - **named object** (default): `#[binary(rename = "...")]`, `#[binary(optional)]`,
//!   `#[binary(skip)]`.
//! - **tuple object** (`#[binary(tuple)]`): members in declaration order, no
//!   names, no `rename` / `optional` / `skip`. Wire-identical to an N-tuple of
//!   the same members.
//! - **C-like enum** (`#[binary(int)]`): discriminant written as an integer.
//! - **tagged union** (`#[binary(union)]`): `tag + auto value`, tag is a compact
//!   integer.
//!
//! Two member-level features cut across the flavours (DESIGN §19.17):
//!
//! - `#[binary(with = "module")]` swaps the member's converter for
//!   `module::Repr`, converting with `module::to` / `module::from`. The wire is
//!   `Repr`'s wire, so a field whose type comes from another crate — where the
//!   orphan rule forbids `Encode` — still fits.
//! - a lifetime parameter makes the item borrow from the input: a member typed
//!   `&'de str` / `&'de [u8]` decodes without copying, because the impl is
//!   `impl<'de> Decode<'de> for Name<'de>`. There is no `#[binary(borrow)]`
//!   attribute — the member type already says it.
//!
//! Everything is parsed from the compiler-provided `proc_macro::TokenStream` — no
//! `syn` / `quote` dependency.
//!
//! `#[derive(Encode)]` emits the `Layout` + `Encode` impls; `#[derive(Decode)]`
//! emits the `Decode` impl. Deriving both in one `#[derive(...)]` is supported;
//! deriving `Decode` alone requires an existing `Layout` impl (normally from
//! `Encode`), because `Layout` is a shared supertrait and two macros cannot both
//! emit the same impl.

use proc_macro::{Delimiter, Group, Literal, Span, TokenStream, TokenTree};

/// A macro error plus the span it should be reported at.
///
/// Tokens keep their `Span` so a diagnostic points at the field that carried the
/// bad attribute, not at `#[derive(...)]`.
#[derive(Clone)]
struct Error {
    msg: String,
    /// `Span::call_site()` for errors about the item as a whole.
    span: Span,
}

impl Error {
    fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            span: Span::call_site(),
        }
    }

    /// Attach the span of the token the error is about (a field name, a
    /// `#[binary(...)]` option, a literal).
    fn at(msg: impl Into<String>, span: Span) -> Self {
        Self {
            msg: msg.into(),
            span,
        }
    }
}

type Res<T> = Result<T, Error>;

/// Lets `?` lift a plain message (from the low-level literal scanner, which has
/// no token spans of its own) into an [`Error`] at the call site.
impl From<String> for Error {
    fn from(msg: String) -> Self {
        Error::new(msg)
    }
}

#[derive(Clone)]
enum Tok {
    Ident(String, Span),
    Punct(char, Span),
    Literal(String, Span),
    Group(Delimiter, Vec<Tok>, Span),
}

impl Tok {
    fn span(&self) -> Span {
        match self {
            Tok::Ident(_, s) | Tok::Literal(_, s) | Tok::Group(_, _, s) => *s,
            Tok::Punct(_, s) => *s,
        }
    }
}

fn flatten(ts: TokenStream) -> Vec<Tok> {
    ts.into_iter()
        .map(|t| {
            let span = t.span();
            match t {
                TokenTree::Ident(i) => Tok::Ident(i.to_string(), span),
                TokenTree::Punct(p) => Tok::Punct(p.as_char(), span),
                TokenTree::Literal(l) => Tok::Literal(l.to_string(), span),
                TokenTree::Group(g) => Tok::Group(g.delimiter(), flatten(g.stream()), span),
            }
        })
        .collect()
}

/// Render tokens back to source text. Used for field types, which are copied
/// verbatim into the generated code.
///
/// A space is inserted only between two word-like tokens (`Ident` / `Literal`),
/// which keeps `*const u8`, `dyn Fn(i32) -> i32` and `impl Trait` valid while
/// `Vec<T>`, `&'a str` and `[u8; 4]` still come out without spaces.
fn render(toks: &[Tok]) -> String {
    let mut out = String::new();
    let mut prev_word = false;
    for t in toks {
        let word = matches!(t, Tok::Ident(..) | Tok::Literal(..));
        if word && prev_word {
            out.push(' ');
        }
        prev_word = word;
        match t {
            Tok::Ident(s, _) | Tok::Literal(s, _) => out.push_str(s),
            Tok::Punct(c, _) => out.push(*c),
            Tok::Group(d, inner, _) => {
                let (open, close) = match d {
                    Delimiter::Parenthesis => ('(', ')'),
                    Delimiter::Brace => ('{', '}'),
                    Delimiter::Bracket => ('[', ']'),
                    Delimiter::None => ('\0', '\0'),
                };
                if open != '\0' {
                    out.push(open);
                }
                out.push_str(&render(inner));
                if close != '\0' {
                    out.push(close);
                }
            }
        }
    }
    out
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn new(toks: Vec<Tok>) -> Self {
        Self { toks, pos: 0 }
    }

    fn eof(&self) -> bool {
        self.pos >= self.toks.len()
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn peek_ident(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(i, _)) if i.as_str() == s)
    }

    fn peek_punct(&self, c: char) -> bool {
        matches!(self.peek(), Some(Tok::Punct(p, _)) if *p == c)
    }

    fn peek_group(&self, d: Delimiter) -> bool {
        matches!(self.peek(), Some(Tok::Group(g, _, _)) if *g == d)
    }

    fn bump(&mut self) -> Tok {
        let t = self
            .toks
            .get(self.pos)
            .cloned()
            .unwrap_or(Tok::Punct('\0', Span::call_site()));
        self.pos += 1;
        t
    }

    fn ident(&mut self, what: &str) -> Res<String> {
        match self.bump() {
            Tok::Ident(i, _) => Ok(i),
            _ => Err(Error::new(format!("expected {} identifier", what))),
        }
    }

    /// Span of the most recently consumed token, for `Error::at`.
    fn last_span(&self) -> Span {
        if self.pos == 0 {
            Span::call_site()
        } else {
            self.toks[self.pos - 1].span()
        }
    }

    fn expect_ident(&mut self, s: &str) -> Res<()> {
        if self.peek_ident(s) {
            self.pos += 1;
            Ok(())
        } else {
            Err(Error::new(format!("expected `{}`", s)))
        }
    }

    fn expect_punct(&mut self, c: char) -> Res<()> {
        if self.peek_punct(c) {
            self.pos += 1;
            Ok(())
        } else {
            Err(Error::new(format!("expected `{}`", c)))
        }
    }

    fn group(&mut self, d: Delimiter) -> Res<Vec<Tok>> {
        match self.bump() {
            Tok::Group(g, inner, _) if g == d => Ok(inner),
            _ => Err(Error::new("expected a delimited group".to_string())),
        }
    }

    /// Consume a field type and return its source text: stop at the first comma
    /// outside `<...>`.
    fn take_type(&mut self) -> String {
        let start = self.pos;
        let mut depth = 0i32;
        while !self.eof() {
            match self.peek() {
                Some(Tok::Punct('<', _)) => {
                    depth += 1;
                    self.pos += 1;
                }
                Some(Tok::Punct('>', _)) => {
                    depth -= 1;
                    self.pos += 1;
                }
                Some(Tok::Punct(',', _)) if depth == 0 => break,
                _ => self.pos += 1,
            }
        }
        render(&self.toks[start..self.pos])
    }

    fn skip_visibility(&mut self) {
        if self.peek_ident("pub") {
            self.pos += 1;
            if self.peek_group(Delimiter::Parenthesis) {
                self.pos += 1;
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Required,
    Optional,
    Skip,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Named,
    Tuple,
    /// `#[binary(int)]`: only valid on enums, encodes the discriminant.
    Int,
    /// `#[binary(union)]`: only valid on enums, encodes `tag + auto value`.
    Union,
}

struct Field {
    name: String,
    ty: String,
    /// Raw member name bytes (UTF-8 of `rename`, or the field name). Unused in
    /// tuple mode.
    wire: Vec<u8>,
    /// True when `#[binary(rename = ...)]` was written, even if it repeats the
    /// field name — `#[binary(tuple)]` rejects it either way.
    renamed: bool,
    /// Module path from `#[binary(with = "...")]`, whose `Repr` / `to` / `from`
    /// replace the field's own `Encode` / `Decode`.
    with: Option<String>,
    /// Span of the field name, used for field-level diagnostics.
    span: Span,
    kind: Kind,
}

impl Field {
    /// `b"..."` literal used in generated `match` arms and `write_named_member`.
    fn wire_literal(&self) -> String {
        Literal::byte_string(&self.wire).to_string()
    }

    /// Type that actually occupies the wire for this member.
    ///
    /// `#[binary(with)]` replaces the field's own layout with its converter's,
    /// so `Layout::LEN` and the tuple length sum must be taken from `Repr` — the
    /// field type need not implement `Layout` at all (that is the point: it may
    /// come from another crate).
    fn wire_ty(&self) -> String {
        match &self.with {
            Some(m) => format!("{m}::Repr"),
            None => self.ty.clone(),
        }
    }

    /// Borrowed expression holding the value that goes on the wire.
    ///
    /// `&self.f` normally; `&m::to(&self.f)` when a converter is attached. The
    /// temporary `Repr` lives until the end of the enclosing statement, which is
    /// exactly as long as the encode call needs it.
    fn wire_value(&self) -> String {
        match &self.with {
            Some(m) => format!("&{m}::to(&self.{})", self.name),
            None => format!("&self.{}", self.name),
        }
    }
}

/// Render wire bytes for diagnostics: `"id"` for text, `b"\xff"` otherwise.
fn display_wire(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => format!("{:?}", s),
        Err(_) => format!("{:?}", bytes),
    }
}

/// Declared generics of the target item: **at most one lifetime parameter**.
///
/// The decode impl must bind that lifetime to `Decode<'de>`, which is
/// expressible only while it is the sole parameter. Type and const parameters
/// are rejected: the `Decode<'de>` relationship cannot be inferred
/// (DESIGN §19.17).
#[derive(Default)]
struct Generics {
    /// `"<'a>"` including the angle brackets, or empty.
    decl: Option<String>,
}

impl Generics {
    /// `impl<'a>` / `impl` prefix.
    fn impl_prefix(&self) -> String {
        match &self.decl {
            Some(d) => format!("impl{d}"),
            None => "impl".to_string(),
        }
    }

    /// `Name<'a>` / `Name` suffix.
    fn self_ty(&self, name: &str) -> String {
        match &self.decl {
            Some(d) => format!("{name}{d}"),
            None => name.to_string(),
        }
    }

    /// Header of the `Decode` impl.
    ///
    /// Without a declared lifetime the decode lifetime is the anonymous `'de`;
    /// with one, the declared name *is* the decode lifetime, so
    /// `impl<'a> Decode<'a> for Name<'a>` — decoding borrows from the input for
    /// exactly as long as the value lives.
    fn decode_impl(&self, name: &str) -> String {
        match &self.decl {
            Some(d) => format!("impl{d} ::binary::Decode{d} for {}{d}", name),
            None => format!("impl<'de> ::binary::Decode<'de> for {name}"),
        }
    }

    /// The lifetime the generated `Decoder` / `&'de [u8]` types use.
    fn decode_lt(&self) -> &str {
        match &self.decl {
            Some(d) => &d[1..d.len() - 1],
            None => "'de",
        }
    }
}

/// Parse `<'a>` (or nothing) after the item name.
///
/// Lifetimes arrive from `proc_macro` as `Punct('\'')` + `Ident`, so `'static`
/// and bounds such as `'a: 'b` are visible here and rejected with a message that
/// names the real limitation instead of "generics are unsupported".
fn parse_generics(p: &mut Parser, what: &str) -> Res<Generics> {
    if !p.peek_punct('<') {
        return Ok(Generics::default());
    }
    p.pos += 1;
    let mut decl: Option<String> = None;
    loop {
        match p.peek() {
            Some(Tok::Punct('\'', _)) => {
                let span = p.peek().map(Tok::span).unwrap_or_else(Span::call_site);
                p.pos += 1;
                let name = p.ident("lifetime name")?;
                if name == "static" {
                    return Err(Error::at(
                        format!("`{what}` cannot declare `'static` as a parameter"),
                        span,
                    ));
                }
                if decl.is_some() {
                    return Err(Error::at(
                        format!("`{what}` takes at most one lifetime parameter"),
                        span,
                    ));
                }
                decl = Some(format!("'{}", name));
            }
            Some(Tok::Punct(',', _)) => p.pos += 1,
            Some(Tok::Punct('>', _)) => {
                p.pos += 1;
                break;
            }
            Some(t) => {
                let span = t.span();
                return Err(Error::at(
                    format!(
                        "`{what}` supports at most one lifetime parameter and no type or const \
                         parameters; write the borrowed form (`&'de str`) or a concrete type"
                    ),
                    span,
                ));
            }
            None => {
                return Err(Error::new(format!(
                    "unterminated parameter list on `{what}`"
                )))
            }
        }
    }
    Ok(Generics {
        decl: decl.map(|d| format!("<{d}>")),
    })
}

struct Info {
    name: String,
    mode: Mode,
    fields: Vec<Field>,
    generics: Generics,
}

/// A C-like enum encoded as its underlying integer.
struct EnumInfo {
    name: String,
    /// Rust type the discriminant is written as, e.g. `i32` or `u8`.
    repr: String,
    /// `size_of::<repr>()`, used for `Layout::LEN`.
    width: usize,
    /// `(variant, discriminant)`, in declaration order.
    variants: Vec<(String, i128)>,
    generics: Generics,
}

/// Shape of one `#[binary(union)]` variant. Decides how the generated code
/// builds patterns and constructor expressions, not how bytes are laid out.
#[derive(Clone, Copy, PartialEq, Eq)]
enum VariantShape {
    /// `Name` — tag only.
    Unit,
    /// `Name(A, B)` — positional fields.
    Tuple,
    /// `Name { a: A, b: B }` — named fields, still positional on the wire.
    Struct,
}

struct VariantField {
    /// `Some` for a struct variant field, `None` for a tuple variant field.
    /// Never written to the wire: union members are positional.
    name: Option<String>,
    ty: String,
}

struct Variant {
    name: String,
    shape: VariantShape,
    fields: Vec<VariantField>,
}

/// An enum encoded as `compact-int tag + payload`. The tag is the declaration
/// index.
struct UnionInfo {
    name: String,
    variants: Vec<Variant>,
    generics: Generics,
}

enum Input {
    Struct(Info),
    IntEnum(EnumInfo),
    Union(UnionInfo),
}

/// Map a `#[repr(...)]` spelling to `(rust integer type, width in bytes)`.
/// An enum without an explicit repr defaults to `i32`.
fn repr_type(name: &str) -> Option<(&'static str, usize)> {
    Some(match name {
        "u8" => ("u8", 1),
        "i8" => ("i8", 1),
        "u16" => ("u16", 2),
        "i16" => ("i16", 2),
        "u32" => ("u32", 4),
        "i32" => ("i32", 4),
        "u64" => ("u64", 8),
        "i64" => ("i64", 8),
        "u128" => ("u128", 16),
        "i128" => ("i128", 16),
        "usize" => ("usize", 8),
        "isize" => ("isize", 8),
        _ => return None,
    })
}

/// Decode a Rust string literal's source text into the bytes it denotes.
///
/// A `b{literal}` splice would only work for ASCII — `b"名字"` is not valid
/// byte-string syntax — so the bytes are decoded and re-emitted with
/// [`Literal::byte_string`].
fn literal_bytes(text: &str) -> Res<Vec<u8>> {
    let fail = || format!("`rename` expects a plain string literal, got `{}`", text);

    // Raw strings: r"..." / r#"..."# / r##"..."##
    if let Some(rest) = text.strip_prefix('r') {
        let hashes = rest.len() - rest.trim_start_matches('#').len();
        let open = 1 + hashes; // `"` plus the hashes
        if rest.len() >= 2 * open {
            let body = &rest[open..rest.len() - open];
            return Ok(body.as_bytes().to_vec());
        }
        return Err(Error::new(fail()));
    }

    if text.len() < 2 || !text.starts_with('"') || !text.ends_with('"') {
        return Err(Error::new(fail()));
    }
    let body = &text[1..text.len() - 1];

    let mut out = Vec::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        let Some(esc) = chars.next() else {
            return Err(Error::new(fail()));
        };
        match esc {
            'n' => out.push(b'\n'),
            'r' => out.push(b'\r'),
            't' => out.push(b'\t'),
            '0' => out.push(0),
            '\\' => out.push(b'\\'),
            '\'' => out.push(b'\''),
            '"' => out.push(b'"'),
            'x' => {
                let mut hex = String::new();
                for _ in 0..2 {
                    hex.push(chars.next().ok_or_else(fail)?);
                }
                let byte = u8::from_str_radix(&hex, 16).map_err(|_| fail())?;
                out.push(byte);
            }
            'u' => {
                if chars.next() != Some('{') {
                    return Err(Error::new(fail()));
                }
                let mut hex = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) => hex.push(c),
                        None => return Err(Error::new(fail())),
                    }
                }
                let code = u32::from_str_radix(&hex, 16).map_err(|_| fail())?;
                let c = char::from_u32(code).ok_or_else(fail)?;
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            // Line continuation: backslash + newline + leading whitespace.
            '\n' => {
                while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
                    chars.next();
                }
            }
            _ => return Err(Error::new(fail())),
        }
    }
    Ok(out)
}

fn parse_field_options(inner: Vec<Tok>) -> Res<(Option<Vec<u8>>, Kind, Option<String>)> {
    let mut p = Parser::new(inner);
    let mut wire = None;
    let mut kind = Kind::Required;
    let mut with = None;
    while !p.eof() {
        let key = p.ident("binary option")?;
        let key_span = p.last_span();
        match key.as_str() {
            "rename" => {
                p.expect_punct('=')?;
                let (text, span) = match p.bump() {
                    Tok::Literal(s, span) => (s, span),
                    other => {
                        return Err(Error::at(
                            "`rename` expects a plain string literal",
                            other.span(),
                        ))
                    }
                };
                wire = Some(literal_bytes(&text).map_err(|e| Error::at(e.msg, span))?);
            }
            "with" => {
                p.expect_punct('=')?;
                let (text, span) = match p.bump() {
                    Tok::Literal(s, span) => (s, span),
                    other => {
                        return Err(Error::at(
                            "`with` expects a plain string literal naming the converter module",
                            other.span(),
                        ))
                    }
                };
                let bytes = literal_bytes(&text).map_err(|e| Error::at(e.msg, span))?;
                let path = String::from_utf8(bytes).map_err(|_| {
                    Error::at("`with` expects a UTF-8 module path such as \"ticks\"", span)
                })?;
                if path.is_empty() {
                    return Err(Error::at("`with` expects a non-empty module path", span));
                }
                with = Some(path);
            }
            "optional" => kind = Kind::Optional,
            "skip" => kind = Kind::Skip,
            other => {
                return Err(Error::at(
                    format!("unknown `#[binary(...)]` option `{}`", other),
                    key_span,
                ))
            }
        }
        if p.peek_punct(',') {
            p.pos += 1;
        }
        if with.is_some() && kind == Kind::Skip {
            return Err(Error::at(
                "`with` cannot be combined with `skip`: a skipped member is never encoded, so a \
                 converter has nothing to convert",
                key_span,
            ));
        }
    }
    Ok((wire, kind, with))
}

/// Struct- or enum-level `#[binary(...)]`. An unknown option is rejected rather
/// than silently dropped, so a user asking for a tuple never gets a named
/// object.
///
/// `current` is the mode accumulated from earlier `#[binary(...)]` attributes,
/// so `#[binary(tuple)] #[binary(union)]` is an error instead of last-one-wins.
fn parse_struct_options(inner: Vec<Tok>, current: Mode) -> Res<Mode> {
    let mut p = Parser::new(inner);
    let mut mode = current;
    while !p.eof() {
        let key = p.ident("binary option")?;
        let key_span = p.last_span();
        let next = match key.as_str() {
            "tuple" => Mode::Tuple,
            "int" => Mode::Int,
            "union" => Mode::Union,
            other => {
                return Err(Error::at(
                    format!(
                        "unknown or unsupported `#[binary(...)]` option `{}`; supported modes are \
                         `tuple` (structs), `int` (enums) and `union` (enums). Member options such \
                         as `rename` / `optional` / `skip` belong on the field",
                        other
                    ),
                    key_span,
                ))
            }
        };
        if mode != Mode::Named && mode != next {
            return Err(Error::at(
                format!(
                    "conflicting `#[binary(...)]` modes: `{}` cannot be combined with an earlier \
                     mode",
                    key
                ),
                key_span,
            ));
        }
        mode = next;
        if p.peek_punct(',') {
            p.pos += 1;
        }
    }
    Ok(mode)
}

/// Item-level attributes: the `#[binary(...)]` layout mode and `#[repr(...)]`.
///
/// `repr` keeps its span so an unsupported spelling is reported on the attribute
/// rather than on `#[derive(...)]`.
struct ItemAttrs {
    mode: Mode,
    repr: Option<(String, Span)>,
}

fn parse_input(ts: TokenStream) -> Res<Input> {
    let mut p = Parser::new(flatten(ts));

    // Leading attributes (doc comments, other derives, cfg, ...). Item-level
    // `#[binary(...)]` and `#[repr(...)]` are captured here.
    let mut attrs = ItemAttrs {
        mode: Mode::Named,
        repr: None,
    };
    while p.peek_punct('#') {
        p.pos += 1;
        let attr = p.group(Delimiter::Bracket)?;
        let mut ap = Parser::new(attr);
        if ap.peek_ident("binary") {
            ap.pos += 1;
            if ap.peek_group(Delimiter::Parenthesis) {
                let inner = ap.group(Delimiter::Parenthesis)?;
                attrs.mode = parse_struct_options(inner, attrs.mode)?;
            }
        } else if ap.peek_ident("repr") {
            let span = ap.peek().map(Tok::span).unwrap_or_else(Span::call_site);
            ap.pos += 1;
            if ap.peek_group(Delimiter::Parenthesis) {
                let inner = ap.group(Delimiter::Parenthesis)?;
                if let Some(Tok::Ident(name, _)) = inner.first() {
                    attrs.repr = Some((name.clone(), span));
                }
            }
        }
    }
    p.skip_visibility();

    if p.peek_ident("enum") {
        return match attrs.mode {
            Mode::Union => parse_union(&mut p, &attrs).map(Input::Union),
            _ => parse_enum(&mut p, &attrs).map(Input::IntEnum),
        };
    }
    if p.peek_ident("union") {
        let span = p.peek().map(Tok::span).unwrap_or_else(Span::call_site);
        return Err(Error::at(
            "`binary` derive does not support Rust `union` types; declare an enum with \
             `#[binary(union)]` instead",
            span,
        ));
    }
    parse_struct(&mut p, attrs.mode).map(Input::Struct)
}

/// Skip attributes on an enum variant. `#[binary(...)]` is rejected: variant
/// options are not supported, and ignoring them would hand back a layout the
/// user did not ask for (DESIGN §14).
fn skip_variant_attrs(p: &mut Parser) -> Res<()> {
    while p.peek_punct('#') {
        p.pos += 1;
        let span = p.peek().map(Tok::span).unwrap_or_else(Span::call_site);
        let attr = p.group(Delimiter::Bracket)?;
        let ap = Parser::new(attr);
        if ap.peek_ident("binary") {
            return Err(Error::at(
                "`#[binary(...)]` is not supported on enum variants",
                span,
            ));
        }
    }
    Ok(())
}

fn parse_enum(p: &mut Parser, attrs: &ItemAttrs) -> Res<EnumInfo> {
    let kw_span = p.peek().map(Tok::span).unwrap_or_else(Span::call_site);
    match attrs.mode {
        Mode::Int => {}
        Mode::Union => unreachable!("union mode dispatches to parse_union"),
        Mode::Tuple => {
            return Err(Error::at(
                "`#[binary(tuple)]` is only valid on structs",
                kw_span,
            ))
        }
        Mode::Named => return Err(Error::at(
            "enums require `#[binary(int)]` (discriminant) or `#[binary(union)]` (tag + value); \
                 without one of them the layout is ambiguous",
            kw_span,
        )),
    }
    p.expect_ident("enum")?;

    // `#[binary(int)]` is not a struct option, so it arrives as an error from
    // `parse_struct_options`; accept the enum spelling here instead.
    let name = p.ident("enum name")?;
    let generics = parse_generics(p, "enum")?;
    if p.peek_ident("where") {
        return Err(Error::at(
            "`where` clauses are not supported by `binary` derive; a lifetime parameter needs no \
             bound",
            p.peek().map(Tok::span).unwrap_or_else(Span::call_site),
        ));
    }

    let body = loop {
        if p.eof() {
            return Err(Error::new("expected `enum Name { ... }`".to_string()));
        }
        if let Tok::Group(Delimiter::Brace, inner, span) = p.bump() {
            break (inner, span);
        }
    };

    let (repr, width) = match &attrs.repr {
        Some((name, span)) => match repr_type(name) {
            Some((ty, w)) => (ty.to_string(), w),
            None => {
                return Err(Error::at(
                    format!(
                        "unsupported `#[repr({})]` on enum; use an integer repr such as `u8` or \
                         `i32`",
                        name
                    ),
                    *span,
                ))
            }
        },
        None => ("i32".to_string(), 4),
    };

    // Variants: `Name`, `Name = 3`. Tuple / struct variants are out of scope.
    let (body, body_span) = body;
    let mut vp = Parser::new(body);
    let mut variants: Vec<(String, i128)> = Vec::new();
    let mut next: i128 = 0;
    while !vp.eof() {
        // Attributes on the variant (doc comments, cfg, ...) are skipped.
        skip_variant_attrs(&mut vp)?;
        if vp.eof() {
            break;
        }
        let vname = vp.ident("variant name")?;
        if vp.peek_group(Delimiter::Parenthesis) || vp.peek_group(Delimiter::Brace) {
            return Err(Error::new(format!(
                "enum variant `{}` has fields; only unit variants are supported in phase 1",
                vname
            )));
        }
        if vp.peek_punct('=') {
            vp.pos += 1;
            let mut negative = false;
            if vp.peek_punct('-') {
                vp.pos += 1;
                negative = true;
            }
            let text = match vp.bump() {
                Tok::Literal(s, _) => s,
                _ => {
                    return Err(Error::new(format!(
                        "enum variant `{}`: only literal discriminants are supported",
                        vname
                    )))
                }
            };
            let digits = text.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '_');
            let value: i128 = digits.parse().map_err(|_| {
                format!(
                    "enum variant `{}`: only integer literals are supported, got `{}`",
                    vname, text
                )
            })?;
            next = if negative { -value } else { value };
        }
        variants.push((vname, next));
        next += 1;

        if vp.peek_punct(',') {
            vp.pos += 1;
        } else if !vp.eof() {
            return Err(Error::new("expected `,` between enum variants".to_string()));
        }
    }

    if variants.is_empty() {
        return Err(Error::at("enum has no variants", body_span));
    }

    Ok(EnumInfo {
        name,
        repr,
        width,
        variants,
        generics,
    })
}

/// Parse the field list of one union variant: `(A, B)` or `{ a: A, b: B }`.
fn parse_variant_fields(toks: Vec<Tok>, shape: VariantShape) -> Res<Vec<VariantField>> {
    let mut p = Parser::new(toks);
    let mut fields = Vec::new();
    while !p.eof() {
        while p.peek_punct('#') {
            p.pos += 1;
            let span = p.peek().map(Tok::span).unwrap_or_else(Span::call_site);
            let attr = p.group(Delimiter::Bracket)?;
            let ap = Parser::new(attr);
            if ap.peek_ident("binary") {
                return Err(Error::at(
                    "`#[binary(...)]` is not supported on union variant fields; union members are \
                     positional and always encoded, so `rename` / `optional` / `skip` do not apply",
                    span,
                ));
            }
        }
        if p.eof() {
            break;
        }

        match shape {
            VariantShape::Tuple => {
                let ty = p.take_type();
                if ty.is_empty() {
                    return Err(Error::new("expected a union field type"));
                }
                fields.push(VariantField { name: None, ty });
            }
            VariantShape::Struct => {
                p.skip_visibility();
                let fname = p.ident("field name")?;
                p.expect_punct(':')?;
                let ty = p.take_type();
                if ty.is_empty() {
                    return Err(Error::new("expected a union field type"));
                }
                fields.push(VariantField {
                    name: Some(fname),
                    ty,
                });
            }
            VariantShape::Unit => unreachable!("a unit variant has no field list"),
        }

        if p.peek_punct(',') {
            p.pos += 1;
        } else if !p.eof() {
            return Err(Error::new("expected `,` between union variant fields"));
        }
    }
    Ok(fields)
}

/// `#[binary(union)]`: `tag + payload`, tag is a compact integer and equals the
/// declaration index.
///
/// Payload layout follows the same rule as a tuple object of the same members:
/// `encode` writes all but the last member with `auto` and the last one raw;
/// `encode_auto` writes every member with `auto`. Neither form adds a header of
/// its own, so a union is self-delimiting and `auto` never prefixes it.
fn parse_union(p: &mut Parser, attrs: &ItemAttrs) -> Res<UnionInfo> {
    if let Some((name, span)) = &attrs.repr {
        return Err(Error::at(
            format!(
                "`#[repr({})]` does not apply to `#[binary(union)]`: the tag is always a compact \
                 integer, independent of any repr",
                name
            ),
            *span,
        ));
    }
    p.expect_ident("enum")?;
    let name = p.ident("enum name")?;
    let generics = parse_generics(p, "enum")?;
    if p.peek_ident("where") {
        return Err(Error::at(
            "`where` clauses are not supported by `binary` derive; a lifetime parameter needs no \
             bound",
            p.peek().map(Tok::span).unwrap_or_else(Span::call_site),
        ));
    }

    let (body, body_span) = loop {
        if p.eof() {
            return Err(Error::new("expected `enum Name { ... }`".to_string()));
        }
        if let Tok::Group(Delimiter::Brace, inner, span) = p.bump() {
            break (inner, span);
        }
    };

    let mut vp = Parser::new(body);
    let mut variants: Vec<Variant> = Vec::new();
    while !vp.eof() {
        skip_variant_attrs(&mut vp)?;
        if vp.eof() {
            break;
        }
        let vname = vp.ident("variant name")?;
        let vspan = vp.last_span();
        let (shape, fields) = if vp.peek_group(Delimiter::Parenthesis) {
            let inner = vp.group(Delimiter::Parenthesis)?;
            (
                VariantShape::Tuple,
                parse_variant_fields(inner, VariantShape::Tuple)?,
            )
        } else if vp.peek_group(Delimiter::Brace) {
            let inner = vp.group(Delimiter::Brace)?;
            (
                VariantShape::Struct,
                parse_variant_fields(inner, VariantShape::Struct)?,
            )
        } else {
            (VariantShape::Unit, Vec::new())
        };

        if vp.peek_punct('=') {
            return Err(Error::at(
                format!(
                    "enum variant `{}`: `#[binary(union)]` tags follow declaration order, so \
                     explicit discriminants are not supported",
                    vname
                ),
                vp.peek().map(Tok::span).unwrap_or(vspan),
            ));
        }

        variants.push(Variant {
            name: vname,
            shape,
            fields,
        });

        if vp.peek_punct(',') {
            vp.pos += 1;
        } else if !vp.eof() {
            return Err(Error::new("expected `,` between enum variants".to_string()));
        }
    }

    if variants.is_empty() {
        return Err(Error::at("enum has no variants", body_span));
    }

    Ok(UnionInfo {
        name,
        variants,
        generics,
    })
}

fn parse_struct(p: &mut Parser, mode: Mode) -> Res<Info> {
    let kw_span = p.peek().map(Tok::span).unwrap_or_else(Span::call_site);
    match mode {
        Mode::Named | Mode::Tuple => {}
        Mode::Int => {
            return Err(Error::at(
                "`#[binary(int)]` is only valid on enums",
                kw_span,
            ))
        }
        Mode::Union => {
            return Err(Error::at(
                "`#[binary(union)]` is only valid on enums",
                kw_span,
            ))
        }
    }
    p.expect_ident("struct")?;
    let name = p.ident("struct name")?;
    let generics = parse_generics(p, "struct")?;
    if p.peek_ident("where") {
        return Err(Error::at(
            "`where` clauses are not supported by `binary` derive; a lifetime parameter needs no \
             bound",
            p.peek().map(Tok::span).unwrap_or_else(Span::call_site),
        ));
    }

    let body = loop {
        if p.eof() {
            return Err(Error::new("expected a struct with fields".to_string()));
        }
        match p.bump() {
            Tok::Group(Delimiter::Brace, inner, span) => break (inner, span),
            Tok::Group(Delimiter::Parenthesis, _, span) => {
                return Err(Error::at(
                    "tuple structs and unit structs are not supported in phase 1; use a named \
                     struct with `#[binary(tuple)]`",
                    span,
                ));
            }
            _ => {}
        }
    };

    let (body, body_span) = body;
    let mut fp = Parser::new(body);
    let mut fields = Vec::new();
    while !fp.eof() {
        let mut wire = None;
        let mut kind = Kind::Required;
        let mut with = None;

        while fp.peek_punct('#') {
            fp.pos += 1;
            let attr = fp.group(Delimiter::Bracket)?;
            let mut ap = Parser::new(attr);
            if ap.peek_ident("binary") {
                ap.pos += 1;
                if ap.peek_group(Delimiter::Parenthesis) {
                    let inner = ap.group(Delimiter::Parenthesis)?;
                    let (w, k, wd) = parse_field_options(inner)?;
                    if w.is_some() {
                        wire = w;
                    }
                    if k != Kind::Required {
                        kind = k;
                    }
                    if wd.is_some() {
                        with = wd;
                    }
                }
            }
        }

        fp.skip_visibility();
        let fname = fp.ident("field name")?;
        let fspan = fp.last_span();
        fp.expect_punct(':')?;
        let ty = fp.take_type();

        let renamed = wire.is_some();
        let wire = wire.unwrap_or_else(|| fname.as_bytes().to_vec());
        fields.push(Field {
            name: fname,
            ty,
            wire,
            renamed,
            with,
            span: fspan,
            kind,
        });

        if fp.peek_punct(',') {
            fp.pos += 1;
        } else if !fp.eof() {
            return Err(Error::new("expected `,` between fields"));
        }
    }

    if fields.is_empty() {
        return Err(Error::at("struct has no fields", body_span));
    }

    if mode == Mode::Tuple {
        for f in &fields {
            if f.kind != Kind::Required || f.renamed {
                return Err(Error::at(
                    format!(
                        "`#[binary(tuple)]` does not support `rename` / `optional` / `skip`; field \
                         `{}` carries one of them (`with` is allowed — it replaces the converter, \
                         not the layout)",
                        f.name
                    ),
                    f.span,
                ));
            }
        }
    } else {
        // Duplicate wire names compile fine as patterns but make the value
        // undecodable: the second occurrence always reports `DuplicateMember`.
        for (i, f) in fields.iter().enumerate() {
            if let Some(g) = fields[..i].iter().find(|g| g.wire == f.wire) {
                return Err(Error::at(
                    format!(
                        "duplicate wire name {} on field `{}`; `{}` already uses it",
                        display_wire(&f.wire),
                        f.name,
                        g.name
                    ),
                    f.span,
                ));
            }
        }
    }

    Ok(Info {
        name,
        mode,
        fields,
        generics,
    })
}

fn count_slots(info: &Info) -> usize {
    info.fields.iter().filter(|f| f.kind != Kind::Skip).count()
}

fn gen_layout_named(info: &Info) -> String {
    format!(
        "{} ::binary::Layout for {} {{ const LEN: usize = 0; }}\n",
        info.generics.impl_prefix(),
        info.generics.self_ty(&info.name)
    )
}

/// A tuple object is fixed-size exactly when every member is: the lengths are
/// summed, and a single variable-length member yields 0.
///
/// `#[binary(with)]` members contribute their converter's `Repr` length — the
/// field type need not implement `Layout` at all.
fn gen_layout_tuple(info: &Info) -> String {
    let mut out = format!(
        "{} ::binary::Layout for {} {{\n",
        info.generics.impl_prefix(),
        info.generics.self_ty(&info.name)
    );
    out.push_str("    const LEN: usize = {\n");
    out.push_str("        let mut __sum: usize = 0;\n");
    out.push_str("        let mut __all_fixed = true;\n");
    for f in &info.fields {
        out.push_str(&format!(
            "        match <{} as ::binary::Layout>::LEN {{\n\
             \x20           0 => __all_fixed = false,\n\
             \x20           __n => __sum += __n,\n\
             \x20       }}\n",
            f.wire_ty()
        ));
    }
    // No `__sum > 0` guard needed: 0 already *is* the variable-length marker,
    // so an all-zero-sized tuple falls out as variable without a second test.
    out.push_str("        if __all_fixed { __sum } else { 0 }\n");
    out.push_str("    };\n}\n");
    out
}

fn gen_encode_named(info: &Info) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} ::binary::Encode for {} {{\n",
        info.generics.impl_prefix(),
        info.generics.self_ty(&info.name)
    ));
    out.push_str(
        "    fn encode(&self, __e: &mut ::binary::Encoder<'_>) -> ::binary::Result<()> {\n",
    );
    for f in &info.fields {
        match f.kind {
            Kind::Skip => {}
            Kind::Required => out.push_str(&format!(
                "        ::binary::write_named_member(__e, {}, {})?;\n",
                f.wire_literal(),
                f.wire_value()
            )),
            Kind::Optional => out.push_str(&format!(
                // Fully qualified `Default`: with a bare `Default::default()`
                // the comparison is ambiguous for byte containers, because
                // `&[u8]: PartialEq<ByteStr>` and `Vec<u8>: PartialEq<ByteString>`
                // also exist — E0283 on `#[binary(optional)] body: &[u8]`.
                "        if self.{f} != <{t} as ::core::default::Default>::default() {{ \
                 ::binary::write_named_member(__e, {w}, {v})?; }}\n",
                f = f.name,
                t = f.ty,
                w = f.wire_literal(),
                v = f.wire_value()
            )),
        }
    }
    out.push_str("        ::core::result::Result::Ok(())\n");
    out.push_str("    }\n}\n");
    out
}

/// Tuple object: members in declaration order, no names. `encode` writes all but
/// the last member with `auto` and the last one raw; `encode_auto` uses `auto`
/// for every member and adds no header of its own — identical to an N-tuple.
fn gen_encode_tuple(info: &Info) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} ::binary::Encode for {} {{\n",
        info.generics.impl_prefix(),
        info.generics.self_ty(&info.name)
    ));

    out.push_str(
        "    fn encode(&self, __e: &mut ::binary::Encoder<'_>) -> ::binary::Result<()> {\n",
    );
    let (last, head) = info.fields.split_last().expect("checked non-empty");
    for f in head {
        out.push_str(&format!(
            "        ::binary::Encode::encode_auto({}, __e)?;\n",
            f.wire_value()
        ));
    }
    out.push_str(&format!(
        "        ::binary::Encode::encode({}, __e)\n",
        last.wire_value()
    ));
    out.push_str("    }\n");

    out.push_str(
        "    fn encode_auto(&self, __e: &mut ::binary::Encoder<'_>) -> ::binary::Result<()> {\n",
    );
    for f in &info.fields {
        out.push_str(&format!(
            "        ::binary::Encode::encode_auto({}, __e)?;\n",
            f.wire_value()
        ));
    }
    out.push_str("        ::core::result::Result::Ok(())\n");
    out.push_str("    }\n}\n");
    out
}

fn gen_decode_named(info: &Info) -> String {
    let mut arms = String::new();
    let mut inits = String::new();
    let mut slot = 0usize;
    let mut first = true;

    for f in &info.fields {
        if !first {
            inits.push_str(", ");
        }
        first = false;
        if f.kind == Kind::Skip {
            inits.push_str(&format!("{}: ::core::default::Default::default()", f.name));
            continue;
        }
        arms.push_str(&format!(
            "            {} => ::core::option::Option::Some({}usize),\n",
            f.wire_literal(),
            slot
        ));
        let name = Literal::string(&f.name);
        match (&f.with, f.kind) {
            // The payload is already isolated by `NamedMembers::parse`, so a
            // converter decodes it exactly like `T::decode` would — same
            // `Decoder::new(payload)`, only the function differs.
            (Some(m), Kind::Optional) => inits.push_str(&format!(
                "{}: match __m.get({slot}) {{ \
                 ::core::option::Option::Some(__p) => \
                 {m}::from(::binary::Decode::decode(&mut ::binary::Decoder::new(__p))?)?, \
                 ::core::option::Option::None => ::core::default::Default::default() }}",
                f.name
            )),
            (Some(m), _) => inits.push_str(&format!(
                "{}: {m}::from(__m.value({slot}, {name})?)?",
                f.name
            )),
            (None, Kind::Optional) => {
                inits.push_str(&format!("{}: __m.optional({})?", f.name, slot))
            }
            (None, _) => inits.push_str(&format!("{}: __m.value({}, {})?", f.name, slot, name)),
        }
        slot += 1;
    }

    let lt = info.generics.decode_lt();
    let mut out = String::new();
    out.push_str(&format!("{} {{\n", info.generics.decode_impl(&info.name)));
    out.push_str(&format!(
        "    fn decode(__d: &mut ::binary::Decoder<{lt}>) -> ::binary::Result<Self> {{\n"
    ));
    out.push_str(&format!(
        "        let __m = ::binary::NamedMembers::<{}>::parse(__d, |__n| match __n {{\n",
        count_slots(info)
    ));
    out.push_str(&arms);
    out.push_str("            _ => ::core::option::Option::None,\n");
    out.push_str("        })?;\n");
    out.push_str(&format!(
        "        ::core::result::Result::Ok(Self {{ {} }})\n",
        inits
    ));
    out.push_str("    }\n}\n");
    out
}

fn gen_decode_tuple(info: &Info) -> String {
    let bind = |f: &Field| format!("__v_{}", f.name);
    let inits = info
        .fields
        .iter()
        .map(|f| match &f.with {
            Some(m) => format!("{}: {m}::from({})?", f.name, bind(f)),
            None => format!("{}: {}", f.name, bind(f)),
        })
        .collect::<Vec<_>>()
        .join(", ");

    // A converter's `Repr` is what the decoder must produce; naming it keeps
    // inference working in both directions (`auto` and raw).
    let declare = |f: &Field, expr: &str| match &f.with {
        Some(_) => format!("        let {}: {} = {expr};\n", bind(f), f.wire_ty()),
        None => format!("        let {} = {expr};\n", bind(f)),
    };

    let lt = info.generics.decode_lt();
    let mut out = String::new();
    out.push_str(&format!("{} {{\n", info.generics.decode_impl(&info.name)));

    out.push_str(&format!(
        "    fn decode(__d: &mut ::binary::Decoder<{lt}>) -> ::binary::Result<Self> {{\n"
    ));
    let (last, head) = info.fields.split_last().expect("checked non-empty");
    for f in head {
        out.push_str(&declare(f, "::binary::Decode::decode_auto(__d)?"));
    }
    out.push_str(&declare(last, "::binary::Decode::decode(__d)?"));
    out.push_str(&format!(
        "        ::core::result::Result::Ok(Self {{ {} }})\n",
        inits
    ));
    out.push_str("    }\n");

    out.push_str(&format!(
        "    fn decode_auto(__d: &mut ::binary::Decoder<{lt}>) -> ::binary::Result<Self> {{\n"
    ));
    for f in &info.fields {
        out.push_str(&declare(f, "::binary::Decode::decode_auto(__d)?"));
    }
    out.push_str(&format!(
        "        ::core::result::Result::Ok(Self {{ {} }})\n",
        inits
    ));
    out.push_str("    }\n}\n");
    out
}

/// Enum-as-integer: `Layout::LEN` is the underlying width, `encode` writes the
/// discriminant in little-endian, `decode` maps it back and rejects unknown
/// values.
fn gen_encode_enum(info: &EnumInfo) -> String {
    let mut out = format!(
        "{} ::binary::Layout for {} {{ const LEN: usize = {}; }}\n",
        info.generics.impl_prefix(),
        info.generics.self_ty(&info.name),
        info.width
    );
    out.push_str(&format!(
        "{} ::binary::Encode for {} {{\n",
        info.generics.impl_prefix(),
        info.generics.self_ty(&info.name)
    ));
    out.push_str(
        "    fn encode(&self, __e: &mut ::binary::Encoder<'_>) -> ::binary::Result<()> {\n",
    );
    out.push_str("        let __v: ");
    out.push_str(&info.repr);
    out.push_str(" = match *self {\n");
    for (name, value) in &info.variants {
        out.push_str(&format!(
            "            Self::{} => {} as {},\n",
            name, value, info.repr
        ));
    }
    out.push_str("        };\n");
    out.push_str("        ::binary::Encode::encode(&__v, __e)\n");
    out.push_str("    }\n}\n");
    out
}

fn gen_decode_enum(info: &EnumInfo) -> String {
    let lt = info.generics.decode_lt();
    let mut out = format!("{} {{\n", info.generics.decode_impl(&info.name));
    out.push_str(&format!(
        "    fn decode(__d: &mut ::binary::Decoder<{lt}>) -> ::binary::Result<Self> {{\n"
    ));
    out.push_str(&format!(
        "        let __v = <{} as ::binary::Decode>::decode(__d)?;\n",
        info.repr
    ));
    out.push_str("        match __v {\n");
    for (name, value) in &info.variants {
        out.push_str(&format!(
            "            {} => ::core::result::Result::Ok(Self::{}),\n",
            value, name
        ));
    }
    out.push_str(
        "            _ => ::core::result::Result::Err(::binary::Error::message_static(\n\
         \x20               \"invalid enum discriminant\",\n\
         \x20           )),\n",
    );
    out.push_str("        }\n    }\n}\n");
    out
}

/// Destructuring pattern for one union variant, binding fields to `__m{i}`.
fn variant_pattern(v: &Variant) -> String {
    match v.shape {
        VariantShape::Unit => format!("Self::{}", v.name),
        VariantShape::Tuple => {
            let binds = (0..v.fields.len())
                .map(|i| format!("__m{i}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("Self::{}({})", v.name, binds)
        }
        VariantShape::Struct => {
            let binds = v
                .fields
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    format!(
                        "{}: __m{}",
                        f.name
                            .as_deref()
                            .expect("struct variant fields always carry a name"),
                        i
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("Self::{} {{ {} }}", v.name, binds)
        }
    }
}

/// Construction expression for one union variant from `__v{i}` bindings.
fn variant_ctor(v: &Variant) -> String {
    match v.shape {
        VariantShape::Unit => format!("Self::{}", v.name),
        VariantShape::Tuple => {
            let args = (0..v.fields.len())
                .map(|i| format!("__v{i}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("Self::{}({})", v.name, args)
        }
        VariantShape::Struct => {
            let args = v
                .fields
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    format!(
                        "{}: __v{}",
                        f.name
                            .as_deref()
                            .expect("struct variant fields always carry a name"),
                        i
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("Self::{} {{ {} }}", v.name, args)
        }
    }
}

/// Tagged union: `compact-int tag + payload`, always variable-size.
///
/// Payload mirrors a tuple object of the variant's members — `encode` writes all
/// but the last with `auto` and the last one raw, `encode_auto` writes every
/// member with `auto`. No header of the union's own in either form, which is
/// what makes the format self-delimiting.
fn gen_encode_union(info: &UnionInfo) -> String {
    let mut out = format!(
        "{} ::binary::Layout for {} {{ const LEN: usize = 0; }}\n",
        info.generics.impl_prefix(),
        info.generics.self_ty(&info.name)
    );
    out.push_str(&format!(
        "{} ::binary::Encode for {} {{\n",
        info.generics.impl_prefix(),
        info.generics.self_ty(&info.name)
    ));

    for auto in [false, true] {
        out.push_str(if auto {
            "    fn encode_auto(&self, __e: &mut ::binary::Encoder<'_>) -> ::binary::Result<()> {\n"
        } else {
            "    fn encode(&self, __e: &mut ::binary::Encoder<'_>) -> ::binary::Result<()> {\n"
        });
        out.push_str("        match self {\n");
        for (tag, v) in info.variants.iter().enumerate() {
            out.push_str(&format!("            {} => {{\n", variant_pattern(v)));
            out.push_str(&format!(
                "                ::binary::Encoder::write_compact_int(__e, {tag}u32)?;\n"
            ));
            let n = v.fields.len();
            for j in 0..n {
                if !auto && j + 1 == n {
                    out.push_str(&format!(
                        "                ::binary::Encode::encode(__m{j}, __e)?;\n"
                    ));
                } else {
                    out.push_str(&format!(
                        "                ::binary::Encode::encode_auto(__m{j}, __e)?;\n"
                    ));
                }
            }
            out.push_str("            }\n");
        }
        out.push_str("        }\n");
        out.push_str("        ::core::result::Result::Ok(())\n    }\n");
    }

    out.push_str("}\n");
    out
}

fn gen_decode_union(info: &UnionInfo) -> String {
    let lt = info.generics.decode_lt();
    let mut out = format!("{} {{\n", info.generics.decode_impl(&info.name));

    for auto in [false, true] {
        out.push_str(&if auto {
            format!("    fn decode_auto(__d: &mut ::binary::Decoder<{lt}>) -> ::binary::Result<Self> {{\n")
        } else {
            format!("    fn decode(__d: &mut ::binary::Decoder<{lt}>) -> ::binary::Result<Self> {{\n")
        });
        out.push_str("        let __tag = ::binary::Decoder::read_compact_int(__d)?;\n");
        out.push_str("        match __tag {\n");
        for (tag, v) in info.variants.iter().enumerate() {
            out.push_str(&format!("            {tag}u32 => {{\n"));
            let n = v.fields.len();
            for (j, f) in v.fields.iter().enumerate() {
                let ty = &f.ty;
                if !auto && j + 1 == n {
                    out.push_str(&format!(
                        "                let __v{j}: {ty} = ::binary::Decode::decode(__d)?;\n"
                    ));
                } else {
                    out.push_str(&format!(
                        "                let __v{j}: {ty} = ::binary::Decode::decode_auto(__d)?;\n"
                    ));
                }
            }
            out.push_str(&format!(
                "                ::core::result::Result::Ok({})\n",
                variant_ctor(v)
            ));
            out.push_str("            }\n");
        }
        out.push_str(
            "            __other => \
             ::core::result::Result::Err(::binary::Error::InvalidTag(__other)),\n",
        );
        out.push_str("        }\n    }\n");
    }

    out.push_str("}\n");
    out
}

fn gen_encode(input: &Input) -> String {
    let info = match input {
        Input::Struct(info) => info,
        Input::IntEnum(info) => return gen_encode_enum(info),
        Input::Union(info) => return gen_encode_union(info),
    };
    let mut out = match info.mode {
        Mode::Named => gen_layout_named(info),
        Mode::Tuple => gen_layout_tuple(info),
        Mode::Int | Mode::Union => unreachable!("rejected while parsing"),
    };
    out.push_str(&match info.mode {
        Mode::Named => gen_encode_named(info),
        Mode::Tuple => gen_encode_tuple(info),
        Mode::Int | Mode::Union => unreachable!(),
    });
    out
}

fn gen_decode(input: &Input) -> String {
    match input {
        Input::Struct(info) => match info.mode {
            Mode::Named => gen_decode_named(info),
            Mode::Tuple => gen_decode_tuple(info),
            Mode::Int | Mode::Union => unreachable!("rejected while parsing"),
        },
        Input::IntEnum(info) => gen_decode_enum(info),
        Input::Union(info) => gen_decode_union(info),
    }
}

/// Re-span a whole token stream (groups included) so `compile_error!` is
/// reported where the offending field / option is, not on `#[derive(...)]`.
fn with_span(ts: TokenStream, span: Span) -> TokenStream {
    ts.into_iter()
        .map(|t| match t {
            TokenTree::Group(g) => {
                let mut ng = Group::new(g.delimiter(), with_span(g.stream(), span));
                ng.set_span(span);
                TokenTree::Group(ng)
            }
            mut other => {
                other.set_span(span);
                other
            }
        })
        .collect()
}

fn compile_error(err: &Error) -> TokenStream {
    let lit = Literal::string(&err.msg);
    match format!("compile_error!({});", lit).parse::<TokenStream>() {
        Ok(ts) => with_span(ts, err.span),
        Err(_) => TokenStream::new(),
    }
}

fn emit(src: &str) -> TokenStream {
    src.parse().unwrap_or_else(|_| {
        let lit = Literal::string("binary-derive generated invalid Rust");
        format!("compile_error!({});", lit)
            .parse()
            .unwrap_or_else(|_| TokenStream::new())
    })
}

#[proc_macro_derive(Encode, attributes(binary))]
pub fn derive_encode(input: TokenStream) -> TokenStream {
    match parse_input(input) {
        Ok(info) => emit(&gen_encode(&info)),
        Err(err) => compile_error(&err),
    }
}

#[proc_macro_derive(Decode, attributes(binary))]
pub fn derive_decode(input: TokenStream) -> TokenStream {
    match parse_input(input) {
        Ok(info) => emit(&gen_decode(&info)),
        Err(err) => compile_error(&err),
    }
}
