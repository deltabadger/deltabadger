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
