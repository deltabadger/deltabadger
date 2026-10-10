//! The signed-in shell: layouts/application.html.erb with layouts/navbar/_signed_in.html.haml.
use super::auth::{app_config, User};
use super::layout::{frame, html, Ctx, Page};
use super::{bots, cable, flash, ring, Inner, WebError};
use askama::Template;
use axum::response::Response;
use rusqlite::Connection;

impl Ctx {
    /// `turbo_stream_from`: the element a page subscribes to a stream with.
    pub fn stream_from(&self, name: &str) -> String {
        cable::stream_source(&self.app.keys.streams, name)
    }
}

/// What the application layout needs from the database on every signed-in page.
pub struct Shell {
    /// AppConfig.setup_sync_in_progress?
    pub syncing: bool,
    /// BotHelper#bot_menu_count: the user's bots that are neither deleted nor archived.
    pub bot_count: i64,
    /// `single_bot_mode?`: the user's one bot that is not deleted, when there is exactly one.
    pub single_bot: Option<i64>,
    /// TrackerHelper#allocation_icon_arcs: empty for the plain circle.
    pub arcs: Vec<ring::Arc>,
    single_bot_path: String,
}

impl Shell {
    pub fn load(c: &Connection, app: &Inner, user: &User) -> Result<Shell, WebError> {
        if c.is_autocommit(){
            let tx=c.unchecked_transaction()?;let origin=crate::sync::cache::capture_read(&tx,user.id,None)?;
            let mut shell=Self::load(&tx,app,user)?;tx.commit()?;
            if !crate::sync::cache::read_is_current(c,&origin)?{shell.arcs.clear();}
            return Ok(shell)
        }

        let mut not_deleted = c.prepare("SELECT id, status FROM bots WHERE user_id = ?1 AND status != 3 ORDER BY id")?;
        let bots = not_deleted.query_map([user.id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        let single_bot = match bots.as_slice() { [(id, _)] => Some(*id), _ => None };
        // AccountBalance.for_user(user).priced.joins(:asset).group('assets.symbol', 'assets.color').sum(:usd_value),
        // without cash unless the user's tracker shows it.
        let tracker_settings: Option<String> = c.query_row("SELECT tracker_settings FROM users WHERE id = ?1", [user.id], |r| r.get(0))?;
        let show_cash = bots::show_cash(tracker_settings.as_deref());
        let mut priced = c.prepare("SELECT assets.symbol, assets.color, SUM(account_balances.usd_value) FROM account_balances JOIN assets ON assets.id = account_balances.asset_id \
                                    WHERE account_balances.user_id = ?1 AND account_balances.usd_value IS NOT NULL AND account_balances.usd_value > 0 \
                                    GROUP BY assets.symbol, assets.color")?;
        let mut values = vec![];
        let mut rows = priced.query([user.id])?;
        while let Some(row) = rows.next()? {
            let (symbol, color): (Option<String>, Option<String>) = (row.get(0)?, row.get(1)?);
            if !show_cash && symbol.as_deref().is_some_and(|symbol| bots::CASH.contains(&symbol)) { continue; }
            // The column is decimal(20, 8): ActiveRecord casts the sum and rounds it to the column's scale.
            // A sum that is no number (an infinity: SQLite adds the column's REALs) fails as `format::Unreadable`; Rails raises drawing it.
            let value = row.get::<_, super::format::Stored>(2)?.0;
            if let Some(value) = value { values.push((value.round(8), color)); }
        }
        // Preserve Rails' unreadable-colour error even for an unknown legacy producer.
        // Only the checked arcs may reach the navbar; stale holdings remain unavailable.
        let mut arcs = ring::icon_arcs(values).ok_or_else(|| WebError::Config("an asset's colour is not a colour".into()))?;
        if crate::sync::cache::stale(c, user.id, None)? { arcs.clear(); }
        Ok(Shell {
            syncing: app_config(c, &app.cipher, "setup_sync_status")?.as_deref() == Some("in_progress"),
            bot_count: bots.iter().filter(|(_, status)| *status != 7).count() as i64,
            single_bot, arcs,
            single_bot_path: single_bot.map(|id| format!("/bots/{id}")).unwrap_or_default(),
        })
    }

    /// Where the navbar's bots links go: the one bot's page in single-bot mode, else the list.
    pub fn bots_path(&self) -> &str {
        if self.single_bot.is_some() { &self.single_bot_path } else { "/bots" }
    }

    pub fn bots_label(&self) -> &'static str {
        if self.single_bot.is_some() { "links.bot" } else { "links.bots" }
    }
}

/// BotHelper#bot_count_font_size: digits as tall as the icons beside them until three no longer fit.
pub fn bot_count_font_size(count: i64) -> i64 {
    let digits = count.to_string().len() as f64;
    ((22.0 / (digits * 0.549)).floor() as i64).min(19)
}

/// BotHelper#bot_count_baseline, printed as Ruby prints a Float rounded to two places.
pub fn bot_count_baseline(size: i64) -> String {
    let rounded = ((12.0 + size as f64 * 0.3655) * 100.0).round() / 100.0;
    if rounded.fract() == 0.0 { format!("{rounded:.1}") } else { rounded.to_string() }
}

/// User::DISPLAY_CURRENCIES with Denomination::UNITS.
const CURRENCIES: [(&str, &str); 5] = [("USD", "$"), ("EUR", "€"), ("GBP", "£"), ("CHF", "Fr."), ("PLN", "zł")];

#[derive(Template)]
#[template(path = "navbar/_signed_in.html")]
struct Navbar<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    user: &'a User,
    shell: &'a Shell,
    currencies: &'a [(&'static str, &'static str)],
    bot_count_font_size: i64,
    bot_count_baseline: String,
}

#[derive(Template)]
#[template(path = "layouts/application.html")]
struct ApplicationLayout<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    body_class: &'a str,
    flash: &'a str,
    shell: &'a Shell,
    navbar: &'a str,
    body: &'a str,
    flash_extra: &'a str,
}

/// layouts/application.html.erb with the signed-in navbar.
pub fn application(ctx: &Ctx, csrf: &str, user: &User, shell: &Shell, page: Page) -> Result<Response, WebError> {
    application_with_flash_extra(ctx,csrf,user,shell,page,"")
}

/// The tracker supplies Rails' content_for(:flash_extra), outside the permanent flash target.
pub fn application_with_flash_extra(ctx: &Ctx, csrf: &str, user: &User, shell: &Shell, page: Page, flash_extra: &str) -> Result<Response, WebError> {
    if ctx.turbo_frame.is_some() {
        return frame(csrf, &page);
    }
    let flash = flash::render(&flash::take(&ctx.session, &page.flash_now))?;
    let size = bot_count_font_size(shell.bot_count);
    let navbar = Navbar { v: ctx, csrf, user, shell, currencies: &CURRENCIES, bot_count_font_size: size, bot_count_baseline: bot_count_baseline(size) }.render()?;
    // Rails also has `hide-chrome` here, on the page that opens the new-bot wizard. Not served yet: see bots::index.
    let body_class = if user.hide_balances { "hide-balances" } else { "" };
    let layout = ApplicationLayout { v: ctx, csrf, body_class, flash: &flash, shell, navbar: &navbar, body: &page.body, flash_extra };
    Ok(html(page.status, layout.render()?))
}
