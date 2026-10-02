//! The OAuth 2.1 provider in front of the MCP server, as Doorkeeper 6 and the app's own controllers
//! (app/controllers/oauth) serve it, on Doorkeeper's tables as Doorkeeper writes them. Rails and this
//! crate can therefore take turns on one database: a client registered and authorised under one
//! refreshes and calls under the other. tests/oauth.rs holds every answer here to Rails'.
//!
//! This file has what a client calls on its own, without a browser: the two discovery documents,
//! registration, the token endpoint and revocation. None of them is cookie-authenticated, so they
//! sit outside the session and CSRF pipeline (`api`) and never read or write the session cookie.
//! The pages a browser sees are in web::consent; the bearer-token check is web::bearer.
//!
//! Codes, tokens, refresh tokens and client ids are stored as they are issued, in plain text
//! (Doorkeeper's default, which the app does not change). Nothing here logs one or puts one in an
//! error body.
use super::{header_text, headers, rate_limit, App, Params, WebError};
use crate::codec::{format_time, parse_time};
use crate::engine::{Clock, EngineError};
use axum::extract::{ConnectInfo, Extension, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64URL, Engine};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;

/// config/initializers/doorkeeper.rb: `default_scopes :mcp`, `optional_scopes :api`.
pub const SCOPES: [&str; 2] = ["mcp", "api"];
pub const DEFAULT_SCOPE: &str = "mcp";
/// `access_token_expires_in 1.hour`, and Doorkeeper's default for an authorization code.
pub const ACCESS_TOKEN_SECONDS: i64 = 3600;
pub const CODE_SECONDS: i64 = 600;

/// Doorkeeper's English error texts (its config/locales/en.yml): the gem's, so not in config/locales.
pub mod text {
    pub const INVALID_CLIENT: &str = "Client authentication failed due to unknown client, no client authentication included, or unsupported authentication method.";
    pub const INVALID_GRANT: &str = "The provided authorization grant is invalid, expired, revoked, does not match the redirection URI used in the authorization request, or was issued to another client.";
    pub const INVALID_REDIRECT_URI: &str = "The requested redirect URI is malformed or doesn't match the client redirect URI.";
    pub const INVALID_SCOPE: &str = "The requested scope is invalid, unknown, or malformed.";
    pub const UNAUTHORIZED_CLIENT: &str = "The client is not authorized to perform this request using this method.";
    pub const ACCESS_DENIED: &str = "The resource owner or authorization server denied the request.";
    pub const UNSUPPORTED_RESPONSE_TYPE: &str = "The authorization server does not support this response type.";
    pub const UNSUPPORTED_RESPONSE_MODE: &str = "The authorization server does not support this response mode.";
    pub const UNSUPPORTED_GRANT_TYPE: &str = "The authorization grant type is not supported by the authorization server.";
    pub const CODE_CHALLENGE_REQUIRED: &str = "Code challenge is required.";
    pub const CODE_CHALLENGE_METHOD: &str = "The code_challenge_method must be S256.";
    pub const MULTIPLE_CLIENT_AUTH: &str = "The request utilizes more than one mechanism for authenticating the client.";
    pub const REVOKE_UNAUTHORIZED: &str = "You are not authorized to revoke this token";
    pub fn missing(parameter: &str) -> String { format!("Missing required parameter: {parameter}.") }
}

/// Ruby's `Base64.decode64` (`String#unpack1("m")`), which takes what a strict decoder refuses:
/// every character outside the alphabet is passed over, whitespace included; a `=` where the third
/// or fourth character of a group would stand ends the value; and a last group of two or three
/// characters gives one or two bytes. A client's `Authorization: Basic` value is decoded this way
/// before its authentication methods are counted, so one that Rails reads is read here.
pub fn decode64(text: &str) -> Vec<u8> {
    let six = |byte: u8| match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let (mut out, mut group): (Vec<u8>, Vec<u8>) = (Vec::with_capacity(text.len() / 4 * 3 + 2), Vec::with_capacity(4));
    for byte in text.bytes() {
        match six(byte) {
            Some(bits) => {
                group.push(bits);
                if let [a, b, c, d] = group[..] {
                    out.extend([a << 2 | b >> 4, b << 4 | c >> 2, c << 6 | d]);
                    group.clear();
                }
            }
            None if byte == b'=' && group.len() >= 2 => break,
            None => {}
        }
    }
    match group[..] {
        [a, b] => out.push(a << 2 | b >> 4),
        [a, b, c] => out.extend([a << 2 | b >> 4, b << 4 | c >> 2]),
        _ => {}
    }
    out
}

/// Doorkeeper's ClientSecretBasic.credentials_from: the client id and secret of an
/// `Authorization: Basic` header, or nothing when the header is not one or names no client. The
/// scheme is `basic` in any case and one space; the value is decoded as Ruby decodes it and cut at
/// its first colon.
pub fn basic_credentials(authorization: &str) -> Option<(String, Option<String>)> {
    let value = authorization.get(..6).filter(|scheme| scheme.eq_ignore_ascii_case("basic ")).map(|_| &authorization[6..])?;
    let decoded = decode64(value);
    let (id, secret) = match decoded.iter().position(|&byte| byte == b':') {
        Some(colon) => (&decoded[..colon], Some(&decoded[colon + 1..])),
        None => (&decoded[..], None),
    };
    // Ruby's `blank?`. Bytes that are not text name no client that exists: they are kept as a marker, not dropped.
    if id.iter().all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')) { return None; }
    Some((String::from_utf8_lossy(id).into_owned(), secret.map(|secret| String::from_utf8_lossy(secret).into_owned())))
}

/// Ruby's `blank?` for a parameter: absent, empty, or only whitespace.
pub fn present(value: Option<&str>) -> Option<&str> {
    value.filter(|v| !v.trim().is_empty())
}

/// `SecureRandom.urlsafe_base64(32)`: a code, an access token, a refresh token, a client id.
pub fn new_token() -> String {
    B64URL.encode(rand::random::<[u8; 32]>())
}

/// Doorkeeper::OAuth::Scopes.from_string: split on whitespace, each name once, in the order given.
pub fn scopes(text: &str) -> Vec<&str> {
    let mut all: Vec<&str> = Vec::new();
    for scope in text.split_whitespace() {
        if !all.contains(&scope) { all.push(scope); }
    }
    all
}

/// ScopeChecker.valid?: not blank, no tab or line break, and every name among `allowed`. Asked of
/// the words as they stand, in one pass: `scopes` compares every name with those before it, which
/// is only cheap for a string that passed here (it then has at most `allowed`'s names).
pub fn scopes_valid(requested: &str, allowed: &[&str]) -> bool {
    !requested.trim().is_empty() && !requested.contains(['\n', '\r', '\t']) && requested.split_whitespace().all(|scope| allowed.contains(&scope))
}

/// A URI as Ruby's `URI.parse` (URI::RFC3986_Parser) reads it, for the questions asked here: the
/// same strings parse, into the same parts. `None` where Ruby raises `URI::InvalidURIError`. Ruby is
/// strict about every part but the query, which may hold any character but `#` and is then
/// rewritten (`normal_query`); a redirect URI a client registered under Rails must not be refused
/// here over a character Ruby let through, and must be compared as Ruby compares it.
/// tests/web.rs holds `parse`, `to_string` and the rules built on them to vectors recorded from Ruby.
#[derive(Debug, PartialEq, Eq)]
pub struct Uri {
    /// Lower case, as Ruby gives it.
    pub scheme: Option<String>,
    pub userinfo: Option<String>,
    /// As written; an IPv6 literal keeps its brackets.
    pub host: Option<String>,
    /// The port as written, without leading zeros. `None` when there is none or it is empty: Ruby
    /// then has the scheme's default.
    pub port: Option<String>,
    pub path: String,
    /// As Ruby keeps it: see `normal_query`.
    pub query: Option<String>,
    pub fragment: Option<String>,
    /// `scheme:rest` with no authority and no leading slash (`urn:x`, `mailto:a@b`).
    pub opaque: bool,
}

/// URI::Generic#query=: tabs, carriage returns and line feeds are taken out, and every character
/// outside ``!$%&()*+,-./0-9:;=?@A-Z[\]^_a-z{|}~`` is written as `%XX`. `None` for the one thing
/// Ruby refuses in a query: a `%` followed by two characters that are both not hex digits.
fn normal_query(query: &str) -> Option<String> {
    let kept: Vec<u8> = query.bytes().filter(|b| !matches!(b, b'\t' | b'\r' | b'\n')).collect();
    if kept.windows(3).any(|w| w[0] == b'%' && !w[1].is_ascii_hexdigit() && !w[2].is_ascii_hexdigit()) {
        return None;
    }
    let mut out = String::with_capacity(kept.len());
    for byte in kept {
        if matches!(byte, b'!' | b'$'..=b'&' | b'('..=b';' | b'=' | b'?'..=b'_' | b'a'..=b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    Some(out)
}

impl Uri {
    pub fn parse(uri: &str) -> Option<Self> {
        if !uri.is_ascii() { return None; }
        let (rest, fragment) = uri.split_once('#').map_or((uri, None), |(rest, fragment)| (rest, Some(fragment)));
        let (rest, query) = rest.split_once('?').map_or((rest, None), |(rest, query)| (rest, Some(query)));
        let is_scheme = |text: &str| text.starts_with(|c: char| c.is_ascii_alphabetic()) && text.bytes().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b));
        let (scheme, rest) = match rest.split_once(':') {
            Some((scheme, rest)) if is_scheme(scheme) => (Some(scheme.to_ascii_lowercase()), rest),
            _ => (None, rest),
        };
        let (authority, path) = match rest.strip_prefix("//") {
            Some(after) => { let (authority, path) = after.split_at(after.find('/').unwrap_or(after.len())); (Some(authority), path) }
            None => (None, rest),
        };
        // Without a scheme the first segment of a path cannot hold a colon (it would read as one).
        if scheme.is_none() && authority.is_none() && path.split('/').next().is_some_and(|first| first.contains(':')) { return None; }
        let (userinfo, host, port) = match authority {
            None => (None, None, None),
            Some(authority) => {
                let (userinfo, host_port) = authority.rsplit_once('@').map_or((None, authority), |(userinfo, host)| (Some(userinfo), host));
                let (host, port) = match host_port.strip_prefix('[') {
                    Some(literal) => {
                        let (address, after) = literal.split_once(']')?;
                        address.parse::<std::net::Ipv6Addr>().ok()?;
                        (&host_port[..address.len() + 2], after.strip_prefix(':').or(after.is_empty().then_some(""))?)
                    }
                    None => host_port.split_once(':').unwrap_or((host_port, "")),
                };
                if !port.bytes().all(|b| b.is_ascii_digit()) || !uri_characters(userinfo.unwrap_or(""), ":") || (!host.starts_with('[') && !uri_characters(host, "")) { return None; }
                // Ruby keeps the port as a number: `:0443` is 443.
                let port = (!port.is_empty()).then(|| { let digits = port.trim_start_matches('0'); if digits.is_empty() { "0" } else { digits }.to_string() });
                // An empty userinfo (`http://@host/`) is none.
                (userinfo.filter(|userinfo| !userinfo.is_empty()).map(str::to_string), Some(host.to_string()), port)
            }
        };
        let query = match query { Some(query) => Some(normal_query(query)?), None => None };
        if !uri_characters(path, ":@/") || !uri_characters(fragment.unwrap_or(""), ":@/?") { return None; }
        let opaque = scheme.is_some() && authority.is_none() && !path.is_empty() && !path.starts_with('/');
        Some(Self { scheme, userinfo, host, port, path: path.to_string(), query, fragment: fragment.map(str::to_string), opaque })
    }

    fn hypertext(&self) -> bool { matches!(self.scheme.as_deref(), Some("http" | "https")) }
    fn has_host(&self) -> bool { self.host.as_deref().is_some_and(|host| !host.is_empty()) }

    /// URIChecker.loopback_uri?: `localhost`, 127.0.0.0/8 or ::1.
    fn loopback(&self) -> bool {
        let Some(host) = self.host.as_deref() else { return false };
        let address = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
        host == "localhost" || address.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
    }
}

/// URI::Generic#to_s, for a URI that is not opaque (no other is ever redirected to): the parts put
/// together again, which is not always the text that was parsed. The scheme is in lower case, the
/// port is a number and is left out when it is the scheme's own, and the query is the rewritten one.
impl std::fmt::Display for Uri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(scheme) = &self.scheme { write!(f, "{scheme}:")?; }
        if self.host.is_some() { f.write_str("//")?; }
        if let Some(userinfo) = &self.userinfo { write!(f, "{userinfo}@")?; }
        if let Some(host) = &self.host { f.write_str(host)?; }
        let default_port = match self.scheme.as_deref() { Some("http" | "ws") => Some("80"), Some("https" | "wss") => Some("443"), _ => None };
        if let Some(port) = self.port.as_deref().filter(|port| Some(*port) != default_port) { write!(f, ":{port}")?; }
        f.write_str(&self.path)?;
        if let Some(query) = &self.query { write!(f, "?{query}")?; }
        if let Some(fragment) = &self.fragment { write!(f, "#{fragment}")?; }
        Ok(())
    }
}

/// Whether every character of `part` may stand there: unreserved, sub-delims, `extra`, or `%XX`.
fn uri_characters(part: &str, extra: &str) -> bool {
    let bytes = part.as_bytes();
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        if byte == b'%' {
            if !(bytes.get(at + 1).is_some_and(u8::is_ascii_hexdigit) && bytes.get(at + 2).is_some_and(u8::is_ascii_hexdigit)) { return false; }
            at += 3;
            continue;
        }
        if !(byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=".contains(&byte) || extra.as_bytes().contains(&byte)) { return false; }
        at += 1;
    }
    true
}

/// Doorkeeper's URIChecker.valid_for_authorization?: whether `requested` may be used with a client
/// whose registered redirect URIs are `registered` (whitespace-separated). It must be a URI that can
/// be redirected to (a scheme that is not a script's, a host when it is http(s), no fragment), and
/// equal to a registered one character for character, or, when both are loopback addresses
/// (RFC 8252 section 7.3), equal in everything but the port.
pub fn redirect_uri_allowed(requested: &str, registered: &str) -> bool {
    let Some(uri) = Uri::parse(requested) else { return false };
    let redirectable = uri.scheme.as_deref().is_some_and(|scheme| !["javascript", "vbscript", "data", "localhost"].contains(&scheme))
        && (!uri.hypertext() || uri.has_host()) && uri.fragment.is_none() && !uri.opaque;
    redirectable && registered.split_whitespace().any(|known| {
        known == requested || Uri::parse(known).is_some_and(|known| {
            uri.loopback() && known.loopback() && uri.scheme == known.scheme && uri.userinfo == known.userinfo && uri.host == known.host
                && uri.path == known.path && uri.query == known.query && uri.fragment == known.fragment
        })
    })
}

/// Doorkeeper's RedirectUriValidator, which Rails runs on a client's row before it is saved: what
/// is wrong with `redirect_uri`, the registered URIs as they are stored (joined by line feeds), in
/// Rails' words, or nothing. The validator looks at the stored text split at whitespace, not at the
/// URIs as they were sent: a URI with a space in its query is two entries here, as it will be for
/// `redirect_uri_allowed`. Each kind of error is named once, in the order it was first met; an
/// entry Ruby cannot parse ends the check.
pub fn redirect_uri_errors(redirect_uri: &str) -> Vec<&'static str> {
    let mut errors: Vec<&'static str> = Vec::new();
    let mut add = |error: &'static str| if !errors.contains(&error) { errors.push(error) };
    // String#split without a pattern: at spaces, tabs, line feeds, vertical tabs, form feeds, carriage returns.
    for entry in redirect_uri.split([' ', '\t', '\n', '\x0B', '\x0C', '\r']).filter(|entry| !entry.is_empty()) {
        if ["urn:ietf:wg:oauth:2.0:oob", "urn:ietf:wg:oauth:2.0:oob:auto"].contains(&entry) { continue; }
        let Some(uri) = Uri::parse(entry) else { add("Redirect URI must be a valid URI."); break };
        let blank_host = !uri.has_host();
        if matches!(uri.scheme.as_deref(), Some("javascript" | "vbscript" | "data")) { add("Redirect URI is forbidden by the server."); }
        if uri.fragment.is_some() { add("Redirect URI cannot contain a fragment."); }
        if uri.opaque || uri.scheme.as_deref() == Some("localhost") { add("Redirect URI must specify a scheme."); }
        if uri.scheme.is_none() && blank_host { add("Redirect URI must be an absolute URI."); }
        if uri.hypertext() && blank_host { add("Redirect URI must be a valid URI."); }
    }
    errors
}

/// URI.decode_www_form_component, on bytes: `+` is a space and `%XX` a byte. A `%` that is not
/// followed by two hex digits stays as it is (Ruby raises there, and Rails answers 500).
fn unescape(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let hex = |b: u8| char::from(b).to_digit(16).and_then(|digit| u8::try_from(digit).ok());
    let (mut out, mut at) = (Vec::with_capacity(bytes.len()), 0);
    while let Some(&byte) = bytes.get(at) {
        let escaped = if byte == b'%' { bytes.get(at + 1).copied().and_then(hex).zip(bytes.get(at + 2).copied().and_then(hex)) } else { None };
        match escaped {
            Some((high, low)) => { out.push(high << 4 | low); at += 3; }
            None => { out.push(if byte == b'+' { b' ' } else { byte }); at += 1; }
        }
    }
    out
}

/// Rack::Utils.escape: a query name or value as Doorkeeper writes it into a redirect.
fn escape(bytes: &[u8]) -> String {
    form_urlencoded::byte_serialize(bytes).collect()
}

/// Doorkeeper's URIBuilder: `redirect_uri` with `parameters` added to its query (replacing a
/// parameter of the same name where it stands), or as its fragment, and the whole written as Ruby
/// writes a URI (`Uri`'s `to_string`). The URI's own query is read as Rack::Utils.parse_query reads
/// it: a name without `=` has no value, a name given again after a value collects both. A parameter
/// whose one value is blank is left out, the answer's and the URI's own alike.
pub fn redirect_with(redirect_uri: &str, parameters: &[(&str, &str)], on_fragment: bool) -> String {
    // Nothing is redirected to before `redirect_uri_allowed` parsed it; a text that does not parse is returned as it is.
    let Some(mut uri) = Uri::parse(redirect_uri) else { return redirect_uri.to_string() };
    let parameters = parameters.iter().filter(|(_, value)| !value.trim().is_empty());
    type Pairs = Vec<(Vec<u8>, Vec<Option<Vec<u8>>>)>;
    let build = |pairs: &Pairs| -> String {
        pairs.iter().flat_map(|(name, values)| values.iter().map(move |value| match value {
            Some(value) => format!("{}={}", escape(name), escape(value)),
            None => escape(name),
        })).collect::<Vec<_>>().join("&")
    };
    if on_fragment {
        let pairs: Pairs = parameters.map(|(name, value)| (name.as_bytes().to_vec(), vec![Some(value.as_bytes().to_vec())])).collect();
        uri.fragment = Some(build(&pairs));
        return uri.to_string();
    }
    let mut pairs: Pairs = Vec::new();
    for pair in uri.query.as_deref().unwrap_or("").split('&').map(|pair| pair.trim_start_matches(' ')).filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').map_or((unescape(pair), None), |(name, value)| (unescape(name), Some(unescape(value))));
        match pairs.iter_mut().find(|(known, _)| *known == name) {
            // Rack starts a list only where the name already has a value.
            Some((_, values)) if values.len() > 1 || values[0].is_some() => values.push(value),
            Some((_, values)) => *values = vec![value],
            None => pairs.push((name, vec![value])),
        }
    }
    for (name, value) in parameters {
        let value = vec![Some(value.as_bytes().to_vec())];
        match pairs.iter_mut().find(|(known, _)| known == name.as_bytes()) {
            Some((_, values)) => *values = value,
            None => pairs.push((name.as_bytes().to_vec(), value)),
        }
    }
    let blank = |value: &Option<Vec<u8>>| value.as_ref().is_none_or(|value| String::from_utf8_lossy(value).trim().is_empty());
    pairs.retain(|(_, values)| values.len() > 1 || !blank(&values[0]));
    uri.query = Some(build(&pairs));
    uri.to_string()
}

/// How a request names its client (RFC 6749 section 2.3).
pub enum Credentials {
    None,
    /// The client id, and the secret it sent, if any.
    Given(String, Option<String>),
    /// More than one of: a Basic header, a secret in the body, a client assertion.
    Multiple,
}

/// The request's body parameters, then its query's: Rails' `params`, in which the query wins.
pub struct Sent<'a> {
    pub params: &'a Params,
}

impl Sent<'_> {
    /// A body parameter (form field or JSON member). Client credentials are read from the body only.
    pub fn body(&self, name: &str) -> Option<String> {
        self.params.form(name).map(str::to_string).or_else(|| match self.params.json.as_ref()?.get(name)? {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            Value::Bool(flag) => Some(flag.to_string()),
            _ => None,
        })
    }

    pub fn get(&self, name: &str) -> Option<String> {
        self.params.query(name).map(str::to_string).or_else(|| self.body(name))
    }

    /// A parameter that has to say something: Ruby's `presence`.
    pub fn present(&self, name: &str) -> Option<String> {
        self.get(name).filter(|value| !value.trim().is_empty())
    }
}

/// Doorkeeper's client authentication methods `client_secret_basic`, `client_secret_post` and `none`.
pub fn credentials(headers: &HeaderMap, sent: &Sent) -> Credentials {
    let authorization = header_text(headers, "authorization").unwrap_or("");
    let basic = basic_credentials(authorization);
    let body_id = sent.body("client_id").filter(|id| !id.trim().is_empty());
    let secret = sent.body("client_secret").filter(|secret| !secret.trim().is_empty());
    let assertion = sent.body("client_assertion").filter(|assertion| !assertion.trim().is_empty());
    let asserted = assertion.is_some() && sent.body("client_assertion_type").as_deref() == Some("urn:ietf:params:oauth:client-assertion-type:jwt-bearer");
    if usize::from(basic.is_some()) + usize::from(body_id.is_some() && secret.is_some()) + usize::from(asserted) > 1 {
        return Credentials::Multiple;
    }
    if let Some((id, basic_secret)) = basic {
        // A client id in the body must be the one in the header.
        return if body_id.as_ref().is_some_and(|body| *body != id) { Credentials::None } else { Credentials::Given(id, basic_secret) };
    }
    // `none` is for a request with no other client authentication: a Bearer header is not one.
    let other_header = !authorization.trim().is_empty() && !authorization.trim_start_matches([' ', '\t']).get(..7).is_some_and(|scheme| scheme.eq_ignore_ascii_case("bearer ") || scheme.eq_ignore_ascii_case("bearer\t"));
    match (body_id, secret) {
        (Some(id), Some(secret)) => Credentials::Given(id, Some(secret)),
        (Some(id), None) if !other_header && assertion.is_none() => Credentials::Given(id, None),
        _ => Credentials::None,
    }
}

/// A JSON body as Rails renders one. `Cache-Control` is Rack's for the status unless the caller set it.
fn json_response(status: StatusCode, body: &Value) -> Response {
    let mut response = (status, [(header::CONTENT_TYPE, "application/json; charset=utf-8")], body.to_string()).into_response();
    // Rack::ETag answers a 201 as it answers a 200.
    if status == StatusCode::CREATED {
        response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("max-age=0, private, must-revalidate"));
    }
    response
}

/// Doorkeeper's ErrorResponse at the token endpoint: the error as JSON and in `WWW-Authenticate`.
pub fn token_error(error: &str, description: &str) -> Response {
    let status = if error == "invalid_client" { StatusCode::UNAUTHORIZED } else { StatusCode::BAD_REQUEST };
    let mut response = json_response(status, &json!({ "error": error, "error_description": description }));
    // RFC 6750: characters outside %x20-21 / %x23-5B / %x5D-7E become "_".
    let clean: String = description.chars().map(|c| if matches!(c, ' '..='!' | '#'..='[' | ']'..='~') { c } else { '_' }).collect();
    if let Ok(challenge) = HeaderValue::from_str(&format!("Bearer realm=\"Doorkeeper\", error=\"{error}\", error_description=\"{clean}\"")) {
        response.headers_mut().insert(header::WWW_AUTHENTICATE, challenge);
    }
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// AppConfig::MCP_TOOL_DEFAULTS: every tool and whether the owner has it on before any override, in
/// the catalogue's order. The names are permanent identifiers: they are what a grant stores.
/// AppConfig::REST_TOOL_DEFAULTS is the same names, all off. tests/oauth.rs compares both with Rails, and tests/web.rs with app/models/app_config.rb.
pub const TOOL_DEFAULTS: [(&str, bool); 39] = [
    ("list_bots", true), ("get_bot_details", true), ("list_exchanges", true), ("get_exchange_balances", true), ("get_portfolio_summary", true),
    ("list_transactions", true), ("create_bot", false), ("start_bot", false), ("stop_bot", false), ("update_bot_settings", false),
    ("start_rule", false), ("stop_rule", false), ("update_rule_settings", false), ("list_open_orders", true), ("market_buy", false),
    ("market_sell", false), ("limit_buy", false), ("limit_sell", false), ("cancel_order", false), ("list_tax_jurisdictions", true),
    ("generate_tax_report", true), ("get_tax_report_status", true), ("download_tax_report", true), ("export_transactions_csv", true),
    ("list_account_transactions", true), ("list_rules", true), ("create_rule", false), ("delete_rule", false), ("list_indices", true),
    ("create_index_bot", false), ("create_signal_bot", false), ("delete_bot", false), ("archive_bot", false), ("unarchive_bot", false),
    ("liquidate_exited_asset", false), ("answer_redeploy_offer", false), ("sync_tracker", false), ("set_transfer_link", false),
    ("set_transaction_price", false),
];

/// AppConfig::TOOL_GROUPS: the catalogue as the consent screen and Settings group it.
pub const TOOL_GROUPS: [(&str, &[&str]); 4] = [
    ("read", &["list_bots", "get_bot_details", "list_exchanges", "get_exchange_balances", "get_portfolio_summary", "list_transactions",
               "list_open_orders", "list_rules", "list_indices"]),
    ("control", &["create_bot", "start_bot", "stop_bot", "update_bot_settings", "start_rule", "stop_rule", "update_rule_settings", "create_rule",
                  "delete_rule", "create_index_bot", "create_signal_bot", "delete_bot", "archive_bot", "unarchive_bot", "sync_tracker",
                  "set_transfer_link", "set_transaction_price"]),
    ("trade", &["market_buy", "market_sell", "limit_buy", "limit_sell", "cancel_order", "liquidate_exited_asset", "answer_redeploy_offer"]),
    ("tax", &["list_tax_jurisdictions", "generate_tax_report", "get_tax_report_status", "download_tax_report", "export_transactions_csv",
              "list_account_transactions"]),
];

fn data_error(what: &str, error: impl std::fmt::Debug) -> WebError {
    WebError::Engine(EngineError::Data(format!("{what}: {error:?}")))
}

pub(crate) fn time(column: &str, text: Option<String>) -> Result<Option<DateTime<Utc>>, WebError> {
    text.map(|t| parse_time(&t).map_err(|e| data_error(column, e))).transpose()
}

/// Whether `now` is after `from + seconds`. Rails writes lifetimes of 600 and 3600; a number no date
/// can hold (a row written by hand) must not stop the process: such a lifetime is never over, or
/// always when it is negative, as in Ruby's arithmetic.
pub(crate) fn past(from: DateTime<Utc>, seconds: i64, now: DateTime<Utc>) -> bool {
    match Duration::try_seconds(seconds).and_then(|life| from.checked_add_signed(life)) {
        Some(until) => now > until,
        None => seconds < 0,
    }
}

/// A row of `oauth_access_tokens`.
#[derive(Clone, Debug)]
pub struct AccessToken {
    pub id: i64,
    pub application_id: i64,
    pub resource_owner_id: Option<i64>,
    pub scopes: String,
    pub expires_in: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub refresh_token: Option<String>,
    pub previous_refresh_token: String,
}

impl AccessToken {
    /// `column` is `token` or `refresh_token`: the lookup is by the stored, plain value.
    pub fn find(c: &Connection, column: &str, value: &str) -> Result<Option<Self>, WebError> {
        let row = c.query_row(&format!("SELECT id, application_id, resource_owner_id, scopes, expires_in, created_at, revoked_at, refresh_token, previous_refresh_token \
                                        FROM oauth_access_tokens WHERE {column} = ?1"), [value], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get::<_, String>(5)?, r.get::<_, Option<String>>(6)?, r.get(7)?, r.get(8)?))
        }).optional()?;
        let Some((id, application_id, resource_owner_id, scopes, expires_in, created_at, revoked_at, refresh_token, previous_refresh_token)) = row else { return Ok(None) };
        let created_at = parse_time(&created_at).map_err(|e| data_error("oauth_access_tokens.created_at", e))?;
        Ok(Some(Self { id, application_id, resource_owner_id, scopes, expires_in, created_at, revoked_at: time("oauth_access_tokens.revoked_at", revoked_at)?,
                       refresh_token, previous_refresh_token }))
    }

    /// Revocable#revoked?
    pub fn revoked(&self, now: DateTime<Utc>) -> bool { self.revoked_at.is_some_and(|at| at <= now) }

    /// Expirable#expired?: strictly after `created_at + expires_in`; no `expires_in`, no expiry.
    pub fn expired(&self, now: DateTime<Utc>) -> bool { self.expires_in.is_some_and(|seconds| past(self.created_at, seconds, now)) }

    /// Revocable#revoke, as one conditional write: a row that is not revoked is revoked now, and a
    /// row that is revoked keeps the time it has, whatever this request's clock says. `true` when
    /// this call revoked it.
    pub fn revoke(&self, c: &Connection, now: DateTime<Utc>) -> Result<bool, WebError> {
        Ok(c.execute("UPDATE oauth_access_tokens SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL", (format_time(now), self.id))? == 1)
    }
}

/// Runs `work` as one write transaction, at one moment. It takes the write lock at once (BEGIN
/// IMMEDIATE), so what `work` reads cannot change before it writes, and the moment is read from
/// the clock after the lock is held: requests are served in the order they get the lock, which is
/// not the order they arrived in, and a time taken on arrival would let a later request look
/// earlier than what an earlier one already wrote.
pub(crate) fn transaction<T>(c: &Connection, clock: &dyn Clock, work: impl FnOnce(&Connection, DateTime<Utc>) -> Result<T, WebError>) -> Result<T, WebError> {
    c.execute_batch("BEGIN IMMEDIATE")?;
    match work(c, clock.now()) {
        Ok(value) => { c.execute_batch("COMMIT")?; Ok(value) }
        Err(error) => { let _ = c.execute_batch("ROLLBACK"); Err(error) }
    }
}

/// Around every route of this file: rack-attack's limit, then the handler, then the headers Rails'
/// middleware and controllers add. No session is read and no cookie is written: these routes act
/// for whoever holds the client id, the code or the token in the request, never for a browser's
/// signed-in user, which is also why they need no CSRF token.
pub async fn api(State(app): State<App>, request: Request, next: Next) -> Response {
    let Some(params) = request.extensions().get::<Arc<Params>>().cloned() else {
        return WebError::Config("a request reached the routes without passing web::router's entry".into()).into_response();
    };
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|info| info.0.ip());
    let address = rate_limit::client_key(&app.config, request.headers(), peer);
    let mut response = match app.limiter.hit(request.method(), &params.route_path, &address, app.now()) {
        Some(retry_after) => rate_limit::throttled(retry_after),
        None => next.run(request).await,
    };
    headers::policy(response.headers_mut(), &headers::new_nonce(), app.config.force_ssl);
    let status = response.status();
    if response.extensions().get::<headers::BelowControllers>().is_some() {
        headers::cache_control(response.headers_mut(), status, false);
    } else {
        headers::controller_defaults(response.headers_mut(), status, false);
    }
    response
}

/// A `/.well-known/` path this server has no document for: 404, so that a client trying the
/// path-suffixed or OpenID spellings first goes on to the one that exists.
pub async fn absent() -> Response {
    let mut response = (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], "Not Found\n").into_response();
    response.extensions_mut().insert(headers::BelowControllers);
    response
}

/// `request.base_url`, from which both documents are built: the origin the request was made to.
fn base_url(app: &App, headers: &HeaderMap) -> Result<String, WebError> {
    app.config.request_origin(headers).ok_or_else(|| WebError::Config("a request without a Host header".into()))
}

/// GET /.well-known/oauth-authorization-server (RFC 8414).
pub async fn authorization_server(State(app): State<App>, headers: HeaderMap) -> Result<Response, WebError> {
    let base = base_url(&app, &headers)?;
    Ok(json_response(StatusCode::OK, &json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "revocation_endpoint": format!("{base}/oauth/revoke"),
        "scopes_supported": SCOPES,
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": ["none"],
        "code_challenge_methods_supported": ["S256"],
    })))
}

/// GET /.well-known/oauth-protected-resource (RFC 9728).
pub async fn protected_resource(State(app): State<App>, headers: HeaderMap) -> Result<Response, WebError> {
    let base = base_url(&app, &headers)?;
    Ok(json_response(StatusCode::OK, &json!({ "resource": format!("{base}/mcp"), "authorization_servers": [base], "bearer_methods_supported": ["header"] })))
}

/// Ruby's `String#strip`.
fn strip(text: &str) -> &str {
    text.trim_matches([' ', '\t', '\n', '\x0B', '\x0C', '\r', '\0'])
}

const MAX_REDIRECT_URIS: usize = 5;
const MAX_REDIRECT_URI_LENGTH: usize = 2000;
const MAX_CLIENT_NAME_LENGTH: usize = 100;

fn registration_error(error: &str, description: &str) -> Response {
    json_response(StatusCode::BAD_REQUEST, &json!({ "error": error, "error_description": description }))
}

/// `Array(params[:redirect_uris])`, each as text. Rails' `params` is the query and the body, and
/// the query wins: the query's `redirect_uris[]` fields or its one `redirect_uris`, else a JSON
/// array or one JSON value, else the form's fields.
fn redirect_uris(params: &Params) -> Vec<String> {
    let as_text = |value: &Value| match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let fields = |fields: &[(String, String)]| -> Option<Vec<String>> {
        let listed: Vec<String> = fields.iter().filter(|(name, _)| name == "redirect_uris[]").map(|(_, value)| value.clone()).collect();
        if listed.is_empty() { fields.iter().rfind(|(name, _)| name == "redirect_uris").map(|(_, one)| vec![one.clone()]) } else { Some(listed) }
    };
    fields(&params.query).unwrap_or_else(|| match params.json.as_ref().and_then(|json| json.get("redirect_uris")) {
        Some(Value::Array(all)) => all.iter().map(as_text).collect(),
        Some(Value::Null) => Vec::new(),
        Some(one) => vec![as_text(one)],
        None => fields(&params.form).unwrap_or_default(),
    })
}

/// POST /oauth/register (RFC 7591), as Oauth::DynamicRegistrationController: anyone may register a
/// public client; the consent screen is what stands between a registration and any access.
pub async fn register(State(app): State<App>, Extension(params): Extension<Arc<Params>>) -> Result<Response, WebError> {
    let sent = Sent { params: &params };
    let uris = redirect_uris(&params);
    if uris.is_empty() {
        return Ok(registration_error("invalid_client_metadata", "redirect_uris is required"));
    }
    if uris.len() > MAX_REDIRECT_URIS {
        return Ok(registration_error("invalid_client_metadata", &format!("at most {MAX_REDIRECT_URIS} redirect_uris")));
    }
    if uris.iter().any(|uri| uri.chars().count() > MAX_REDIRECT_URI_LENGTH) {
        return Ok(registration_error("invalid_client_metadata", &format!("each redirect_uri must be at most {MAX_REDIRECT_URI_LENGTH} characters")));
    }
    if !uris.iter().all(|uri| Uri::parse(uri).is_some_and(|uri| uri.hypertext() && uri.has_host())) {
        return Ok(registration_error("invalid_redirect_uri", "redirect_uris must be absolute http(s) URLs"));
    }
    // normalize_scopes: blank is the default; else every name must be known, and they are stored sorted.
    let scope = match sent.get("scope").filter(|scope| !scope.trim().is_empty()) {
        None => DEFAULT_SCOPE.to_string(),
        Some(requested) => {
            if !requested.split_whitespace().all(|name| SCOPES.contains(&name)) {
                return Ok(registration_error("invalid_client_metadata", &format!("scope must be a subset of: {}", SCOPES.join(" "))));
            }
            let mut names = scopes(&requested);
            names.sort_unstable();
            names.join(" ")
        }
    };
    // Doorkeeper's own validation of the row, on the text that is stored.
    let errors = redirect_uri_errors(&uris.join("\n"));
    if !errors.is_empty() {
        return Ok(registration_error("invalid_redirect_uri", &errors.join(", ")));
    }
    let name = sent.get("client_name").filter(|name| !name.trim().is_empty()).map(|name| strip(&name).chars().take(MAX_CLIENT_NAME_LENGTH).collect::<String>())
        .unwrap_or_else(|| "MCP Client".to_string());
    let (uid, registration_token, inner) = (new_token(), hex::encode(rand::random::<[u8; 32]>()), app.clone());
    let row = (name.clone(), uid.clone(), uris.join("\n"), scope.clone(), registration_token.clone());
    app.db(move |c| {
        let row = (row.0, row.1, row.2, row.3, row.4, format_time(inner.now()));
        c.execute("INSERT INTO oauth_applications (name, uid, redirect_uri, scopes, confidential, registration_access_token, token_endpoint_auth_method, \
                   grant_types, response_types, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 0, ?5, 'none', 'authorization_code', 'code', ?6, ?6)", row)?;
        Ok(())
    }).await?;
    Ok(json_response(StatusCode::CREATED, &json!({
        "client_id": uid, "client_name": name, "redirect_uris": uris, "registration_access_token": registration_token,
        "token_endpoint_auth_method": "none", "grant_types": ["authorization_code"], "response_types": ["code"], "scope": scope,
    })))
}

/// A row of `oauth_applications`: a client.
#[derive(Clone, Debug)]
pub struct Application {
    pub id: i64,
    pub uid: String,
    pub name: String,
    pub secret: Option<String>,
    /// The registered redirect URIs, whitespace-separated.
    pub redirect_uri: String,
    pub scopes: String,
    pub confidential: bool,
    /// The per-user REST token's application: refused every grant flow (doorkeeper.rb).
    pub personal: bool,
}

impl Application {
    fn read(c: &Connection, condition: &str, value: &dyn rusqlite::ToSql) -> Result<Option<Self>, WebError> {
        Ok(c.query_row(&format!("SELECT id, uid, name, secret, redirect_uri, scopes, confidential, personal_access_token FROM oauth_applications WHERE {condition} = ?1"), [value], |r| {
            Ok(Self { id: r.get(0)?, uid: r.get(1)?, name: r.get(2)?, secret: r.get(3)?, redirect_uri: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                      scopes: r.get(5)?, confidential: r.get(6)?, personal: r.get(7)? })
        }).optional()?)
    }

    pub fn by_uid(c: &Connection, uid: &str) -> Result<Option<Self>, WebError> { Self::read(c, "uid", &uid) }
    pub fn by_id(c: &Connection, id: i64) -> Result<Option<Self>, WebError> { Self::read(c, "id", &id) }

    /// The scopes a request may ask this client for: its own, or the server's when it has none.
    pub fn allowed_scopes(&self) -> Vec<&str> {
        let own = scopes(&self.scopes);
        if own.is_empty() { SCOPES.to_vec() } else { own }
    }
}
