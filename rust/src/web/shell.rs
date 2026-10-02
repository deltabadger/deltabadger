//! The signed-in shell: layouts/application.html.erb with layouts/navbar/_signed_in.html.haml.
use super::auth::User;
use super::layout::{frame, html, Ctx, Page};
use super::{cable, flash, WebError};
use askama::Template;
use axum::response::Response;

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
}

/// layouts/application.html.erb with the signed-in navbar.
pub fn application(ctx: &Ctx, csrf: &str, user: &User, shell: &Shell, page: Page) -> Result<Response, WebError> {
    if ctx.turbo_frame.is_some() {
        return frame(csrf, &page);
    }
    let flash = flash::render(&flash::take(&ctx.session, &page.flash_now))?;
    let size = bot_count_font_size(shell.bot_count);
    let navbar = Navbar { v: ctx, csrf, user, shell, currencies: &CURRENCIES, bot_count_font_size: size, bot_count_baseline: bot_count_baseline(size) }.render()?;
    // Rails also has `hide-chrome` here, on the page that opens the new-bot wizard. Not in this plan: see bots::index.
    let body_class = if user.hide_balances { "hide-balances" } else { "" };
    let layout = ApplicationLayout { v: ctx, csrf, body_class, flash: &flash, shell, navbar: &navbar, body: &page.body };
    Ok(html(page.status, layout.render()?))
}
