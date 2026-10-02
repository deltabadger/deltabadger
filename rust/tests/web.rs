//! The web layer's pure parts, each held to what Rails recorded (tests/fixtures/ruby_vectors.json).
mod common;

mod embedded_assets {
    use deltabadger::web::assets;

    #[test]
    fn every_asset_a_template_names_is_embedded_under_a_fingerprinted_path() {
        for logical in ["application.js", "application.css", "favicon/favicon-96x96.png", "favicon/favicon.svg", "favicon/favicon.ico",
                        "favicon/apple-touch-icon.png", "favicon/site.webmanifest", "flags/eu.svg"] {
            let path = assets::path(logical);
            let file = assets::find(path).unwrap_or_else(|| panic!("{logical} -> {path} is not embedded"));
            let (stem, extension) = logical.rsplit_once('.').unwrap();
            assert!(path.starts_with(&format!("/assets/{stem}-")) && path.ends_with(&format!(".{extension}")), "{path}");
            assert_eq!(path.len(), "/assets/".len() + logical.len() + 17, "a 16-hex-digit fingerprint: {path}");
            assert!(!file.body.is_empty());
        }
        assert_eq!(assets::path("no/such.png"), "/assets/missing");
    }

    #[test]
    fn the_web_manifest_points_at_its_fingerprinted_icons() {
        let manifest = std::str::from_utf8(assets::find(assets::path("favicon/site.webmanifest")).unwrap().body).unwrap();
        assert!(!manifest.contains("<%"), "{manifest}");
        for icon in ["favicon/web-app-manifest-192x192.png", "favicon/web-app-manifest-512x512.png"] {
            assert!(manifest.contains(&format!("\"src\": \"{}\"", assets::path(icon))), "{icon} in {manifest}");
        }
    }

    #[test]
    fn public_files_keep_their_own_paths_and_types() {
        for (path, content_type) in [("/fonts/Dosis-digits.woff2", "font/woff2"), ("/service-worker.js", "text/javascript; charset=utf-8"),
                                     ("/500.html", "text/html; charset=utf-8"), ("/robots.txt", "text/plain; charset=utf-8"), ("/icon.png", "image/png")] {
            assert_eq!(assets::find(path).unwrap_or_else(|| panic!("{path} is not embedded")).content_type, content_type);
        }
        assert!(assets::find("/assets/application.js").is_none(), "only the fingerprinted path is served");
        let response = assets::respond(assets::find("/robots.txt").unwrap(), false);
        assert_eq!(response.headers()["cache-control"], "public, max-age=31536000");
    }
}

mod locales {
    use super::common;
    use deltabadger::web::{locale, normalize_path};

    #[test]
    fn the_locales_are_rails_available_locales() {
        let recorded = &common::vectors()["i18n"];
        assert_eq!(serde_json::json!(locale::LOCALES), recorded["locales"]);
        assert_eq!(locale::DEFAULT, recorded["default"].as_str().unwrap());
    }

    #[test]
    fn a_locale_prefix_is_split_off_only_for_known_locales_and_scoped_routes() {
        assert_eq!(locale::split("/de/login"), (Some("de"), "/login"));
        assert_eq!(locale::split("/en/bots/12"), (Some("en"), "/bots/12"));
        assert_eq!(locale::split("/de"), (Some("de"), "/"));
        assert_eq!(locale::split("/login"), (None, "/login"));
        assert_eq!(locale::split("/zz/login"), (None, "/zz/login"));
        assert_eq!(locale::split("/de/up"), (None, "/de/up"));
        assert_eq!(locale::split("/de/cable"), (None, "/de/cable"));
        assert_eq!(locale::split("/"), (None, "/"));
    }

    #[test]
    fn switch_locale_prefers_the_parameter_then_the_user_and_ignores_unknown_values() {
        assert_eq!(locale::switch(Some("de"), Some("pl")), "de");
        assert_eq!(locale::switch(None, Some("pl")), "pl");
        assert_eq!(locale::switch(Some(""), Some("pl")), "pl");
        assert_eq!(locale::switch(Some("zz"), Some("pl")), "en", "an invalid parameter is not replaced by the user's locale");
        assert_eq!(locale::switch(None, Some("zz")), "en");
        assert_eq!(locale::switch(None, None), "en");
    }

    #[test]
    fn generated_paths_are_prefixed_for_every_locale_but_the_default() {
        assert_eq!(locale::path("en", "/bots"), "/bots");
        assert_eq!(locale::path("de", "/bots"), "/de/bots");
        assert_eq!(locale::path("de", "/"), "/de");
        assert_eq!(locale::path("en", "/"), "/");
        assert_eq!(locale::path("de", "/bots/new?auto_open=true"), "/de/bots/new?auto_open=true");
    }

    #[test]
    fn the_language_switch_keeps_the_page_and_its_query_without_reserved_keys() {
        let query = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        assert_eq!(locale::switch_path("de", "/login", &[]), "/de/login");
        assert_eq!(locale::switch_path("en", "/login", &[]), "/en/login", "English is explicit here, unlike a generated path");
        assert_eq!(locale::switch_path("de", "/", &[]), "/de");
        // Recorded from Rails by the login_page_query scenario of tests/pages.rs.
        let given = query(&[("x", "1"), ("host", "evil.example"), ("locale", "de"), ("b", "two words"), ("a[]", "1")]);
        assert_eq!(locale::switch_path("nl", "/login", &given), "/nl/login?a%5B%5D=1&b=two+words&x=1");
        // Recorded by the login_page_repeated_query_keys scenario: the last value of a repeated key, every value of a list.
        let repeated = query(&[("x", "1"), ("user[email]", "first"), ("x", "2"), ("user[email]", "last"), ("a[]", "1"), ("a[]", "2")]);
        assert_eq!(locale::switch_path("en", "/login", &repeated), "/en/login?a%5B%5D=1&a%5B%5D=2&user%5Bemail%5D=last&x=2");
    }

    #[test]
    fn paths_are_normalised_as_rails_routes_them() {
        for pair in common::vectors()["rack_attack"]["normalize"].as_array().unwrap() {
            assert_eq!(normalize_path(pair[0].as_str().unwrap()), pair[1].as_str().unwrap());
        }
    }
}

mod form_fields {
    use axum::http::Method;
    use deltabadger::web::{method_override, Params};

    fn fields(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// Rack keeps the last value of a repeated key. Rails' `check_box` depends on it: a hidden "0",
    /// then the checkbox's "1" under the same name.
    #[test]
    fn a_repeated_form_field_reads_its_last_value_as_rack_does() {
        let form = fields(&[("user[remember_me]", "0"), ("a", "1"), ("user[remember_me]", "1")]);
        let params = Params { full_path: "/login".into(), fullpath: "/login".into(), route_path: "/login".into(), path_locale: None, query: Vec::new(), form };
        assert_eq!(params.form("user[remember_me]"), Some("1"));
        assert_eq!(params.form("a"), Some("1"));
        assert_eq!(params.form("b"), None);
    }

    /// The query string too: `/login?locale=de&locale=pl` is Polish (Rails' answer is the
    /// login_page_repeated_query_keys scenario of tests/pages.rs).
    #[test]
    fn a_repeated_query_key_reads_its_last_value_as_rack_does() {
        let query = fields(&[("locale", "de"), ("x", "1"), ("locale", "pl")]);
        let params = Params { full_path: "/login".into(), fullpath: "/login".into(), route_path: "/login".into(), path_locale: None, query, form: Vec::new() };
        assert_eq!(params.query("locale"), Some("pl"));
        assert_eq!(params.locale(), Some("pl"));
        assert_eq!(params.query("x"), Some("1"));
        assert_eq!(params.query("y"), None);
    }

    #[test]
    fn a_repeated_method_field_uses_its_last_value() {
        assert_eq!(method_override(&fields(&[("_method", "delete"), ("_method", "patch")])), Some(Method::PATCH));
        assert_eq!(method_override(&fields(&[("_method", "delete"), ("_method", "get")])), None, "the last one decides, also when it asks for nothing");
        assert_eq!(method_override(&fields(&[("_method", "Put")])), Some(Method::PUT));
        assert_eq!(method_override(&fields(&[("a", "1")])), None);
    }
}

mod time_zones {
    use super::common::web::at;
    use deltabadger::web::timezone;

    #[test]
    fn every_rails_time_zone_name_maps_to_a_zone_chrono_tz_knows() {
        let table: serde_json::Map<String, serde_json::Value> = serde_json::from_str(include_str!("../src/web/time_zones.json")).unwrap();
        assert!(table.len() >= 150, "{} names", table.len());
        for (name, iana) in &table {
            assert_eq!(timezone::zone(name).map(|z| z.name()), iana.as_str(), "{name}");
        }
        assert_eq!(timezone::zone("Europe/Warsaw"), None, "the column holds Rails names, not IANA ids");
        assert_eq!(timezone::local(at("2026-07-01T12:00:00Z"), "Warsaw").to_rfc3339(), "2026-07-01T14:00:00+02:00");
        assert_eq!(timezone::local(at("2026-01-01T12:00:00Z"), "Eastern Time (US & Canada)").to_rfc3339(), "2026-01-01T07:00:00-05:00");
        assert_eq!(timezone::local(at("2026-01-01T12:00:00Z"), "Nowhere").to_rfc3339(), "2026-01-01T12:00:00+00:00");
    }
}

mod sessions {
    use super::common;
    use super::common::web::{at, header_map};
    use deltabadger::web::session::{self, Pending, SessionData};
    use deltabadger::web::Config;

    fn full_session() -> SessionData {
        SessionData {
            user: Some((7, "$2a$11$abcdefghijklmnopqrstuv".into())), csrf: Some("c3Jm".into()),
            flash: vec![("alert".into(), "Zażółć \"it\"".into()), ("notice".into(), "ok".into())],
            pending: Some(Pending { user_id: 7, started_at: 1_789_041_630 }), return_to: Some("/bots?filter=active".into()), auto_open_bot_wizard: true,
        }
    }

    #[test]
    fn a_session_survives_the_cookie_and_nothing_else_opens_it() {
        let (key, now) = ([3u8; 32], at("2026-09-10T12:00:30Z"));
        let data = full_session();
        let cookie = session::seal(&key, &data, now);
        assert_eq!(session::open(&key, &cookie, now), Some(data.clone()));
        assert_ne!(session::seal(&key, &data, now), cookie, "a fresh nonce every time");
        assert!(!cookie.contains("bots") && cookie.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'), "opaque and cookie-safe");
        assert_eq!(session::open(&[4u8; 32], &cookie, now), None, "another key");
        let mut tampered = cookie.clone().into_bytes();
        tampered[20] = if tampered[20] == b'A' { b'B' } else { b'A' };
        assert_eq!(session::open(&key, std::str::from_utf8(&tampered).unwrap(), now), None, "one changed character");
        for junk in ["", "abc", "!!!", &cookie[..30]] { assert_eq!(session::open(&key, junk, now), None, "{junk:?}"); }
    }

    #[test]
    fn a_session_expires_thirty_days_after_it_was_last_written() {
        let (key, now) = ([3u8; 32], at("2026-09-10T12:00:30Z"));
        assert_eq!(session::LIFETIME_SECONDS, common::vectors()["devise"]["session_expire_after"].as_i64().unwrap());
        let cookie = session::seal(&key, &full_session(), now);
        assert!(session::open(&key, &cookie, at("2026-10-10T12:00:29Z")).is_some());
        assert_eq!(session::open(&key, &cookie, at("2026-10-10T12:00:30Z")), None, "the expiry is enforced here, not left to the browser");
    }

    #[test]
    fn a_cookie_carries_its_own_expiry_and_reading_it_does_not_move_it() {
        let (key, now) = ([3u8; 32], at("2026-09-10T12:00:30Z"));
        let cookie = session::seal(&key, &full_session(), now);
        let opened = session::read(&key, &cookie, now).unwrap();
        assert_eq!((opened.expires_at, opened.data), (now.timestamp() + session::LIFETIME_SECONDS, full_session()));
        assert_eq!(session::read(&key, &cookie, at("2026-10-09T12:00:30Z")).unwrap().expires_at, opened.expires_at, "29 days of use later: the same end");
    }

    #[test]
    fn the_cookie_header_is_rails_session_cookie_under_our_name() {
        let now = at("2026-09-10T12:00:30.123456Z");
        assert_eq!(session::set_cookie("v", now, false), "_deltabadger_rust_session=v; path=/; expires=Sat, 10 Oct 2026 12:00:30 GMT; httponly; samesite=lax");
        assert_eq!(session::set_cookie("v", now, true), "_deltabadger_rust_session=v; path=/; expires=Sat, 10 Oct 2026 12:00:30 GMT; secure; httponly; samesite=lax");
        let headers = header_map(&[("cookie", "other=1; _deltabadger_rust_session=abc-_; _deltabadger_session=rails")]);
        assert_eq!(session::cookie_values(&headers).collect::<Vec<_>>(), ["abc-_"]);
        assert_eq!(session::cookie_values(&header_map(&[("cookie", "_deltabadger_session=rails")])).next(), None, "Rails' cookie is not ours");
        let several = header_map(&[("cookie", "_deltabadger_rust_session=a; _deltabadger_rust_session_x=b; x_deltabadger_rust_session=c; _deltabadger_rust_session=d")]);
        assert_eq!(session::cookie_values(&several).collect::<Vec<_>>(), ["a", "d"], "every cookie of our exact name, in order; a longer name is another cookie");
    }

    #[test]
    fn secure_cookies_and_the_proxy_rule_follow_rails_env_resolution() {
        let config = |pairs: &'static [(&'static str, &'static str)]| {
            Config::from_env(&move |name| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string()).or((name == "SECRET_KEY_BASE").then(|| "s".to_string()))).unwrap()
        };
        assert!(!config(&[]).force_ssl && !config(&[]).behind_proxy);
        assert!(config(&[("APP_ROOT_URL", "https://x.example")]).force_ssl && config(&[("APP_ROOT_URL", "https://x.example")]).behind_proxy);
        assert!(!config(&[("APP_ROOT_URL", "https://x.example"), ("FORCE_SSL", "off")]).force_ssl);
        assert!(config(&[("FORCE_SSL", " Yes ")]).force_ssl);
        assert!(config(&[("APP_ROOT_URL", "https://x.example"), ("FORCE_SSL", "maybe")]).force_ssl, "an unrecognised spelling is no answer");
        assert!(config(&[("BEHIND_PROXY", "1")]).behind_proxy && !config(&[("BEHIND_PROXY", "1")]).force_ssl);
        assert!(!config(&[("APP_ROOT_URL", "https://x.example"), ("BEHIND_PROXY", "false")]).behind_proxy);
        assert!(Config::from_env(&|_| None).is_err(), "SECRET_KEY_BASE is required");
    }
}

mod csrf_tokens {
    use super::common;
    use super::common::web::header_map;
    use axum::http::{HeaderMap, HeaderName};
    use deltabadger::web::{csrf, Config};

    #[test]
    fn a_masked_token_verifies_and_is_different_every_time() {
        let token = csrf::new_token();
        let (first, second) = (csrf::masked(&token), csrf::masked(&token));
        assert_ne!(first, second);
        assert_eq!(first.len(), 86, "64 bytes, base64url without padding, as Rails' tokens");
        assert!(csrf::valid(&token, &first) && csrf::valid(&token, &second));
        assert!(!csrf::valid(&csrf::new_token(), &first), "another session's token");
        assert!(!csrf::valid(&token, &token), "the raw token is not accepted");
        for junk in ["", "abc", "%%%"] { assert!(!csrf::valid(&token, junk)); }
    }

    #[test]
    fn either_the_form_token_or_the_header_token_is_enough() {
        let token = csrf::new_token();
        let masked = csrf::masked(&token);
        let ours = Some("http://localhost:3000");
        let with_header = |value: &str| header_map(&[("x-csrf-token", value)]);
        assert!(csrf::verified(Some(&token), &HeaderMap::new(), Some(&masked), ours));
        assert!(csrf::verified(Some(&token), &with_header(&masked), None, ours));
        assert!(csrf::verified(Some(&token), &with_header(&masked), Some("not-a-token"), ours), "an invalid field beside a valid header, as Rails");
        assert!(csrf::verified(Some(&token), &with_header("not-a-token"), Some(&masked), ours));
        assert!(!csrf::verified(Some(&token), &with_header("not-a-token"), Some("neither"), ours));
        assert!(!csrf::verified(Some(&token), &HeaderMap::new(), None, ours));
        assert!(!csrf::verified(None, &HeaderMap::new(), Some(&masked), ours), "a session with no token verifies nothing");
    }

    #[test]
    fn an_origin_header_must_name_this_deployments_origin_exactly() {
        let token = csrf::new_token();
        let masked = csrf::masked(&token);
        let check = |origin: &str, expected: Option<&str>| csrf::verified(Some(&token), &header_map(&[("origin", origin)]), Some(&masked), expected);
        assert!(check("http://localhost:3000", Some("http://localhost:3000")));
        for foreign in ["https://localhost:3000", "http://localhost:3001", "http://localhost", "http://evil.example", "null", "", "localhost:3000"] {
            assert!(!check(foreign, Some("http://localhost:3000")), "{foreign:?} is another origin: scheme, host and port all count");
        }
        assert!(!check("http://localhost:3000", None), "with no origin of our own to compare, a stated origin cannot match");
        let mut not_text = HeaderMap::new();
        not_text.insert("origin", axum::http::HeaderValue::from_bytes(b"http://localhost:3000\xff").unwrap());
        assert!(!csrf::verified(Some(&token), &not_text, Some(&masked), Some("http://localhost:3000")), "a malformed header is a mismatch, not an absent header");
    }

    #[test]
    fn the_deployments_origin_is_app_root_url_or_else_the_requests() {
        let config = |pairs: &'static [(&'static str, &'static str)]| {
            Config::from_env(&move |name| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string()).or((name == "SECRET_KEY_BASE").then(|| "s".to_string()))).unwrap()
        };
        let host = header_map(&[("host", "bot.example.com:8080")]);
        assert_eq!(config(&[]).origin(&host).as_deref(), Some("http://bot.example.com:8080"));
        assert_eq!(config(&[("FORCE_SSL", "1")]).origin(&host).as_deref(), Some("https://bot.example.com:8080"));
        assert_eq!(config(&[("APP_ROOT_URL", "https://my.example.org/")]).origin(&host).as_deref(), Some("https://my.example.org"));
        assert_eq!(config(&[("APP_ROOT_URL", "http://127.0.0.1:3000/some/path")]).origin(&host).as_deref(), Some("http://127.0.0.1:3000"));
        assert_eq!(config(&[]).origin(&HeaderMap::new()), None);

        // Canonical, as a browser writes an Origin: lower case, and no port that is the scheme's default.
        let of = |pairs, host_header: &str| config(pairs).origin(&header_map(&[("host", host_header)]));
        assert_eq!(of(&[("APP_ROOT_URL", "https://Bot.Example.com:443/")], "x").as_deref(), Some("https://bot.example.com"));
        assert_eq!(of(&[("APP_ROOT_URL", "HTTP://bot.example.com:80")], "x").as_deref(), Some("http://bot.example.com"));
        assert_eq!(of(&[("APP_ROOT_URL", "https://bot.example.com:8443/x?y")], "x").as_deref(), Some("https://bot.example.com:8443"));
        assert_eq!(of(&[("APP_ROOT_URL", "https://bot.example.com:80")], "x").as_deref(), Some("https://bot.example.com:80"), "80 is not https' default");
        assert_eq!(of(&[("FORCE_SSL", "1")], "BOT.Example.com:443").as_deref(), Some("https://bot.example.com"));
        assert_eq!(of(&[], "bot.example.com:80").as_deref(), Some("http://bot.example.com"));
        assert_eq!(of(&[], "[::1]:80").as_deref(), Some("http://[::1]"));
        assert!(config(&[("APP_ROOT_URL", "HTTPS://bot.example.com")]).force_ssl);
        // What that buys: a deployment configured with the default port spelled out accepts its own pages.
        let configured = config(&[("APP_ROOT_URL", "https://Bot.Example.com:443/")]);
        let from_the_browser = header_map(&[("host", "10.0.0.7:3000"), ("origin", "https://bot.example.com")]);
        assert!(csrf::same_origin(&from_the_browser, configured.origin(&from_the_browser).as_deref()));
    }

    /// Without APP_ROOT_URL the deployment's origin is the request's own, and that is Rails'
    /// `request.base_url` as production computes it: Puma's env, AssumeSSL when SSL is on, then Rack
    /// and Action Dispatch, which follow the headers a proxy in front writes (the scheme it
    /// terminated, the host it was asked for). Recorded by script/rust/record_vectors.rb.
    #[test]
    fn without_app_root_url_the_origin_is_rails_base_url_with_what_a_proxy_forwarded() {
        let config = |pairs: &'static [(&'static str, &'static str)]| {
            Config::from_env(&move |name| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string()).or((name == "SECRET_KEY_BASE").then(|| "s".to_string()))).unwrap()
        };
        let recorded = common::vectors()["base_url"].as_array().unwrap().clone();
        assert!(recorded.len() >= 100 && recorded.iter().any(|case| case["ssl"] == true) && recorded.iter().any(|case| case["ssl"] == false), "{} vectors", recorded.len());
        let request = |case: &serde_json::Value| {
            let mut headers = header_map(&[("host", case["host"].as_str().unwrap())]);
            for line in case["headers"].as_array().unwrap() {
                let (name, value) = line.as_str().unwrap().split_once(':').unwrap();
                headers.append(HeaderName::from_bytes(name.as_bytes()).unwrap(), value.trim().parse().unwrap());
            }
            headers
        };
        let (plain, forced) = (config(&[]), config(&[("FORCE_SSL", "true")]));
        let configured = config(&[("APP_ROOT_URL", "http://my.example.org")]);
        for case in &recorded {
            let headers = request(case);
            let ours = if case["ssl"] == true { &forced } else { &plain };
            assert_eq!(ours.origin(&headers).as_deref(), case["base_url"].as_str(), "{case}");
            assert_eq!(configured.origin(&headers).as_deref(), Some("http://my.example.org"), "APP_ROOT_URL wins over anything forwarded: {case}");
        }
    }
}

mod flash_messages {
    use deltabadger::web::{flash, session};

    #[test]
    fn a_flash_is_shown_once_and_flash_now_joins_it() {
        let session = session::Session::default();
        flash::set(&session, flash::NOTICE, "first".into());
        flash::set(&session, flash::ALERT, "second <b>".into());
        assert_eq!(session.lock().flash, vec![("alert".to_string(), "second <b>".to_string())], "a new flash replaces what was waiting");
        let shown = flash::take(&session, &[("alert", "now".into()), ("success", "done".into())]);
        assert_eq!(shown.iter().map(|m| (m.style, m.text.as_str())).collect::<Vec<_>>(), vec![("danger", "now"), ("success", "done")]);
        assert!(session.lock().flash.is_empty() && flash::take(&session, &[]).is_empty());
        let markup = flash::render(&[flash::Message { style: "primary", text: "a <b> & c".into() }]).unwrap();
        assert!(markup.contains("salert--primary") && markup.contains("a &#60;b&#62; &#38; c"), "{markup}");
    }
}

mod response_headers {
    use super::common::web::header_map;
    use axum::http::{HeaderMap, StatusCode};
    use deltabadger::web::headers;

    #[test]
    fn the_policy_is_the_one_rails_test_pins() {
        // test/integration/content_security_policy_test.rb `expected_policy`.
        assert_eq!(headers::content_security_policy("N0nce=="),
                   "default-src 'self'; font-src 'self' data:; img-src 'self' data: https:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'; \
                    script-src 'self' 'nonce-N0nce=='; style-src 'self' 'unsafe-inline'; connect-src 'self' ipc: http://ipc.localhost; report-uri /csp-report");
        let (a, b) = (headers::new_nonce(), headers::new_nonce());
        assert!(a != b && a.len() == 24, "16 random bytes in base64: {a}");
    }

    #[test]
    fn cache_control_follows_rails_and_never_overrides_a_handler() {
        let value = |status, signed_in| { let mut h = HeaderMap::new(); headers::controller_defaults(&mut h, status, signed_in); h["cache-control"].to_str().unwrap().to_string() };
        assert_eq!(value(StatusCode::OK, false), "max-age=0, private, must-revalidate");
        assert_eq!(value(StatusCode::FOUND, false), "no-cache");
        assert_eq!(value(StatusCode::UNPROCESSABLE_ENTITY, false), "no-cache");
        assert_eq!(value(StatusCode::OK, true), "no-store");
        assert_eq!(value(StatusCode::SEE_OTHER, true), "no-store");
        let mut set = header_map(&[("cache-control", "no-cache")]);
        headers::controller_defaults(&mut set, StatusCode::FOUND, true);
        assert_eq!(set["cache-control"], "no-cache");
        assert_eq!(set["x-frame-options"], "SAMEORIGIN");
        assert_eq!(set["referrer-policy"], "strict-origin-when-cross-origin");
        let mut policy = HeaderMap::new();
        headers::policy(&mut policy, "n", true);
        assert_eq!(policy["strict-transport-security"], "max-age=63072000; includeSubDomains");
        headers::policy(&mut HeaderMap::new(), "n", false);
    }
}

mod turbo_streams {
    use super::common;
    use super::common::web::header_map;
    use axum::http::HeaderMap;
    use deltabadger::web::turbo;

    #[test]
    fn stream_elements_are_turbo_rails_markup() {
        let recorded = &common::vectors()["turbo"];
        let want = |name: &str| recorded[name].as_str().unwrap().to_string();
        assert_eq!(turbo::stream("replace", "bot_1", "<p>a &amp; b</p>"), want("replace"));
        assert_eq!(turbo::stream("update", "bot_1", "<p>x</p>"), want("update"));
        assert_eq!(turbo::stream("append", "orders", "<tr></tr>"), want("append"));
        assert_eq!(turbo::prepend_flash("<div>hi</div>"), want("prepend"));
        assert_eq!(turbo::remove("bot_1"), want("remove"));
        assert_eq!(turbo::refresh(), want("refresh"));
        assert_eq!(turbo::redirect("/de/bots?a=1&b=2"), want("redirect"));
        assert_eq!(turbo::add_class("columns_bot_1", "bot-locked"), want("add_class"));
        assert_eq!(turbo::remove_class("columns_bot_1", "bot-locked"), want("remove_class"));
        assert_eq!(turbo::CONTENT_TYPE, format!("{}; charset=utf-8", want("content_type")));
        assert_eq!(turbo::frame(&header_map(&[("turbo-frame", "modal")])), Some("modal"));
        assert_eq!(turbo::frame(&HeaderMap::new()), None);
    }
}

mod going_back {
    use super::common::web::header_map;
    use deltabadger::web::{layout, Config};

    #[test]
    fn a_referer_is_followed_only_within_this_deployments_origin_and_never_to_another_host() {
        let config = |root: Option<&'static str>| {
            Config::from_env(&move |name| match name { "SECRET_KEY_BASE" => Some("s".into()), "APP_ROOT_URL" => root.map(String::from), _ => None }).unwrap()
        };
        let back = |config: &Config, referer: &str| layout::back(config, &header_map(&[("host", "bot.example"), ("referer", referer)]));
        let https = config(Some("https://bot.example"));
        assert_eq!(back(&https, "https://bot.example/login?x=1").as_deref(), Some("/login?x=1"));
        assert_eq!(back(&https, "https://bot.example").as_deref(), Some("/"));
        assert_eq!(back(&https, "https://bot.example/login?next=//x").as_deref(), Some("/login?next=//x"), "a query is not a path");
        assert_eq!(back(&https, "https://bot.example//evil.test/path").as_deref(), Some("/evil.test/path"), "never `//evil.test/path`, which names another host");
        assert_eq!(back(&https, "https://bot.example///evil.test").as_deref(), Some("/evil.test"));
        for foreign in [
            "http://bot.example/login", "https://bot.example.evil.test/", "https://bot.example@evil.test/", "https://bot.example:8443/",
            "https://evil.test/https://bot.example/", "https://bot.example/\\evil.test", "//bot.example/login", "/login", "", "javascript:alert(1)",
        ] {
            assert_eq!(back(&https, foreign), None, "{foreign}");
        }
        // Without APP_ROOT_URL the origin is the request's own: its Host, and http unless SSL is forced or a proxy forwarded another scheme.
        let plain = config(None);
        assert_eq!(back(&plain, "http://bot.example/bots").as_deref(), Some("/bots"));
        assert_eq!(back(&plain, "https://bot.example/bots"), None);
    }
}

mod sign_in_limits {
    use super::common;
    use deltabadger::web::auth;

    #[test]
    fn the_lock_is_devises() {
        let recorded = &common::vectors()["devise"];
        assert_eq!(auth::MAXIMUM_ATTEMPTS, recorded["maximum_attempts"].as_i64().unwrap());
        assert_eq!(auth::UNLOCK_IN_SECONDS, recorded["unlock_in"].as_i64().unwrap());
        assert_eq!(auth::PENDING_TTL_SECONDS, recorded["pending_ttl"].as_i64().unwrap());
    }
}

mod rate_limits {
    use super::common;
    use super::common::web::{at, header_map};
    use axum::http::{Method, StatusCode};
    use deltabadger::web::{rate_limit, Config};

    #[test]
    fn the_limits_are_rack_attacks() {
        let recorded = &common::vectors()["rack_attack"];
        for (rule, _, limit) in rate_limit::RULES {
            assert_eq!(recorded["throttles"][rule], serde_json::json!({ "limit": limit, "period": 60 }), "{rule}");
        }
        let response = rate_limit::throttled(17);
        assert_eq!((response.status(), response.headers()["retry-after"].to_str().unwrap(), response.headers()["content-type"].to_str().unwrap()),
                   (StatusCode::TOO_MANY_REQUESTS, "17", "text/plain; charset=utf-8"));
    }

    #[test]
    fn a_window_is_a_clock_minute_per_address_and_rule() {
        let limiter = rate_limit::Limiter::default();
        let now = at("2026-09-10T12:00:30Z");
        for _ in 0..10 { assert_eq!(limiter.hit(&Method::POST, "/login", "1.1.1.1", now), None); }
        assert_eq!(limiter.hit(&Method::POST, "/login", "1.1.1.1", now), Some(30), "the 11th in the window; the window ends in 30 s");
        assert_eq!(limiter.hit(&Method::POST, "/login", "1.1.1.1", at("2026-09-10T12:00:59Z")), Some(1), "refused requests count too");
        assert_eq!(limiter.hit(&Method::POST, "/login", "2.2.2.2", now), None, "another address");
        assert_eq!(limiter.hit(&Method::GET, "/login", "1.1.1.1", now), None, "only POST is limited");
        assert_eq!(limiter.hit(&Method::DELETE, "/login", "3.3.3.3", now), None, "a form POST that _method made a DELETE is not a POST to rack-attack");
        assert_eq!(limiter.hit(&Method::POST, "/bots", "1.1.1.1", now), None, "no rule");
        assert_eq!(limiter.hit(&Method::POST, "/login", "1.1.1.1", at("2026-09-10T12:01:00Z")), None, "the next clock minute starts at zero");
        for _ in 0..5 { assert_eq!(limiter.hit(&Method::POST, "/verify_two_factor", "1.1.1.1", now), None); }
        assert_eq!(limiter.hit(&Method::POST, "/verify_two_factor", "1.1.1.1", now), Some(30));
    }

    #[test]
    fn headers_name_the_client_only_behind_a_declared_proxy_and_only_when_a_proxy_sent_them() {
        // Behind a trusted proxy the answer is Rails', except for these X-Forwarded-For values, where
        // Rails can be told an address and this crate cannot: (the header, this crate's answer).
        // - an entry nobody can read: Rails skips it and believes the entry before it, which only the
        //   caller wrote; here the walk ends at the last trusted hop (10.0.0.9, or the peer 10.0.0.5);
        // - an IPv4-mapped address: Rails' trusted ranges do not cover it and it prints it as IPv6;
        //   here it is the IPv4 address it carries.
        const STRICTER: [(&str, &str); 7] = [
            ("198.51.100.7, unknown, , 10.0.0.9", "10.0.0.9"),
            ("198.51.100.99, garbage, 10.0.0.9", "10.0.0.9"),
            ("198.51.100.99, 203.0.113.7:notaport", "10.0.0.5"),
            ("198.51.100.99, 203.0.113.7/32", "10.0.0.5"),
            ("198.51.100.99, ::ffff:10.0.0.9", "198.51.100.99"),
            ("198.51.100.99, ::ffff:203.0.113.7", "203.0.113.7"),
            ("198.51.100.99, [::ffff:203.0.113.7]:443", "203.0.113.7"),
        ];
        let (mut rails_believed_a_forged_header, mut stricter) = (0, 0);
        for case in common::vectors()["remote_ip"].as_array().unwrap() {
            let peer = case["remote_addr"].as_str().unwrap();
            let got = rate_limit::remote_ip(peer.parse().ok(), case["forwarded_for"].as_str(), case["client_ip"].as_str()).map(|ip| ip.to_string());
            let listed = STRICTER.iter().find(|(header, _)| case["forwarded_for"] == *header);
            if case["peer_trusted"] != true {
                assert_eq!(got.as_deref(), Some(peer), "a peer that is no trusted proxy is the client, whatever it sends: {case}");
                rails_believed_a_forged_header += usize::from(case["ip"] != peer);
            } else if let Some((_, ours)) = listed {
                assert_eq!(got.as_deref(), Some(*ours), "{case}");
                assert_ne!(case["ip"], *ours, "Rails answers the same now: take this one off the list. {case}");
                stricter += 1;
            } else {
                assert_eq!(got.as_deref(), case["ip"].as_str(), "behind a trusted proxy the answer is Rails': {case}");
            }
        }
        assert_eq!(rails_believed_a_forged_header, 4, "the vectors where Rails takes the caller's own header for its address");
        assert_eq!(stricter, STRICTER.len(), "every listed difference has its vector");
        let config = |behind: &'static str| Config::from_env(&move |name| match name { "SECRET_KEY_BASE" => Some("s".into()), "BEHIND_PROXY" => Some(behind.into()), _ => None }).unwrap();
        let forwarded = header_map(&[("x-forwarded-for", "198.51.100.7")]);
        let peer = Some("10.0.0.5".parse().unwrap());
        assert_eq!(rate_limit::client_key(&config("false"), &forwarded, peer), "10.0.0.5", "the header is only what the caller typed");
        assert_eq!(rate_limit::client_key(&config("true"), &forwarded, peer), "198.51.100.7");
        assert_eq!(rate_limit::client_key(&config("true"), &forwarded, Some("203.0.113.9".parse().unwrap())), "203.0.113.9", "a direct public peer cannot rename itself");
        assert_eq!(rate_limit::client_key(&config("true"), &forwarded, Some("::ffff:10.0.0.5".parse().unwrap())), "198.51.100.7", "a private IPv4 proxy on a dual-stack socket");
        assert_eq!(rate_limit::client_key(&config("false"), &forwarded, None), "unattributed", "never an empty key");
    }
}

mod navbar_numbers {
    use super::common;
    use deltabadger::web::{bots, shell};

    #[test]
    fn the_bot_count_icon_is_sized_as_rails_sizes_it() {
        for case in common::vectors()["navbar"]["bot_count"].as_array().unwrap() {
            let size = shell::bot_count_font_size(case["count"].as_i64().unwrap());
            assert_eq!((size.to_string().as_str(), shell::bot_count_baseline(size).as_str()), (case["font_size"].as_str().unwrap(), case["baseline"].as_str().unwrap()), "{case}");
        }
    }

    #[test]
    fn the_tracker_icon_is_a_ring_exactly_when_rails_draws_one() {
        let recorded = &common::vectors()["tracker"];
        assert_eq!(serde_json::json!(bots::CASH), recorded["cash"], "Tracker::UnfundedCash's FIAT and STABLECOINS");
        for case in recorded["show_cash"].as_array().unwrap() {
            assert_eq!(bots::show_cash(case["column"].as_str()), case["shown"], "{case}");
        }
        let held = |symbols: &[&str]| symbols.iter().map(|s| Some(s.to_string())).collect::<Vec<_>>();
        assert!(!bots::tracker_ring(&[], true), "nothing priced: the plain circle");
        assert!(!bots::tracker_ring(&held(&["USD", "USDC"]), false), "cash only, cash not shown: the plain circle");
        assert!(bots::tracker_ring(&held(&["USD", "USDC"]), true), "cash is drawn once the tracker shows it");
        assert!(bots::tracker_ring(&held(&["USD", "BTC"]), false));
        assert!(bots::tracker_ring(&held(&["usd"]), false), "the comparison is exact, as in Ruby");
        assert!(bots::tracker_ring(&[None], false), "an asset without a symbol is not cash");
    }
}
