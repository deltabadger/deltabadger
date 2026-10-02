//! The page-parity normaliser must see every real difference and nothing else (tests/common/html.rs).
mod common;
use common::html::{first_difference, normalize};

const PAGE: &str = r#"<!DOCTYPE html>
<html lang="en">
  <head>
    <meta name="csrf-token" content="AAAA" />
    <meta name="csp-nonce" content="n1" />
    <link rel="stylesheet" href="/assets/application-968350348a11e8e8745bb923147f1caddd03be17103d6422b9d0cd3559b5b0fd.css" media="all" />
  </head>
  <body class="a b">
    <form action="/login" method="post"><input type="hidden" name="authenticity_token" value="BBBB" />
      <a href="/x">one</a> <a href="/y">two</a>
    </form>
    <turbo-cable-stream-source channel="Turbo::StreamsChannel" signed-stream-name="InVzZXJfMTpwcmVmZXJlbmNlcyI=--aaaa"></turbo-cable-stream-source>
  </body>
</html>"#;

fn same(other: &str) -> bool {
    normalize(PAGE) == normalize(other)
}

#[test]
fn what_differs_by_design_is_masked() {
    assert!(same(&PAGE.replace("AAAA", "another-token").replace("BBBB", "a-third")), "CSRF tokens");
    assert!(same(&PAGE.replace("content=\"n1\"", "content=\"n2\"")), "the nonce");
    assert!(same(&PAGE.replace("968350348a11e8e8745bb923147f1caddd03be17103d6422b9d0cd3559b5b0fd", "2ec3a7c0cf99c409")), "asset fingerprints");
    assert!(same(&PAGE.replace("--aaaa", "--bbbb")), "the signature of a stream name");
    assert!(normalize(PAGE).iter().any(|line| line.contains("[signed user_1:preferences]")), "{:?}", normalize(PAGE));
}

#[test]
fn what_a_browser_ignores_is_ignored() {
    assert!(same(&PAGE.replace("<body class=\"a b\">", "<body   class='a b' >")), "quoting and spacing inside a tag");
    assert!(same(&PAGE.replace("<link rel=\"stylesheet\" href=", "<link href=").replace("media=\"all\" />", "media=\"all\" rel=\"stylesheet\">")), "attribute order, self-closing");
    assert!(same(&PAGE.replace("\n      <a href=\"/x\">", "\n\n\t <a href=\"/x\">")), "how much whitespace");
    assert!(same(&PAGE.replace("one</a>", "&#111;ne</a>")), "how a character is escaped");
    assert_eq!(normalize("<p>a\n\t  b</p>"), normalize("<p>a b</p>"), "any run of HTML whitespace is one space");
}

#[test]
fn whitespace_that_a_reader_sees_is_kept() {
    assert_ne!(normalize("<p>10 USD</p>"), normalize("<p>10\u{a0}USD</p>"), "a non-breaking space is not a space");
    assert_ne!(normalize("<p>a b</p>"), normalize("<p>a\u{2009}b</p>"), "nor is a thin space");
    for tag in ["pre", "textarea", "script", "style"] {
        assert_ne!(normalize(&format!("<{tag}>a  b</{tag}>")), normalize(&format!("<{tag}>a b</{tag}>")), "inside <{tag}> whitespace is content");
        assert_ne!(normalize(&format!("<{tag}>a\nb</{tag}>")), normalize(&format!("<{tag}>a b</{tag}>")), "inside <{tag}> a line break is content");
    }
    assert_ne!(normalize("<pre><b>a  b</b></pre>"), normalize("<pre><b>a b</b></pre>"), "also below an element inside <pre>");
}

#[test]
fn the_masked_values_can_be_read_before_they_are_masked() {
    let masked = common::html::masked_values(PAGE);
    assert_eq!(masked.tokens, vec!["AAAA", "BBBB"]);
    assert_eq!(masked.streams, vec!["InVzZXJfMTpwcmVmZXJlbmNlcyI=--aaaa"]);
    assert_eq!(masked.assets, vec!["/assets/application-968350348a11e8e8745bb923147f1caddd03be17103d6422b9d0cd3559b5b0fd.css"]);
    // An asset reference with a digest nobody built is masked like any other, so the page still
    // compares equal: it is the harness that must look each one up (tests/pages.rs, script/rust/pages.rb).
    let broken = PAGE.replace("968350348a11e8e8745bb923147f1caddd03be17103d6422b9d0cd3559b5b0fd", "0000000000000000");
    assert!(same(&broken));
    assert_eq!(common::html::masked_values(&broken).assets, vec!["/assets/application-0000000000000000.css"]);
    let more = common::html::masked_values("<img src=\"https://cdn.example/assets/flags/de-0123456789abcdef.svg?v=1\"><a href=\"/assets/plain.txt\">x</a>");
    assert_eq!(more.assets, vec!["/assets/flags/de-0123456789abcdef.svg", "/assets/plain.txt"], "any attribute, with or without a digest");
}

#[test]
fn a_location_on_another_host_never_equals_a_local_path() {
    use common::html::location;
    assert_eq!(location("http://localhost:3000/bots?filter=a"), location("/bots?filter=a"), "Rails' absolute URL and this crate's path");
    assert_eq!(location("http://localhost:3000"), location("/"));
    assert_eq!(location("http://localhost:3000//evil.test/path"), location("/evil.test/path"), "the same page of this host: Rails' router squeezes slashes");
    assert_eq!(location("/login/"), location("/login"));
    // The mutation that must show: a redirect that leaves the site.
    assert_ne!(location("//bots"), location("/bots"), "`//bots` is the host `bots`");
    assert_ne!(location("//bots"), location("http://localhost:3000/bots"));
    assert_ne!(location("///bots"), location("/bots"));
    for elsewhere in ["https://localhost:3000/bots", "http://localhost:3000.evil.test/bots", "http://localhost:30001/bots", "http://localhost:3000@evil.test/bots", "http://evil.test/bots", "bots"] {
        assert_ne!(location(elsewhere), location("/bots"), "{elsewhere}");
    }
    assert_ne!(location("//bots"), location("//other"), "what is elsewhere is kept whole");
}

#[test]
fn one_changed_character_anywhere_is_a_difference() {
    for (what, changed) in [
        ("text", PAGE.replace(">one<", ">onE<")),
        ("an attribute value", PAGE.replace("class=\"a b\"", "class=\"a c\"")),
        ("the order of classes", PAGE.replace("class=\"a b\"", "class=\"b a\"")),
        ("a trailing space in a value", PAGE.replace("class=\"a b\"", "class=\"a b \"")),
        ("an attribute name", PAGE.replace("method=\"post\"", "methods=\"post\"")),
        ("a missing attribute", PAGE.replace(" media=\"all\"", "")),
        ("an element name", PAGE.replace("<a href=\"/y\">two</a>", "<b href=\"/y\">two</b>")),
        ("whitespace between two inline elements, present or not", PAGE.replace("</a> <a", "</a><a")),
        ("the stream name", PAGE.replace("InVzZXJfMTpwcmVmZXJlbmNlcyI=", "InVzZXJfMjpwcmVmZXJlbmNlcyI=")),
        ("an asset's name", PAGE.replace("/assets/application-", "/assets/applicatiom-")),
        ("a path outside /assets", PAGE.replace("action=\"/login\"", "action=\"/logim\"")),
        ("the language", PAGE.replace("lang=\"en\"", "lang=\"de\"")),
        ("the doctype", PAGE.replace("<!DOCTYPE html>\n", "")),
    ] {
        assert!(!same(&changed), "{what} must be seen");
        assert!(first_difference(&normalize(PAGE), &normalize(&changed)).is_some(), "{what} must be reported");
    }
}
