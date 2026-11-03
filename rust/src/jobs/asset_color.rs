//! Asset::InferColorFromImageJob (app/jobs/asset/infer_color_from_image_job.rb, Asset#infer_color_from_image,
//! Utilities::Image): an asset's colour from its logo, after a CoinGecko refresh changed its image_url
//! (reference::refresh wakes it per asset).
//!
//! The image URL is upstream text. It is checked against Rails' host allow-list before anything is sent, fetched with a
//! 5 s timeout and a 5 MB cap counted while it arrives, never following a redirect, and its format is sniffed from the
//! bytes (PNG, JPEG, GIF, WebP; never SVG or anything else). Decoding runs one image at a time, off the runtime, under
//! pixel limits (D-COLOR-IMAGE-LIMITS).
//!
//! Rails asks ImageMagick for five colours (`convert -colors 5 -depth 8 -alpha on histogram:info:`). There is no
//! ImageMagick in the one binary, so the five come from a small quantizer that approximates it (D-COLOR-QUANTIZER;
//! tests/asset_colors.rs measures and pins the agreement). Everything after the histogram is Rails' own.
use super::{Cx, Job, JobFuture, Outcome, Retry, Spec, Wake};
use super::schedule::Jitter;
use image::{AnimationDecoder, ImageDecoder, ImageFormat, Limits, RgbaImage};
use rusqlite::OptionalExtension;
use std::io::Cursor;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

pub const NAME: &str = "infer_color_from_image_job";
/// Utilities::Image::MAX_BYTES and TIMEOUT.
pub const MAX_BYTES: usize = 5_000_000;
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// Utilities::Image::ALLOWED_HOSTS, plus the configured market-data host (allowed_hosts).
const HOSTS: [&str; 2] = ["coin-images.coingecko.com", "assets.coingecko.com"];
/// D-COLOR-IMAGE-LIMITS: the largest side, and every frame of an image together at most this many pixels.
pub const MAX_SIDE: u32 = 2048;
const MAX_PIXELS: u64 = (MAX_SIDE as u64) * (MAX_SIDE as u64);
const MAX_ALLOC: u64 = 32 << 20;
const MAX_FRAMES: usize = 64;
/// The quantizer looks at most this many pixels of a frame (an even stride); a 250 px logo is under it.
const SAMPLE: usize = 65_536;

/// Asset::COLOR_OVERRIDES (asset.rb), pinned by the recorded fixture (tests/asset_colors.rs).
pub const OVERRIDES: &[(&str, &str)] = &[
    ("ripple", "#6366F1"), ("altlayer", "#6f51b1"), ("nervos-network", "#3cc68a"), ("lisk", "#3e6ded"), ("bittensor", "#00C48C"), ("algorand", "#00bea5"),
    ("arweave", "#ff6700"), ("stellar", "#04b5e5"), ("fetch-ai", "#4B0082"), ("quant", "#bd2426"), ("worldcoin-wld", "#1A1A1A"), ("forefront", "#6366F1"),
    ("ai-rig-complex", "#001f3f"), ("jito", "#085639"), ("convex-finance", "#1682fe"), ("big-time", "#FFD700"), ("axelar", "#ff6414"), ("syndicate", "#5865F2"),
    ("zksync", "#1a24b2"), ("immutable-x", "#ecc968"), ("falcon-finance", "#0EA5E9"), ("usdtb", "#1E3A5F"), ("apenft", "#151432"), ("pippin", "#5B8DEF"),
    ("falcon-finance-ff", "#0EA5E9"), ("aioz-network", "#35D687"), ("aleo", "#c4ffc2"), ("creditcoin-2", "#1EEEB0"), ("moonbirds", "#39438A"),
    ("centrifuge-2", "#FFC012"), ("ergo", "#E74C3C"), ("wojak-4", "#90FF4A"), ("boundless", "#537263"), ("avici", "#FFA727"), ("covalent", "#FF4C8B"),
    ("future-ai", "#3380F6"), ("onomy-protocol", "#353340"), ("litr", "#7EB4D4"), ("infiblue-world", "#3A80A7"), ("smell", "#E86CA0"),
    ("qitmeer-network", "#58E7A3"), ("sakura-united-platform", "#C22A2A"), ("crystal-palace-fan-token", "#1B458F"), ("neblio", "#50479E"),
    ("calamari-network", "#6705BA"), ("lumishare", "#D4AF37"), ("safe-road-club", "#44A1B0"), ("pmg-coin", "#E84460"), ("donablock", "#4CAF82"),
    ("soldex", "#9C1CD8"), ("secretum", "#B8C4E6"), ("leeds-united-fan-token", "#1D428A"), ("futurecoin", "#7B2D8E"), ("snkrz-fit", "#FF6B2B"),
    ("xpmarket", "#1A1F71"), ("privateai", "#6C3FC5"), ("yachtingverse-old", "#1565C0"), ("marvellex-classic", "#D4A017"), ("blend-3", "#4A90D9"),
    ("dragon-3", "#C62828"), ("nomoex-token", "#2962FF"), ("qlindo", "#2E7D32"), ("zynecoin", "#F5A623"), ("virtual-x", "#00BFA5"), ("evrynet", "#3D5AFE"),
    ("wodo-gaming", "#7C4DFF"), ("apollo-name-service", "#FF7043"), ("consciousdao", "#00ACC1"), ("storepay", "#5C6BC0"), ("mongol-nft", "#E53935"),
    ("selo", "#4A6CF7"), ("fimarkcoin-com", "#1E88E5"), ("crt-ai-network", "#6C5CE7"), ("sedra-coin", "#00B4D8"), ("dank-doge", "#E8A317"),
    ("arkefi", "#C9A84C"), ("yolo", "#FF4136"), ("waltonchain", "#8247E5"), ("eon-marketplace", "#5B6EE1"), ("wattton", "#4CAF50"),
    ("lightning-bitcoin", "#F7931A"), ("inofi", "#7B61FF"), ("conun", "#2196F3"), ("sportsology-game", "#E53935"), ("artube", "#FF6B6B"),
    ("talki", "#3B82F6"), ("bionergy", "#2ECC71"), ("decred", "#2970FF"), ("pancakeswap-token", "#D1884F"), ("virtual-protocol", "#2A9D8F"),
    ("janus-henderson-anemoy-aaa-clo-fund", "#E87722"), ("kite-2", "#B5A88A"), ("a7a5", "#CC2222"), ("river", "#1E90FF"),
];

/// Utilities::Image.allowed_source?: https, and a host (any port; userinfo is not the host) in the list or the configured
/// market-data host. URI.parse refuses whitespace, controls and non-ASCII, which the url crate would encode instead.
pub fn allowed_source(url: &str, market_host: Option<&str>) -> bool {
    if url.chars().any(|c| !c.is_ascii() || c.is_ascii_whitespace() || c.is_ascii_control()) { return false; }
    let Ok(u) = reqwest::Url::parse(url) else { return false };
    let Some(host) = u.host_str().filter(|h| !h.is_empty()).map(str::to_ascii_lowercase) else { return false };
    u.scheme() == "https" && (HOSTS.contains(&host.as_str()) || market_host.is_some_and(|m| m == host))
}

/// The host of MarketDataSettings.deltabadger_public_url: AppConfig.market_data_url (a row, even an empty one, wins over
/// ENV['MARKET_DATA_URL']), whatever the provider; the docker alias `data-api` is the public host.
pub fn market_host(c: &rusqlite::Connection, cipher: &crate::crypto::Cipher, env_url: Option<&str>) -> Result<Option<String>, String> {
    let url = if crate::app_config::exists(c, "market_data_url")? { crate::app_config::get(c, cipher, "market_data_url")?.unwrap_or_default() } else { env_url.unwrap_or_default().to_string() };
    if url.trim().is_empty() { return Ok(None); }
    let public = match reqwest::Url::parse(&url) { Ok(u) if u.host_str() == Some("data-api") => "https://data.deltabadger.com".to_string(), _ => url };
    Ok(reqwest::Url::parse(&public).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase)))
}

/// Utilities::Image.coder_for: the format from the first bytes; anything else is refused.
pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" { return Some(ImageFormat::WebP); }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") { return Some(ImageFormat::Png); }
    if bytes.starts_with(b"\xFF\xD8\xFF") { return Some(ImageFormat::Jpeg); }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") { return Some(ImageFormat::Gif); }
    None
}

/// The image fetch: Rails' timeouts, no proxy, no redirect followed, no retry.
#[derive(Clone)]
pub struct Fetcher { client: reqwest::Client, reroute: Option<(String, String)> }

impl Default for Fetcher {
    fn default() -> Self { Self { client: crate::venue::http::client_with(TIMEOUT, TIMEOUT, TIMEOUT * 2), reroute: None } }
}

impl Fetcher {
    /// Tests: after the allow-list has passed `from…`, send it to `to…` instead (a local server), with `timeout`.
    #[doc(hidden)]
    pub fn rerouted(from: &str, to: &str, timeout: Duration) -> Self {
        Self { client: crate::venue::http::client_with(timeout, timeout, timeout), reroute: Some((from.into(), to.into())) }
    }

    /// Utilities::Image.fetch: any status under 400 has its body read (a 3xx is never followed), at most MAX_BYTES of it;
    /// every failure is None.
    pub async fn fetch(&self, url: &str) -> Option<Vec<u8>> {
        let target = match &self.reroute { Some((from, to)) if url.starts_with(from.as_str()) => format!("{to}{}", &url[from.len()..]), _ => url.to_string() };
        let mut resp = self.client.get(target).send().await.ok()?;
        if resp.status().as_u16() >= 400 { return None; }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.ok()? {
            if body.len() + chunk.len() > MAX_BYTES { return None; }
            body.extend_from_slice(&chunk);
        }
        Some(body)
    }
}

/// One decode at a time, process-wide: the permit is taken before the blocking work is spawned and travels with it, so a
/// caller dropped while waiting starts nothing, and one dropped while decoding holds the next back until it ends.
static DECODE: LazyLock<Arc<tokio::sync::Semaphore>> = LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(1)));

/// `work` on the blocking pool under the decode permit.
pub async fn decode_exclusive<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let permit = DECODE.clone().acquire_owned().await.ok()?;
    tokio::task::spawn_blocking(move || { let _permit = permit; work() }).await.ok()
}

/// Utilities::Image.dominant_colors_from_url: allow-list, fetch, sniff, then the five colours.
pub async fn dominant_colors_from_url(fetcher: &Fetcher, url: &str, market_host: Option<&str>) -> Option<Vec<String>> {
    if !allowed_source(url, market_host) { return None; }
    let bytes = fetcher.fetch(url).await?;
    let format = sniff(&bytes)?;
    decode_exclusive(move || dominant_colors(&bytes, format)).await.flatten()
}

fn limits() -> Limits {
    let mut l = Limits::default();
    l.max_image_width = Some(MAX_SIDE);
    l.max_image_height = Some(MAX_SIDE);
    l.max_alloc = Some(MAX_ALLOC);
    l
}

/// The frames to quantize: one for a still image, each GIF frame (as ImageMagick reads them) up to the pixel budget.
fn frames(bytes: &[u8], format: ImageFormat) -> Option<Vec<RgbaImage>> {
    if format == ImageFormat::Gif {
        let mut decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).ok()?;
        decoder.set_limits(limits()).ok()?;
        let mut out = vec![];
        let mut pixels = 0u64;
        for frame in decoder.into_frames().take(MAX_FRAMES) {
            let buffer = frame.ok()?.into_buffer();
            pixels += u64::from(buffer.width()) * u64::from(buffer.height());
            if pixels > MAX_PIXELS { break; }
            out.push(buffer);
        }
        return (!out.is_empty()).then_some(out);
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits());
    Some(vec![reader.decode().ok()?.to_rgba8()])
}

/// Utilities::Image.extract_dominant_colors on decoded bytes: each frame's clusters with their share of all pixels,
/// largest first (ties by colour), over 1 %, the first five, as `#RRGGBB`.
pub fn dominant_colors(bytes: &[u8], format: ImageFormat) -> Option<Vec<String>> {
    let frames = frames(bytes, format)?;
    let mut rows: Vec<(usize, String)> = vec![];
    let mut total = 0usize;
    for frame in &frames {
        let (clusters, n) = quantize(frame);
        total += n;
        rows.extend(clusters);
    }
    if total == 0 { return None; }
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    Some(rows.into_iter().filter(|(n, _)| *n as f64 / total as f64 > 0.01).map(|(_, hex)| hex).take(5).collect())
}

type Px = [u8; 4];

/// D-COLOR-QUANTIZER: five clusters of a frame and their pixel counts, and the pixels looked at. Pixels under half
/// opacity are one cluster (ImageMagick's RGBA quantizer separates transparency); the rest are median-cut into the
/// remaining boxes, then refined by eight rounds of k-means. A cluster's colour is its mean, rounded half to even.
fn quantize(frame: &RgbaImage) -> (Vec<(usize, String)>, usize) {
    let all = frame.as_raw().chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect::<Vec<Px>>();
    let stride = all.len().div_ceil(SAMPLE).max(1);
    let px: Vec<Px> = all.into_iter().step_by(stride).collect();
    let (clear, opaque): (Vec<Px>, Vec<Px>) = px.iter().partition(|p| p[3] < 128);
    let mut clusters: Vec<Vec<Px>> = vec![];
    if !clear.is_empty() { clusters.push(clear); }
    if !opaque.is_empty() { clusters.extend(kmeans(&opaque, median_cut(opaque.clone(), 5 - clusters.len()))); }
    let rows = clusters.iter().map(|c| (c.len(), hex(c))).collect();
    (rows, px.len())
}

fn hex(cluster: &[Px]) -> String {
    let mean = |i: usize| (cluster.iter().map(|p| f64::from(p[i])).sum::<f64>() / cluster.len() as f64).round_ties_even() as u8;
    format!("#{:02X}{:02X}{:02X}", mean(0), mean(1), mean(2))
}

/// Split the box with the widest channel range (the later box on a tie) at its median along that channel (the first
/// widest), until there are `k`.
fn median_cut(px: Vec<Px>, k: usize) -> Vec<Vec<Px>> {
    let range = |b: &[Px], c: usize| b.iter().map(|p| p[c]).max().unwrap_or(0) - b.iter().map(|p| p[c]).min().unwrap_or(0);
    let mut boxes = vec![px];
    while boxes.len() < k {
        let Some((_, i)) = boxes.iter().enumerate().filter(|(_, b)| b.len() > 1).map(|(i, b)| ((0..4).map(|c| range(b, c)).max().unwrap_or(0), i)).max() else { break };
        let mut b = boxes.remove(i);
        let widest = (0..4).map(|c| range(&b, c)).max().unwrap_or(0);
        let c = (0..4).find(|&c| range(&b, c) == widest).unwrap_or(0);
        b.sort_by_key(|p| p[c]);
        let right = b.split_off(b.len() / 2);
        boxes.push(b);
        boxes.push(right);
    }
    boxes
}

/// Eight Lloyd rounds from the boxes' means (RGB); a pixel goes to the nearest centre (the first on a tie), an empty
/// cluster keeps its centre; the non-empty clusters, in centre order.
fn kmeans(px: &[Px], boxes: Vec<Vec<Px>>) -> Vec<Vec<Px>> {
    let mean = |g: &[Px]| -> [f64; 3] { let n = g.len() as f64; [0, 1, 2].map(|c| g.iter().map(|p| f64::from(p[c])).sum::<f64>() / n) };
    let mut centres: Vec<[f64; 3]> = boxes.iter().map(|b| mean(b)).collect();
    let mut groups: Vec<Vec<Px>> = vec![];
    for _ in 0..8 {
        groups = vec![vec![]; centres.len()];
        for p in px {
            let d = |c: &[f64; 3]| (0..3).map(|i| (f64::from(p[i]) - c[i]).powi(2)).sum::<f64>();
            let mut best = 0;
            for (i, c) in centres.iter().enumerate() { if d(c) < d(&centres[best]) { best = i; } }
            groups[best].push(*p);
        }
        centres = groups.iter().zip(&centres).map(|(g, c)| if g.is_empty() { *c } else { mean(g) }).collect();
    }
    groups.into_iter().filter(|g| !g.is_empty()).collect()
}

/// Utilities::Image.most_vivid_color: HSL saturation, zero below 0.2 saturation, under 0.1 or over 0.9 lightness; the
/// first of the highest (so an all-grey logo gets its most frequent colour).
pub fn most_vivid_color(colors: &[String]) -> Option<String> {
    let mut best: Option<(&str, f64)> = None;
    for h in colors {
        let hex = h.trim_start_matches('#');
        let ch = |i: usize| hex.get(i..i + 2).and_then(|s| u8::from_str_radix(s, 16).ok()).map_or(0.0, |v| f64::from(v) / 255.0);
        let (r, g, b) = (ch(0), ch(2), ch(4));
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let (delta, l) = (max - min, (max + min) / 2.0);
        let s = if delta == 0.0 { 0.0 } else { delta / (1.0 - (2.0 * l - 1.0).abs()) };
        let score = if s < 0.2 || !(0.1..=0.9).contains(&l) { 0.0 } else { s };
        if best.is_none_or(|(_, top)| score > top) { best = Some((hex, score)); }
    }
    best.map(|(hex, _)| format!("#{hex}"))
}

/// Asset::InferColorFromImageJob for one asset (`perform(asset)`), no retry.
pub struct InferColor { pub asset: i64, pub fetcher: Fetcher, pub env_market_url: Option<String> }

impl InferColor {
    pub fn new(asset: i64) -> Self { Self { asset, fetcher: Fetcher::default(), env_market_url: std::env::var("MARKET_DATA_URL").ok() } }
}

impl Job for InferColor {
    fn spec(&self) -> Spec {
        Spec { name: NAME, scope: Some(self.asset.to_string()), schedule: None, jitter: Jitter::NONE, retry: Retry::None, deadline: Duration::from_secs(60) }
    }
    fn run<'a>(&'a self, cx: Cx<'a>, _wakes: Vec<Wake>) -> JobFuture<'a> { Box::pin(infer(self, cx)) }
}

/// Asset#infer_color_from_image, its guards in Rails' order.
async fn infer(job: &InferColor, cx: Cx<'_>) -> Outcome {
    let (asset, env_url) = (job.asset, job.env_market_url.clone());
    let read = cx.db.run(move |c, cipher| {
        crate::engine::provider::bind_cipher(c, cipher).map_err(|e| format!("{e:?}"))?;
        let deltabadger = crate::engine::provider::settings(c).map_err(|e| format!("{e:?}"))?.deltabadger();
        let row: Option<(Option<String>, Option<String>, Option<String>)> = c.query_row("SELECT color, external_id, image_url FROM assets WHERE id = ?1", [asset], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional().map_err(|e| e.to_string())?;
        Ok((deltabadger, row, market_host(c, cipher, env_url.as_deref())?))
    }).await;
    let (deltabadger, row, host) = match read { Ok(v) => v, Err(m) => return Outcome::Failed(m) };
    let Some((color, external_id, image_url)) = row else { return Outcome::Failed(format!("Couldn't find Asset with 'id'={asset}")) };
    let present = |s: &Option<String>| s.as_deref().is_some_and(|s| !s.trim().is_empty());
    if present(&color) && deltabadger { return Outcome::NothingNew; }
    let pick = if let Some((_, c)) = OVERRIDES.iter().find(|(id, _)| Some(*id) == external_id.as_deref()) {
        c.to_string()
    } else {
        let Some(url) = image_url.filter(|u| !u.trim().is_empty()) else { return Outcome::NothingNew };
        let colors = match dominant_colors_from_url(&job.fetcher, &url, host.as_deref()).await { Some(c) if !c.is_empty() => c, _ => return Outcome::NothingNew };
        let Some(pick) = most_vivid_color(&colors) else { return Outcome::NothingNew };
        pick
    };
    let now = crate::codec::format_time(cx.clock.now());
    // update!: written, and updated_at moved, only when the colour changes.
    match cx.db.run(move |c, _| c.execute("UPDATE assets SET color = ?1, updated_at = ?2 WHERE id = ?3 AND color IS NOT ?1", rusqlite::params![pick, now, asset]).map_err(|e| e.to_string())).await {
        Ok(_) => Outcome::Done,
        Err(m) => Outcome::Failed(m),
    }
}
