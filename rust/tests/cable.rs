//! /cable driven the way @rails/actioncable and turbo-rails' `<turbo-cable-stream-source>` drive it
//! (node_modules/@rails/actioncable/src/connection.js, subscriptions.js, connection_monitor.js).
mod common;
use common::web::{self, TestClock};
use deltabadger::web::session::{self, SessionData};
use deltabadger::web::{cable, router, App, Config};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Every user's `encrypted_password` here. Nothing verifies it; its first 29 characters are the
/// salt a session must carry.
const HASH: &str = "$2a$04$abcdefghijklmnopqrstuuKq8n2RkM1bXh0Zc3TtYw5LpJv7dEoGi";
/// What it becomes when a test changes a password: another salt.
const NEW_HASH: &str = "$2a$04$ZYXWVUTSRQPONMLKJIHGFuKq8n2RkM1bXh0Zc3TtYw5LpJv7dEoGi";
const OWNER: i64 = 1;
const SECOND: i64 = 2;

/// The session cookie of a browser signed in as `user_id` while the password was `hash`.
fn signed_in_with(app: &App, user_id: i64, hash: &str) -> String {
    session::seal(&app.keys.session, &SessionData { user: Some((user_id, hash[..29].to_string())), ..SessionData::default() }, app.now())
}

fn signed_in(app: &App, user_id: i64) -> String {
    signed_in_with(app, user_id, HASH)
}

/// As `served_with`, asking each connection every 100 ms whether its session is still good.
async fn served() -> (tempfile::TempDir, App, SocketAddr) {
    let (dir, app, address, _clock) = served_with(Duration::from_millis(100)).await;
    (dir, app, address)
}

/// An install with two confirmed users, OWNER and SECOND, served on a local port. `recheck` is how
/// often an open connection's session is checked again (60 seconds outside tests).
async fn served_with(recheck: Duration) -> (tempfile::TempDir, App, SocketAddr, std::sync::Arc<TestClock>) {
    let (dir, opened, seeded) = common::install();
    assert_eq!(seeded.user_id, OWNER);
    opened.primary.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00'", [HASH]).unwrap();
    opened.primary.execute("INSERT INTO users (email, encrypted_password, name, admin, confirmed_at, created_at, updated_at) \
                            VALUES ('second@example.com', ?1, 'Second', 0, ?2, ?2, ?2)", (HASH, "2026-01-01 00:00:00")).unwrap();
    assert_eq!(opened.primary.last_insert_rowid(), SECOND);
    // A deployment that names its own origin; the listener itself is 127.0.0.1:<port>.
    let env = |name: &str| match name {
        "SECRET_KEY_BASE" => Some(web::SECRET.to_string()),
        "APP_ROOT_URL" => Some("http://localhost:3000".to_string()),
        _ => None,
    };
    let clock = TestClock::at("2026-09-10T12:00:30Z");
    let app = App::new(Config::from_env(&env).unwrap(), &env, opened.primary, clock.clone()).unwrap();
    let app = app.with_cable_timing(Duration::from_millis(50), recheck).unwrap();
    OWNER_COOKIE.get_or_init(|| signed_in(&app, OWNER));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let service = router(app.clone()).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, service).await });
    (dir, app, address, clock)
}

/// As the browser's consumer opens it: both protocols on offer, the page's origin, and the
/// browser's session cookie, if it has one.
async fn open_as(address: SocketAddr, origin: &str, cookie: Option<&str>) -> Result<(Socket, Option<String>), u16> {
    let mut request = format!("ws://{address}/cable").into_client_request().unwrap();
    request.headers_mut().insert("sec-websocket-protocol", "actioncable-v1-json, actioncable-unsupported".parse().unwrap());
    request.headers_mut().insert("origin", origin.parse().unwrap());
    if let Some(cookie) = cookie {
        request.headers_mut().insert("cookie", format!("_deltabadger_rust_session={cookie}").parse().unwrap());
    }
    match tokio_tungstenite::connect_async(request).await {
        Ok((socket, response)) => Ok((socket, response.headers().get("sec-websocket-protocol").map(|v| v.to_str().unwrap().to_string()))),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => Err(response.status().as_u16()),
        Err(other) => panic!("{other:?}"),
    }
}

/// The owner's browser. Every install here has the same secret, users and clock, so one cookie serves all.
static OWNER_COOKIE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

async fn open(address: SocketAddr, origin: &str) -> Result<(Socket, Option<String>), u16> {
    open_as(address, origin, Some(OWNER_COOKIE.get().expect("served() ran first"))).await
}

async fn next(socket: &mut Socket) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(2), socket.next()).await.expect("a message within 2 s").unwrap().unwrap();
    serde_json::from_str(message.to_text().unwrap()).unwrap()
}

/// The next message that is not a ping.
async fn next_event(socket: &mut Socket) -> Value {
    loop {
        let message = next(socket).await;
        if message["type"] != "ping" { return message; }
    }
}

/// `JSON.stringify({channel, signed_stream_name})`, as turbo-rails builds the identifier.
fn identifier(app: &App, stream: &str) -> String {
    json!({ "channel": "Turbo::StreamsChannel", "signed_stream_name": cable::signed_stream_name(&app.keys.streams, stream) }).to_string()
}

async fn command(socket: &mut Socket, command: &str, identifier: &str) {
    socket.send(Message::Text(json!({ "command": command, "identifier": identifier }).to_string().into())).await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn the_handshake_is_action_cables() {
    let (_dir, app, address) = served().await;
    let (mut socket, protocol) = open(address, "http://localhost:3000").await.unwrap();
    assert_eq!(protocol.as_deref(), Some("actioncable-v1-json"), "the client disconnects for good on any other protocol");
    assert_eq!(next(&mut socket).await, json!({ "type": "welcome" }));

    let id = identifier(&app, "user_7:bot_updates");
    command(&mut socket, "subscribe", &id).await;
    assert_eq!(next_event(&mut socket).await, json!({ "identifier": id, "type": "confirm_subscription" }), "this is what sets the element's `connected` attribute");

    app.hub.broadcast("user_7:bot_updates", "<turbo-stream action=\"remove\" target=\"bot_1\"></turbo-stream>");
    app.hub.broadcast("user_8:bot_updates", "<turbo-stream action=\"remove\" target=\"other\"></turbo-stream>");
    assert_eq!(next_event(&mut socket).await, json!({ "identifier": id, "message": "<turbo-stream action=\"remove\" target=\"bot_1\"></turbo-stream>" }));

    let ping = next(&mut socket).await; // only another stream's broadcast was pending: the next message is a ping
    assert_eq!(ping, json!({ "type": "ping", "message": 1_789_041_630 }), "epoch seconds from the app's clock");
}

#[tokio::test(flavor = "current_thread")]
async fn a_name_this_process_did_not_sign_is_rejected() {
    let (_dir, _app, address) = served().await;
    let (mut socket, _) = open(address, "http://localhost:3000").await.unwrap();
    next(&mut socket).await;
    for forged in ["InVzZXJfNzpib3RfdXBkYXRlcyI=--314db8d9ed04fea11386095332ae2b145a1fbd114d2bac92453058f9098d58a4", "nonsense", ""] {
        let id = json!({ "channel": "Turbo::StreamsChannel", "signed_stream_name": forged }).to_string();
        command(&mut socket, "subscribe", &id).await;
        assert_eq!(next_event(&mut socket).await, json!({ "identifier": id, "type": "reject_subscription" }));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_repeated_subscribe_is_confirmed_once_and_unsubscribe_stops_delivery() {
    let (_dir, app, address) = served().await;
    let (mut socket, _) = open(address, "http://localhost:3000").await.unwrap();
    next(&mut socket).await;
    let id = identifier(&app, "settings_sync");
    command(&mut socket, "subscribe", &id).await;
    command(&mut socket, "subscribe", &id).await; // the client's 500 ms subscribe retry
    assert_eq!(next_event(&mut socket).await["type"], "confirm_subscription");
    app.hub.broadcast("settings_sync", "one");
    assert_eq!(next_event(&mut socket).await, json!({ "identifier": id, "message": "one" }), "no second confirmation came first");

    command(&mut socket, "unsubscribe", &id).await;
    let other = identifier(&app, "user_1:preferences");
    command(&mut socket, "subscribe", &other).await;
    assert_eq!(next_event(&mut socket).await["type"], "confirm_subscription");
    app.hub.broadcast("settings_sync", "two");
    app.hub.broadcast("user_1:preferences", "three");
    assert_eq!(next_event(&mut socket).await, json!({ "identifier": other, "message": "three" }));
}

#[tokio::test(flavor = "current_thread")]
async fn a_reconnecting_client_is_welcomed_and_confirmed_again() {
    let (_dir, app, address) = served().await;
    let id = identifier(&app, "user_7:bot_updates");
    for _ in 0..2 {
        let (mut socket, _) = open(address, "http://localhost:3000").await.unwrap();
        assert_eq!(next(&mut socket).await["type"], "welcome");
        command(&mut socket, "subscribe", &id).await; // subscriptions.reload() after every welcome
        assert_eq!(next_event(&mut socket).await["type"], "confirm_subscription");
        socket.close(None).await.unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn only_this_sites_pages_may_connect() {
    let (_dir, _app, address) = served().await;
    assert_eq!(open(address, "http://evil.example").await.err(), Some(404));
    assert!(open(address, "http://localhost:3000").await.is_ok(), "APP_ROOT_URL's origin");
    assert!(open(address, &format!("http://{address}")).await.is_ok(), "the host the request came to is allowed, as allow_same_origin_as_host");
    assert_eq!(open(address, &format!("https://{address}")).await.err(), Some(404), "the scheme is part of an origin");
    assert_eq!(open(address, "http://localhost:3000.evil.example").await.err(), Some(404));
    assert_eq!(open(address, "http://localhost:30001").await.err(), Some(404));
}

/// Whether the server ends the connection within three seconds: pings may still arrive, then a
/// close, an error or the end of the stream.
async fn closed(socket: &mut Socket) -> bool {
    let ended = async {
        while let Some(Ok(message)) = socket.next().await {
            if matches!(message, Message::Close(_)) { break; }
        }
    };
    tokio::time::timeout(Duration::from_secs(3), ended).await.is_ok()
}

#[tokio::test(flavor = "current_thread")]
async fn a_message_larger_than_any_command_ends_the_connection() {
    let (_dir, app, address) = served().await;
    // A signed-in browser needs no signed name to get this far. The server reads the length from the
    // frame header and stops there; it never holds the megabyte.
    let (mut socket, _) = open(address, "http://localhost:3000").await.unwrap();
    next(&mut socket).await;
    let _ = socket.send(Message::Text("x".repeat(1024 * 1024).into())).await; // the server may hang up mid-write
    assert!(closed(&mut socket).await, "an oversized message is not read, parsed or answered");

    // One byte over the limit is over; a real command is some 200 bytes and goes through.
    let (mut socket, _) = open(address, "http://localhost:3000").await.unwrap();
    next(&mut socket).await;
    let id = identifier(&app, "user_7:bot_updates");
    assert!(json!({ "command": "subscribe", "identifier": id }).to_string().len() < cable::MAX_MESSAGE_BYTES / 8);
    command(&mut socket, "subscribe", &id).await;
    assert_eq!(next_event(&mut socket).await["type"], "confirm_subscription");
    socket.send(Message::Text("x".repeat(cable::MAX_MESSAGE_BYTES + 1).into())).await.unwrap();
    assert!(closed(&mut socket).await);
}

#[tokio::test(flavor = "current_thread")]
async fn a_connection_holds_a_bounded_number_of_subscriptions() {
    let (_dir, app, address) = served().await;
    let (mut socket, _) = open(address, "http://localhost:3000").await.unwrap();
    next(&mut socket).await;
    for n in 0..cable::MAX_SUBSCRIPTIONS {
        command(&mut socket, "subscribe", &identifier(&app, &format!("stream_{n}"))).await;
        assert_eq!(next_event(&mut socket).await["type"], "confirm_subscription", "subscription {n}");
    }
    command(&mut socket, "subscribe", &identifier(&app, "one_more")).await;
    assert!(closed(&mut socket).await, "the subscription over the limit ends the connection");
}

#[tokio::test(flavor = "current_thread")]
async fn only_a_signed_in_browser_may_connect() {
    let (dir, app, address) = served().await;
    let origin = "http://localhost:3000";
    assert_eq!(open_as(address, origin, None).await.err(), Some(401), "no cookie");
    assert_eq!(open_as(address, origin, Some("junk")).await.err(), Some(401), "not a cookie of ours");
    let signed_out = session::seal(&app.keys.session, &SessionData { csrf: Some("c3Jm".into()), ..SessionData::default() }, app.now());
    assert_eq!(open_as(address, origin, Some(&signed_out)).await.err(), Some(401), "a session, but nobody signed in (the login page's)");
    assert_eq!(open_as(address, origin, Some(&signed_in(&app, 99))).await.err(), Some(401), "no such user");

    // Devise's per-request check, as on a page: each of these ends a session that was signed in.
    let second = signed_in(&app, SECOND);
    assert!(open_as(address, origin, Some(&second)).await.is_ok());
    assert!(open_as(address, origin, Some(&format!("junk; _deltabadger_rust_session={second}"))).await.is_ok(), "a planted cookie of our name before the real one");
    let db = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    for (what, change, undo) in [
        ("locked", "locked_at = '2026-09-10 12:00:00'", "locked_at = NULL"),
        ("unconfirmed", "confirmed_at = NULL", "confirmed_at = '2026-01-01 00:00:00'"),
        ("password changed", "encrypted_password = '$2a$04$ANOTHERSALTANOTHERSALTANOTHERSALTANOTHERSALTANOTHERSALT0'", "encrypted_password = encrypted_password"),
    ] {
        db.execute(&format!("UPDATE users SET {change} WHERE id = {SECOND}"), []).unwrap();
        assert_eq!(open_as(address, origin, Some(&second)).await.err(), Some(401), "{what}");
        db.execute(&format!("UPDATE users SET {undo} WHERE id = {SECOND}"), []).unwrap();
    }
    assert!(open(address, origin).await.is_ok(), "the owner was never affected");
}

#[tokio::test(flavor = "current_thread")]
async fn a_user_at_the_connection_limit_takes_nothing_from_another_user() {
    let (_dir, app, address) = served().await;
    let origin = "http://localhost:3000";
    let mut tabs = Vec::new();
    for _ in 0..cable::MAX_CONNECTIONS_PER_USER {
        tabs.push(open(address, origin).await.unwrap().0);
    }
    assert_eq!(open(address, origin).await.err(), Some(503), "one more than the user's limit is refused before the upgrade");
    assert!(open_as(address, origin, Some(&signed_in(&app, SECOND))).await.is_ok(), "the second user is not affected");
    drop(tabs.pop()); // a tab closes
    let mut reopened = false;
    for _ in 0..40 {
        if open(address, origin).await.is_ok() { reopened = true; break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(reopened, "a closed connection gives its place back");
}

/// A connection lives longer than the request that authenticated it. Each one asks again, every
/// `cable_recheck` (100 ms here, 60 seconds outside tests), whether its session would still open it.
#[tokio::test(flavor = "current_thread")]
async fn an_open_connection_ends_when_its_session_would_no_longer_authenticate() {
    let (dir, app, address, clock) = served_with(Duration::from_millis(100)).await;
    let origin = "http://localhost:3000";
    let db = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let (mut owner, _) = open(address, origin).await.unwrap();
    assert_eq!(next(&mut owner).await["type"], "welcome");

    let (mut second, _) = open_as(address, origin, Some(&signed_in(&app, SECOND))).await.unwrap();
    db.execute("UPDATE users SET encrypted_password = ?1 WHERE id = ?2", (NEW_HASH, SECOND)).unwrap();
    assert!(closed(&mut second).await, "a password change ends the sockets the old session opened");

    let (mut second, _) = open_as(address, origin, Some(&signed_in_with(&app, SECOND, NEW_HASH))).await.unwrap();
    db.execute("UPDATE users SET locked_at = '2026-09-10 12:00:00' WHERE id = ?1", [SECOND]).unwrap();
    assert!(closed(&mut second).await, "so does a lock");

    assert_eq!(next(&mut owner).await["type"], "ping", "the owner's connection was never touched");
    clock.set(web::at("2026-10-10T12:00:30Z"));
    assert!(closed(&mut owner).await, "and a connection does not outlive its cookie: 30 days after the session was written");
}

/// A client that stops reading and keeps asking for subscriptions that are rejected, until the
/// rejections have nowhere to go: the server's writer is blocked, and so, soon after, is this
/// client's own. (Each rejection echoes an identifier of 3,000 bytes; the buffers between the two
/// ends hold a few megabytes.)
async fn jam(socket: &mut Socket) {
    let forged = json!({ "channel": "Turbo::StreamsChannel", "signed_stream_name": "x".repeat(3000) }).to_string();
    let flood = async {
        while socket.send(Message::Text(json!({ "command": "subscribe", "identifier": forged }).to_string().into())).await.is_ok() {}
    };
    assert!(tokio::time::timeout(Duration::from_secs(2), flood).await.is_err(), "the server went on accepting commands for two seconds");
}

/// Whether `user`'s connections are all gone within `within`, by the hub's own count.
async fn gone(app: &App, user: i64, within: Duration) -> bool {
    let wait = async { while app.hub.open(user) > 0 { tokio::time::sleep(Duration::from_millis(20)).await; } };
    tokio::time::timeout(within, wait).await.is_ok()
}

/// Revocation must not wait for a write. A connection whose client reads nothing has its writer
/// blocked, and is closed all the same when the periodic check finds its password changed.
#[tokio::test(flavor = "current_thread")]
async fn a_connection_whose_client_stopped_reading_is_still_closed_when_its_password_changes() {
    let (dir, app, address, _clock) = served_with(Duration::from_millis(100)).await;
    let (mut stuck, _) = open_as(address, "http://localhost:3000", Some(&signed_in(&app, SECOND))).await.unwrap();
    jam(&mut stuck).await;
    assert_eq!(app.hub.open(SECOND), 1, "blocked, and alive");
    let db = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    db.execute("UPDATE users SET encrypted_password = ?1 WHERE id = ?2", (NEW_HASH, SECOND)).unwrap();
    assert!(gone(&app, SECOND, Duration::from_secs(3)).await, "the recheck ended the task although its write never finished");
}

/// Without any revocation: a write that cannot finish ends the connection after `SEND_DEADLINE`.
#[tokio::test(flavor = "current_thread")]
async fn a_write_that_cannot_finish_ends_the_connection() {
    let (_dir, app, address, _clock) = served_with(Duration::from_secs(3600)).await;
    let (mut stuck, _) = open_as(address, "http://localhost:3000", Some(&signed_in(&app, SECOND))).await.unwrap();
    jam(&mut stuck).await;
    assert!(!gone(&app, SECOND, cable::SEND_DEADLINE / 2).await, "not before the deadline");
    assert!(gone(&app, SECOND, cable::SEND_DEADLINE * 2).await, "a client that reads nothing does not hold a task for ever");
}

/// One broadcast is one write per identifier subscribed to its stream, and a page may subscribe to
/// one stream under several identifiers. The first write that misses its deadline ends the
/// connection: the writes still owed are not tried, each for another deadline.
#[tokio::test(flavor = "current_thread")]
async fn a_broadcast_to_a_client_that_stopped_reading_ends_the_connection_at_the_first_missed_deadline() {
    let (_dir, app, address, _clock) = served_with(Duration::from_secs(3600)).await;
    let (mut socket, _) = open_as(address, "http://localhost:3000", Some(&signed_in(&app, SECOND))).await.unwrap();
    next(&mut socket).await;
    let signed = cable::signed_stream_name(&app.keys.streams, "user_2:bot_updates");
    for n in 0..8 {
        let id = json!({ "channel": "Turbo::StreamsChannel", "signed_stream_name": signed, "n": n }).to_string();
        command(&mut socket, "subscribe", &id).await;
        assert_eq!(next_event(&mut socket).await["type"], "confirm_subscription", "identifier {n}, the same stream");
    }
    // From here the client reads nothing. Four broadcasts of a megabyte are 32 writes of a megabyte,
    // far more than the buffers between the two ends hold: one of the first writes blocks.
    for _ in 0..4 {
        app.hub.broadcast("user_2:bot_updates", &"x".repeat(1024 * 1024));
    }
    // The write that is blocked gets one deadline. On this machine the connection was gone about
    // 10.06 seconds later in nearly every run and 15.2 in a few: now and then a blocked megabyte
    // still gets through after some five seconds (the kernel probing the full window, it seems),
    // and the deadline of the next write starts then. With a deadline for each of the thirty-odd
    // writes still owed it would live for minutes.
    let blocked_at = std::time::Instant::now();
    assert!(gone(&app, SECOND, cable::SEND_DEADLINE * 3).await, "one deadline, not one for every write still owed");
    assert!(blocked_at.elapsed() > cable::SEND_DEADLINE / 2, "and not before the deadline: {:?}", blocked_at.elapsed());
}

/// Sixteen sockets opened with a cookie from before a password change must not keep the user out.
/// Here the periodic check is an hour away: the new connection itself makes room. One of the
/// sixteen has stopped reading, so its writer is blocked when it is told to go.
#[tokio::test(flavor = "current_thread")]
async fn a_17th_connection_succeeds_after_the_16_stale_ones_are_invalidated() {
    let (dir, app, address, _clock) = served_with(Duration::from_secs(3600)).await;
    let origin = "http://localhost:3000";
    let stolen = signed_in(&app, SECOND);
    let mut held = Vec::new();
    for _ in 0..cable::MAX_CONNECTIONS_PER_USER {
        held.push(open_as(address, origin, Some(&stolen)).await.unwrap().0);
    }
    jam(&mut held[0]).await;
    assert_eq!(open_as(address, origin, Some(&stolen)).await.err(), Some(503), "the user is at the limit");

    let db = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    db.execute("UPDATE users SET encrypted_password = ?1 WHERE id = ?2", (NEW_HASH, SECOND)).unwrap();
    let recovered = signed_in_with(&app, SECOND, NEW_HASH);
    assert_eq!(open_as(address, origin, Some(&stolen)).await.err(), Some(401), "the old cookie opens nothing any more");
    let (mut seventeenth, _) = open_as(address, origin, Some(&recovered)).await.expect("the 16 stale connections gave their places back");
    assert_eq!(app.hub.open(SECOND), 1, "the new connection was admitted only after every one of the sixteen tasks had ended");
    for socket in &mut held {
        assert!(closed(socket).await, "and each of them was closed, the blocked one too");
    }
    assert_eq!(next(&mut seventeenth).await["type"], "welcome");

    // The limit itself still holds for connections that are good.
    let mut good = vec![seventeenth];
    for _ in 1..cable::MAX_CONNECTIONS_PER_USER {
        good.push(open_as(address, origin, Some(&recovered)).await.unwrap().0);
    }
    assert_eq!(open_as(address, origin, Some(&recovered)).await.err(), Some(503));
}

/// The ceiling for the whole process, without 256 sockets: the count the upgrade consults.
#[test]
fn the_process_has_a_ceiling_whoever_holds_the_connections() {
    let hub = cable::Hub::default();
    let enter = |user: i64| hub.enter(user, "salt", i64::MAX);
    let users = (cable::MAX_CONNECTIONS / cable::MAX_CONNECTIONS_PER_USER) as i64;
    let mut first_place = None;
    for user in 1..=users {
        for _ in 0..cable::MAX_CONNECTIONS_PER_USER { first_place = first_place.or(enter(user).map(|(id, _)| id)); }
        assert!(enter(user).is_none(), "user {user} is at its own limit");
    }
    assert!(enter(users + 1).is_none(), "the process is full: a user with no connection is refused too");
    hub.leave(1, first_place.unwrap());
    hub.leave(1, first_place.unwrap()); // giving a place back twice gives back one
    assert!(enter(users + 1).is_some(), "a place given back is anyone's");
    assert!(enter(1).is_none(), "and the process is full again");

}

/// A connection told to close is counted until its task has ended: the count is never less than
/// the connections that are alive. (The upgrade waits for the places; see `connect`.)
#[tokio::test(flavor = "current_thread")]
async fn a_place_is_given_back_when_its_connection_has_ended_not_when_it_is_told_to_close() {
    let hub = cable::Hub::default();
    let stale: Vec<_> = (0..cable::MAX_CONNECTIONS_PER_USER).map(|_| hub.enter(7, "old salt", i64::MAX).unwrap()).collect();
    assert_eq!(hub.evict_stale(7, "old salt", 0), 0, "all sixteen are good for this salt");
    assert_eq!(hub.evict_stale(7, "new salt", 0), cable::MAX_CONNECTIONS_PER_USER, "the password changed: all sixteen are told to close");
    for (_, told) in &stale {
        assert!(tokio::time::timeout(Duration::from_millis(50), told.notified()).await.is_ok(), "each got the signal");
    }
    assert_eq!(hub.open(7), cable::MAX_CONNECTIONS_PER_USER, "told, and still counted: they are still being torn down");
    assert!(hub.enter(7, "new salt", i64::MAX).is_none(), "so there is no room yet");
    for (index, (id, _)) in stale.iter().enumerate() {
        hub.leave(7, *id); // the connection's task has ended
        assert_eq!(hub.open(7), cable::MAX_CONNECTIONS_PER_USER - index - 1);
    }
    assert!(hub.enter(7, "new salt", 100).is_some(), "now there is");
    assert_eq!((hub.evict_stale(7, "new salt", 99), hub.evict_stale(7, "new salt", 100)), (0, 1), "a cookie that ran out at 100 is stale at 100");
}

#[test]
fn stream_names_are_signed_in_rails_shape_under_our_own_key() {
    let key = [7u8; 32];
    let signed = cable::signed_stream_name(&key, "user_7:bot_updates");
    let recorded = common::vectors()["turbo"]["stream_from"].as_str().unwrap().to_string();
    let rails_signed = recorded.split("signed-stream-name=\"").nth(1).unwrap().split('"').next().unwrap();
    assert_eq!(signed.split("--").next(), rails_signed.split("--").next(), "the data part is base64 of the JSON string, as in Rails");
    assert_eq!(cable::verified_stream_name(&key, &signed).as_deref(), Some("user_7:bot_updates"));
    assert_eq!(cable::verified_stream_name(&key, rails_signed), None, "a name Rails signed is not ours");
    assert_eq!(cable::verified_stream_name(&[8u8; 32], &signed), None);
    assert_eq!(cable::stream_source(&key, "user_7:bot_updates").replace(&signed, rails_signed), recorded, "the element is turbo_stream_from's");
}
