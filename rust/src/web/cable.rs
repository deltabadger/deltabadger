//! Live updates: Action Cable's WebSocket protocol on /cable, as much of it as turbo-rails' client
//! uses. The compiled JS only treats a page as live once every `<turbo-cable-stream-source>` has the
//! `connected` attribute, which only this protocol sets, so SSE is not an option
//! (app/javascript/controllers/broadcast/on_connect_controller.js).
//!
//! Server to client: `{"type":"welcome"}` on connect, `{"type":"ping","message":<epoch>}` every 3 s
//! (the client reconnects after 6 s of silence), `{"identifier":…,"type":"confirm_subscription"}` or
//! `"reject_subscription"`, and `{"identifier":…,"message":"<turbo-stream …>"}` for a broadcast.
//! Client to server: `{"command":"subscribe"|"unsubscribe","identifier":"<json>"}`.
//!
//! Who may connect: a browser with a signed-in session, checked on the upgrade request exactly as a
//! page request checks it. Rails does not authenticate the connection at all
//! (app/channels/application_cable/connection.rb identifies nobody): there a signed stream name is
//! the only gate, and anyone can open sockets. Here a stranger gets a 401 before the upgrade, and
//! what a signed-in user can hold is bounded: `MAX_MESSAGE_BYTES`, `MAX_SUBSCRIPTIONS`,
//! `MAX_CONNECTIONS_PER_USER`, `MAX_CONNECTIONS`. A signed stream name is still needed to subscribe.
//!
//! Which streams: in Rails whoever holds a signed name may subscribe to it. Here a stream that
//! belongs to one user (`user_<id>` or `user_<id>:…`, the names the pages sign for
//! `turbo_stream_from "user_<id>", …`) is confirmed only on that user's own connections
//! (`stream_is_for`); for anyone else it is rejected like a name that does not verify. Other
//! stream names are as in Rails.
//!
//! A connection lives longer than the request that opened it, so what authenticated it is kept with
//! it (the user, the password salt of the session, the end of the session's cookie) and asked again:
//! every `cable_recheck` by the connection itself, and at once when a new connection of the same
//! user finds no room. A connection whose user is gone, locked or unconfirmed, whose password has
//! changed, or whose cookie has run out is closed and its place freed. So a password change ends
//! the sockets a stolen cookie opened, and they cannot keep the user's places.
//!
//! Closing does not depend on the client. Reading, writing and pinging are one future, revocation is
//! another, and whichever ends first ends the connection: when revocation wins, the first future is
//! dropped with whatever write it was waiting on, and the socket with it, without a closing
//! handshake. Every write also has a deadline (`SEND_DEADLINE`), so a client that stops reading
//! cannot hold a task for ever. A place is counted until the connection's task has ended, never
//! less: a connection told to close still holds its place until it is gone.
use super::auth::{self, Current};
use super::session::{self, Session, SessionData};
use super::{canonical_origin, header_text, i18n::escape, App, Config};
use axum::extract::ws::{rejection::WebSocketUpgradeRejection, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::{engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD}, Engine};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;
use tokio::sync::Notify;

pub const PROTOCOL: &str = "actioncable-v1-json";
pub const CHANNEL: &str = "Turbo::StreamsChannel";

/// The largest message and frame read from a client. A subscribe command is about 200 bytes (the
/// identifier is a channel name and a signed stream name), so 4 KiB is some twenty times the largest
/// honest message. axum's defaults are 64 MiB and 16 MiB. The limit is checked against the length
/// in the frame header, before the payload is read.
pub const MAX_MESSAGE_BYTES: usize = 4096;
/// Subscriptions one connection may hold. The Rails page with the most stream sources has four
/// (three in tracker/index or bots/show, one in the layout's syncing notice); 32 leaves room for
/// pages to come and bounds a connection at 32 identifiers of at most 4 KiB.
pub const MAX_SUBSCRIPTIONS: usize = 32;
/// Connections one user may have open. Every open tab with a stream source is one connection, and
/// so is the desktop app's window: 16 fits a dozen tabs, the desktop app and a phone, with room. A
/// user at the limit takes nothing from anyone else.
pub const MAX_CONNECTIONS_PER_USER: usize = 16;
/// Connections open at once, whoever holds them: a backstop for what the single runtime thread
/// carries (per connection: a task, a receiver on the hub, its subscriptions). 256 is sixteen users
/// at their own limit, far more than one install has.
pub const MAX_CONNECTIONS: usize = 256;

/// How long one write to a client may take. A client that reads nothing fills the buffers between
/// the two ends, and then a write waits; after this long the connection is ended.
pub const SEND_DEADLINE: Duration = Duration::from_secs(10);
/// How long an upgrade that told stale connections of its user to close waits for their places.
const EVICTION_WAIT: Duration = Duration::from_secs(1);

/// Payloads a connection's mailbox holds before that connection is closed. One per target of a page (a newer
/// `replace` of a target supersedes the pending one), so a page with 500 bots fits.
pub const MAILBOX_ENTRIES: usize = 1536;
/// Bytes a connection's mailbox holds before that connection is closed: room for every chart of a large account twice.
pub const MAILBOX_BYTES: usize = 32 * 1024 * 1024;

/// Every broadcast goes into the mailbox of every connection subscribed to its stream.
pub struct Hub {
    boxes: Mutex<Vec<Weak<Mailbox>>>,
    seats: Mutex<Seats>,
}

/// What one connection still has to send. Posting never waits; the connection takes from the front.
#[derive(Default)]
pub struct Mailbox {
    inbox: Mutex<Inbox>,
    ready: Notify,
}

/// (the stream and target of a single `replace`, which a newer one supersedes; the stream; the payload)
type Pending = (Option<(Arc<str>, String)>, Arc<str>, Arc<str>);

#[derive(Default)]
struct Inbox {
    streams: HashSet<String>,
    queue: VecDeque<Pending>,
    bytes: usize,
    overflowed: bool,
}

/// The target of a payload that is exactly one `<turbo-stream action="replace">`, as turbo::stream writes it.
fn replaced_target(html: &str) -> Option<String> {
    let rest = html.strip_prefix("<turbo-stream action=\"replace\" target=\"")?;
    (html.matches("<turbo-stream").count() == 1).then(|| rest.split('"').next().unwrap_or_default().to_string()) // allow-swallow: an Option; split always yields a first piece
}

impl Mailbox {
    fn inbox(&self) -> std::sync::MutexGuard<'_, Inbox> {
        self.inbox.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// From now on, payloads of `stream` are kept.
    pub fn listen(&self, stream: &str) {
        self.inbox().streams.insert(stream.to_string());
    }

    /// Only payloads of these streams are kept from now on.
    pub fn only<'a>(&self, streams: impl Iterator<Item = &'a String>) {
        self.inbox().streams = streams.cloned().collect();
    }

    /// Keeps `html` if its stream is listened to, superseding a pending `replace` of the same stream and target. A payload
    /// that would take the mailbox past either bound overflows it: it drops everything it holds and keeps nothing more,
    /// and its connection is closed as soon as the write in hand returns or misses its deadline. So a mailbox never
    /// holds more than the bounds, even while its writer is blocked.
    pub fn post(&self, stream: &Arc<str>, html: &Arc<str>) {
        let mut inbox = self.inbox();
        if inbox.overflowed || !inbox.streams.contains(&**stream) {
            return;
        }
        let key = replaced_target(html).map(|target| (stream.clone(), target));
        if let Some(key) = &key {
            if let Some(at) = inbox.queue.iter().position(|(pending, _, _)| pending.as_ref() == Some(key)) {
                if let Some((_, _, old)) = inbox.queue.remove(at) {
                    inbox.bytes -= old.len();
                }
            }
        }
        if inbox.queue.len() >= MAILBOX_ENTRIES || inbox.bytes + html.len() > MAILBOX_BYTES {
            inbox.overflowed = true;
            inbox.queue = VecDeque::new();
            inbox.bytes = 0;
        } else {
            inbox.bytes += html.len();
            inbox.queue.push_back((key, stream.clone(), html.clone()));
        }
        drop(inbox);
        self.ready.notify_one();
    }

    /// The payloads waiting, oldest first.
    pub fn pending(&self) -> Vec<String> {
        self.inbox().queue.iter().map(|(_, _, html)| html.to_string()).collect()
    }

    /// Whether the mailbox overflowed: its connection is closed.
    fn overflowed(&self) -> bool {
        self.inbox().overflowed
    }

    /// The next (stream, payload); `None` once the mailbox overflowed, which closes the connection.
    async fn next(&self) -> Option<(Arc<str>, Arc<str>)> {
        loop {
            {
                let mut inbox = self.inbox();
                if inbox.overflowed {
                    return None;
                }
                if let Some((_, stream, html)) = inbox.queue.pop_front() {
                    inbox.bytes -= html.len();
                    return Some((stream, html));
                }
            }
            self.ready.notified().await;
        }
    }
}

/// A connection's mailbox, registered with the hub until the connection's task ends, however it ends.
struct Registered<'a> {
    hub: &'a Hub,
    mailbox: Arc<Mailbox>,
}

impl std::ops::Deref for Registered<'_> {
    type Target = Mailbox;
    fn deref(&self) -> &Mailbox {
        &self.mailbox
    }
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        let mut boxes = self.hub.boxes.lock().unwrap_or_else(PoisonError::into_inner);
        boxes.retain(|other| other.strong_count() > 0 && !std::ptr::eq(other.as_ptr(), Arc::as_ptr(&self.mailbox)));
    }
}

/// The open connections, per user id.
#[derive(Default)]
struct Seats {
    last_id: u64,
    by_user: HashMap<i64, Vec<Seat>>,
}

/// One open connection, with what authenticated it.
struct Seat {
    id: u64,
    /// The password salt in the session that opened it (Devise's authenticatable_salt).
    salt: String,
    /// When that session's cookie ends (epoch seconds).
    expires_at: i64,
    /// Told when the connection must close. The place stays taken until its task has ended.
    evicted: Arc<Notify>,
}

impl Seats {
    fn full(&self, user_id: i64) -> bool {
        self.by_user.get(&user_id).map_or(0, Vec::len) >= MAX_CONNECTIONS_PER_USER || self.by_user.values().map(Vec::len).sum::<usize>() >= MAX_CONNECTIONS
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self { boxes: Mutex::new(Vec::new()), seats: Mutex::new(Seats::default()) }
    }
}

/// One connection's place and what authenticated it. The connection's task owns it, and the place
/// is given back when the task has ended, however it ends, and not before.
struct Place {
    app: App,
    user_id: i64,
    id: u64,
    salt: String,
    expires_at: i64,
    evicted: Arc<Notify>,
}

impl Place {
    /// Whether the session that opened this connection would open one now: its cookie has not run
    /// out, and Devise's per-request check still passes for the salt it carried.
    async fn still_admitted(&self) -> bool {
        let now = self.app.now();
        let session = Session::new(SessionData { user: Some((self.user_id, self.salt.clone())), ..SessionData::default() });
        self.expires_at > now.timestamp() && matches!(auth::current_user(&self.app, &session, now).await, Ok(Current::SignedIn(_)))
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        self.app.hub.leave(self.user_id, self.id);
    }
}

impl Hub {
    /// Takes a place for a new connection of `user_id`, whose session carries `salt` and ends at
    /// `expires_at`. `None` when the user or the process is at its limit. Returns the place's id
    /// and the signal that tells the connection to close.
    pub fn enter(&self, user_id: i64, salt: &str, expires_at: i64) -> Option<(u64, Arc<Notify>)> {
        let mut seats = self.seats.lock().unwrap_or_else(PoisonError::into_inner);
        if seats.full(user_id) {
            return None;
        }
        seats.last_id += 1;
        let (id, evicted) = (seats.last_id, Arc::new(Notify::new()));
        seats.by_user.entry(user_id).or_default().push(Seat { id, salt: salt.to_string(), expires_at, evicted: evicted.clone() });
        Some((id, evicted))
    }

    /// Tells the connections of `user_id` that would no longer authenticate to close: those with
    /// another salt than `salt` (the caller has just authenticated a session with it, so it is the
    /// user's current one) and those whose cookie has run out. Returns how many were told. Their
    /// places stay taken until their tasks have ended (`leave`): a connection that is still being
    /// torn down is still counted.
    pub fn evict_stale(&self, user_id: i64, salt: &str, now: i64) -> usize {
        let seats = self.seats.lock().unwrap_or_else(PoisonError::into_inner);
        let stale = seats.by_user.get(&user_id).into_iter().flatten().filter(|seat| seat.salt != salt || seat.expires_at <= now);
        stale.map(|seat| seat.evicted.notify_one()).count()
    }

    /// How many places `user_id` holds: its connections whose tasks have not ended.
    pub fn open(&self, user_id: i64) -> usize {
        self.seats.lock().unwrap_or_else(PoisonError::into_inner).by_user.get(&user_id).map_or(0, Vec::len)
    }

    /// Gives back the place `enter` granted. Called when the connection's task ends, and only then.
    pub fn leave(&self, user_id: i64, id: u64) {
        let mut seats = self.seats.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(own) = seats.by_user.get_mut(&user_id) {
            own.retain(|seat| seat.id != id);
            if own.is_empty() {
                seats.by_user.remove(&user_id);
            }
        }
    }

    /// Sends `html` (one or more `<turbo-stream>` elements) to every page subscribed to `stream`: into each connection's
    /// mailbox, without waiting for any of them.
    pub fn broadcast(&self, stream: &str, html: &str) {
        let (stream, html): (Arc<str>, Arc<str>) = (stream.into(), html.into());
        let mut boxes = self.boxes.lock().unwrap_or_else(PoisonError::into_inner);
        boxes.retain(|mailbox| mailbox.strong_count() > 0);
        for mailbox in boxes.iter().filter_map(Weak::upgrade) {
            mailbox.post(&stream, &html);
        }
    }

    /// A new connection's mailbox, unregistered when the connection's task ends. Registering also forgets any mailbox
    /// whose connection is gone, so the hub holds the open connections' mailboxes, broadcasts or not.
    fn mailbox(&self) -> Registered<'_> {
        let mailbox = Arc::new(Mailbox::default());
        let mut boxes = self.boxes.lock().unwrap_or_else(PoisonError::into_inner);
        boxes.retain(|other| other.strong_count() > 0);
        boxes.push(Arc::downgrade(&mailbox));
        drop(boxes);
        Registered { hub: self, mailbox }
    }

    /// (mailboxes registered, payloads and bytes they hold): what the hub keeps for its connections.
    pub fn held(&self) -> (usize, usize, usize) {
        let boxes = self.boxes.lock().unwrap_or_else(PoisonError::into_inner);
        let mut held = (boxes.len(), 0, 0);
        for mailbox in boxes.iter().filter_map(Weak::upgrade) {
            let inbox = mailbox.inbox();
            (held.1, held.2) = (held.1 + inbox.queue.len(), held.2 + inbox.bytes);
        }
        held
    }
}

fn signature(key: &[u8; 32], data: &str) -> Option<String> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).ok()?; // allow-swallow: HMAC takes a key of any length; never fails
    mac.update(data.as_bytes());
    Some(hex::encode(mac.finalize().into_bytes()))
}

/// `<base64 of the JSON string>--<hex HMAC-SHA256 of that base64>`: the shape of Rails' signed
/// stream names, under this process's own key, so a name Rails signed is not accepted here.
pub fn signed_stream_name(key: &[u8; 32], name: &str) -> String {
    let data = B64.encode(Value::String(name.to_string()).to_string());
    format!("{data}--{}", signature(key, &data).unwrap_or_default()) // allow-swallow: an Option that is never None (above)
}

pub fn verified_stream_name(key: &[u8; 32], signed: &str) -> Option<String> {
    let (data, given) = signed.split_once("--")?;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).ok()?; // allow-swallow: HMAC takes a key of any length; never fails
    mac.update(data.as_bytes());
    mac.verify_slice(&hex::decode(given).ok()?).ok()?; // constant-time; allow-swallow: a name that does not verify is refused
    serde_json::from_slice::<Value>(&B64.decode(data).ok()?).ok()?.as_str().map(str::to_string) // allow-swallow: so is one that does not decode
}

/// Whether a connection of `user_id` may subscribe to `stream`. A name that begins `user_<digits>`,
/// alone or followed by `:`, is that user's and nobody else's; any other name is not tied to a user.
pub fn stream_is_for(stream: &str, user_id: i64) -> bool {
    let owner = stream.strip_prefix("user_").and_then(|rest| rest.split(':').next()).filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()));
    owner.is_none_or(|id| id == user_id.to_string())
}

/// Whose a stream named after a record is (`turbo_stream_from bot, …`: the record's GlobalID in URL-safe base64, then
/// `:`). `Some(false)`: not a record's stream; `Some(true)`: a bot of `user_id`'s; `None`: another user's bot, or a
/// record no connection may follow. A failure to read the bot's owner closes the connection, and the page's reconnect
/// asks again.
async fn record_stream(app: &App, stream: &str, user_id: i64) -> Result<Option<bool>, axum::Error> {
    let head = stream.split(':').next().unwrap_or_default(); // allow-swallow: an Option; split always yields a first piece
    let decoded = URL_SAFE_NO_PAD.decode(head.trim_end_matches('=')).ok() // allow-swallow: a name that is not base64 is not a record's
        .and_then(|bytes| String::from_utf8(bytes).ok()); // allow-swallow: nor one that is not text
    let Some(record) = decoded.as_deref().and_then(|gid| gid.strip_prefix("gid://")) else { return Ok(Some(false)) };
    let bot = record.strip_prefix("deltabadger/Bots::").and_then(|rest| rest.split_once('/'))
        .and_then(|(_, id)| id.parse::<i64>().ok()); // allow-swallow: a record that is not a bot is followed by no one
    let Some(bot) = bot else { return Ok(None) };
    let owned = app.db(move |c| Ok(c.query_row("SELECT count(*) FROM bots WHERE id = ?1 AND user_id = ?2", [bot, user_id], |r| r.get::<_, i64>(0))? > 0)).await
        .map_err(|_| axum::Error::new("the bot's owner could not be read"))?;
    Ok(owned.then_some(true))
}

/// `turbo_stream_from`: the element the page subscribes with.
pub fn stream_source(key: &[u8; 32], name: &str) -> String {
    format!("<turbo-cable-stream-source channel=\"{CHANNEL}\" signed-stream-name=\"{}\"></turbo-cable-stream-source>", escape(&signed_stream_name(key, name)))
}

/// Action Cable's allow_request_origin?, with one difference. The whole origin (scheme, host,
/// port) must be the deployment's own: APP_ROOT_URL's when that is set, and then no other, exactly
/// as the CSRF check of a form (Rails would also accept the origin of the request's own Host
/// there, which is whatever the client wrote). Without APP_ROOT_URL it is the one the request came
/// to: the scheme as Rails reads it (Rack's `ssl?`, so what a proxy forwarded counts) and the
/// `Host` header, which is all Action Cable looks at; a forwarded host is not consulted here.
/// A missing header is refused.
fn origin_allowed(config: &Config, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else { return false }; // allow-swallow: an Origin that is not text is refused
    let came_to = || header_text(headers, "host").map(|host| canonical_origin(if matches!(config.request_scheme(headers), "https" | "wss") { "https" } else { "http" }, host));
    config.own_origin.clone().or_else(came_to).is_some_and(|allowed| allowed == origin)
}

fn plain(status: StatusCode, body: &'static str) -> Response {
    (status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body).into_response()
}

pub async fn connect(State(app): State<App>, headers: HeaderMap, upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>) -> Response {
    let Ok(upgrade) = upgrade else { return plain(StatusCode::NOT_FOUND, "Page not found") };
    if !origin_allowed(&app.config, &headers) {
        return plain(StatusCode::NOT_FOUND, "Page not found"); // Rails' answer
    }
    // The session cookie, read as the page pipeline reads it, and Devise's per-request check: a
    // locked or unconfirmed account, or a session from before a password change, is not signed in.
    let now = app.now();
    let opened = session::from_request(&app.keys.session, &headers, now);
    let (data, expires_at) = opened.map_or((SessionData::default(), 0), |opened| (opened.data, opened.expires_at));
    let salt = data.user.as_ref().map(|(_, salt)| salt.clone()).unwrap_or_default(); // allow-swallow: an Option; no user is refused below
    let user_id = match auth::current_user(&app, &Session::new(data), now).await {
        Ok(Current::SignedIn(user)) => user.id,
        Ok(_) => return plain(StatusCode::UNAUTHORIZED, "Sign in first"),
        Err(error) => return error.into_response(),
    };
    let mut entered = app.hub.enter(user_id, &salt, expires_at);
    // No room. This session has just authenticated, so connections of the same user that no longer
    // would are told to close, and the upgrade waits, for a bounded time, until their tasks have
    // ended and given their places back. A place is never handed out while its connection lives.
    if entered.is_none() && app.hub.evict_stale(user_id, &salt, now.timestamp()) > 0 {
        let give_up = tokio::time::Instant::now() + EVICTION_WAIT;
        while entered.is_none() && tokio::time::Instant::now() < give_up {
            tokio::time::sleep(Duration::from_millis(10)).await;
            entered = app.hub.enter(user_id, &salt, expires_at);
        }
    }
    // The client retries a refused connection with a growing delay, so a full house heals by itself.
    let Some((id, evicted)) = entered else { return plain(StatusCode::SERVICE_UNAVAILABLE, "Too many connections") };
    let place = Place { app, user_id, id, salt, expires_at, evicted };
    upgrade.protocols([PROTOCOL]).max_message_size(MAX_MESSAGE_BYTES).max_frame_size(MAX_MESSAGE_BYTES).on_upgrade(move |socket| serve(place, socket))
}

/// One write, with its deadline: a write that cannot finish in `SEND_DEADLINE` ends the connection.
async fn send(socket: &mut WebSocket, message: Value) -> Result<(), axum::Error> {
    match tokio::time::timeout(SEND_DEADLINE, socket.send(Message::Text(message.to_string().into()))).await {
        Ok(sent) => sent,
        Err(_) => Err(axum::Error::new("the client does not read")),
    }
}

/// One connection's task: it talks to the client until either side ends it, or until its session
/// is revoked, whichever comes first. When revocation comes first, the talking future is dropped
/// where it stands, a write it was waiting on included, and the socket with it: the connection is
/// closed without waiting for the peer. The place is given back when this function returns.
async fn serve(place: Place, socket: WebSocket) {
    tokio::select! {
        _ = talk(&place.app, place.user_id, socket) => {}
        _ = revoked(&place) => {}
    }
}

/// Ends when this connection must close although the client has done nothing: a newer connection
/// of the same user needed its place and this one's session is no longer good (`evict_stale`), or
/// the check every `cable_recheck` finds that the session would no longer authenticate.
async fn revoked(place: &Place) {
    let mut recheck = tokio::time::interval(place.app.cable_recheck);
    recheck.tick().await; // the first tick is immediate, and the upgrade has just checked
    loop {
        tokio::select! {
            _ = place.evicted.notified() => return,
            _ = recheck.tick() => if !place.still_admitted().await { return },
        }
    }
}

/// Reading, writing and pinging, until either side ends the connection. The server ends it on a
/// message over `MAX_MESSAGE_BYTES` (the read fails), on a subscription over `MAX_SUBSCRIPTIONS`,
/// on a write that misses its deadline, and when the connection cannot keep up with the hub.
async fn talk(app: &App, user_id: i64, mut socket: WebSocket) {
    let mailbox = app.hub.mailbox();
    let mut subscriptions: Vec<(String, String)> = Vec::new(); // (identifier as the client sent it, stream)
    if send(&mut socket, json!({ "type": "welcome" })).await.is_err() {
        return;
    }
    let mut ping = tokio::time::interval(app.cable_ping);
    ping.tick().await; // the first tick is immediate, and the welcome has just gone out
    loop {
        if mailbox.overflowed() {
            return; // closed as soon as the write in hand has returned: whatever was published since is dropped
        }
        let sent = tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => command(app, user_id, &mut socket, &mut subscriptions, &mailbox, text.as_str()).await,
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => Ok(()),
            },
            _ = ping.tick() => send(&mut socket, json!({ "type": "ping", "message": app.now().timestamp() })).await,
            delivery = mailbox.next() => match delivery {
                Some((stream, html)) => deliver(&mut socket, &subscriptions, &stream, &html).await,
                // The mailbox overflowed: close, so the client reconnects and subscribes again, and is sent the latest
                // figures of each stream it subscribes (Task 4). Only this connection is affected.
                None => return,
            },
        };
        if sent.is_err() {
            return;
        }
    }
}

/// One broadcast, written once for every identifier subscribed to its stream. It stops at the first
/// write that fails or misses its deadline, and the connection ends there: the writes still owed
/// are not tried, so a client that reads nothing costs one `SEND_DEADLINE`, not one per identifier.
async fn deliver(socket: &mut WebSocket, subscriptions: &[(String, String)], stream: &str, html: &str) -> Result<(), axum::Error> {
    for (identifier, _) in subscriptions.iter().filter(|(_, subscribed)| subscribed == stream) {
        send(socket, json!({ "identifier": identifier, "message": html })).await?;
    }
    Ok(())
}

async fn command(app: &App, user_id: i64, socket: &mut WebSocket, subscriptions: &mut Vec<(String, String)>, mailbox: &Mailbox, text: &str) -> Result<(), axum::Error> {
    let Ok(message) = serde_json::from_str::<Value>(text) else { return Ok(()) };
    let Some(identifier) = message["identifier"].as_str() else { return Ok(()) };
    match message["command"].as_str() {
        Some("subscribe") => {
            if subscriptions.iter().any(|(known, _)| known == identifier) {
                return Ok(()); // the client re-sends an unconfirmed subscribe; Rails answers the first only
            }
            let options: Value = serde_json::from_str(identifier).unwrap_or(Value::Null); // allow-swallow: malformed subscription options have no channel and are ignored below, as in Rails
            if options["channel"] != CHANNEL {
                return Ok(()); // Rails logs "Subscription class not found" and sends nothing
            }
            // A name that verifies, and that is not another user's own stream.
            let stream = options["signed_stream_name"].as_str().and_then(|signed| verified_stream_name(&app.keys.streams, signed));
            let refused = json!({ "identifier": identifier, "type": "reject_subscription" });
            match stream.filter(|stream| stream_is_for(stream, user_id)) {
                Some(_) if subscriptions.len() >= MAX_SUBSCRIPTIONS => Err(axum::Error::new("too many subscriptions")),
                Some(stream) => match record_stream(app, &stream, user_id).await? {
                    None => send(socket, refused).await,
                    Some(own_bot) => {
                        mailbox.listen(&stream);
                        if own_bot || stream == format!("user_{user_id}:bot_updates") {
                            // A page that subscribes again, after its connection dropped a publication, ends with the
                            // current figures.
                            let on: Arc<str> = stream.as_str().into();
                            for html in super::figure::loading::resubscribed(app, user_id, &stream).await {
                                mailbox.post(&on, &html);
                            }
                        }
                        subscriptions.push((identifier.to_string(), stream));
                        send(socket, json!({ "identifier": identifier, "type": "confirm_subscription" })).await
                    }
                },
                None => send(socket, refused).await,
            }
        }
        Some("unsubscribe") => {
            subscriptions.retain(|(known, _)| known != identifier);
            mailbox.only(subscriptions.iter().map(|(_, stream)| stream));
            Ok(())
        }
        _ => Ok(()), // "message": Turbo::StreamsChannel has no actions
    }
}
