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
    }

    #[test]
    fn paths_are_normalised_as_rails_routes_them() {
        for pair in common::vectors()["rack_attack"]["normalize"].as_array().unwrap() {
            assert_eq!(normalize_path(pair[0].as_str().unwrap()), pair[1].as_str().unwrap());
        }
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
        assert_eq!(session::cookie_value(&headers).as_deref(), Some("abc-_"));
        assert_eq!(session::cookie_value(&header_map(&[("cookie", "_deltabadger_session=rails")])), None, "Rails' cookie is not ours");
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
