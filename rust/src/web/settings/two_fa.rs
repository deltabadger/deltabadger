use crate::{
    codec::format_time,
    web::{
        auth::{self, User},
        i18n::{self, escape},
        layout::{Ctx, Page},
        shell::{self, Shell},
        App, WebError,
    },
};
use axum::{
    http::StatusCode,
    response::Response,
};
use qrcode::Color;
use rusqlite::{Transaction, TransactionBehavior};
/// ROTP::Base32.random's shape: 32 characters of base32, 160 random bits.
pub(crate) fn new_seed() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let bytes: [u8; 32] = rand::random();
    bytes
        .iter()
        .map(|b| char::from(ALPHABET[usize::from(b & 31)]))
        .collect()
}
fn qr(ctx: &Ctx, user: &User, seed: &str) -> Result<String, WebError> {
    if user.otp_enabled {
        return Ok(String::new());
    }
    let email: String = form_urlencoded::byte_serialize(user.email.as_bytes()).collect();
    let uri = format!("otpauth://totp/Deltabadger:{email}?secret={seed}&issuer=Deltabadger");
    let colors = super::qr::colors(&uri)?;
    let width = 65;
    let mut rows = String::new();
    for line in colors.chunks(width) {
        rows.push_str("      <tr>\n");
        for color in line {
            rows.push_str(if *color == Color::Dark {
                "            <td class=\"black\"></td>\n"
            } else {
                "            <td class=\"white\"></td>\n"
            });
        }
        rows.push_str("      </tr>\n");
    }
    Ok(format!("<div class=\"qrcode\">\n  <table class=\"qr\" align=\"center\">\n{rows}  </table>\n</div>\n<div class=\"mb-4 text-center\">\n  {}\n</div>\n<div class=\"mb-5 text-center\">\n  <b>{}</b>\n</div>",ctx.t("helpers.label.settings.scan_code_info"),escape(seed)))
}
pub async fn handle(app: App, ctx: Ctx, write: bool) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else {
        return Ok(auth::unauthenticated(&ctx));
    };
    let inner = app.clone();
    let code = ctx
        .params
        .form("user[otp_code_token]")
        .unwrap_or("")
        .to_string();
    let (user, seed, shell, valid) = app
        .db(move |c| {
            let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate)?;
            let Some(mut user) = User::find(&tx, user.id)? else {
                return Err(WebError::Config("account no longer exists".into()));
            };
            let seed = user
                .otp_secret_key
                .as_deref()
                .map(|seed| {
                    inner
                        .cipher
                        .decrypt(seed)
                        .map_err(|_| WebError::Config("unreadable OTP seed".into()))
                })
                .transpose()?;
            let seed = if !write && seed.as_deref().is_none_or(crate::ruby::blank) {
                let seed = new_seed();
                let envelope = inner.cipher.encrypt(&seed);
                tx.execute(
                    "UPDATE users SET otp_secret_key=?1,updated_at=?2 WHERE id=?3",
                    (&envelope, format_time(inner.now()), user.id),
                )?;
                user.otp_secret_key = Some(envelope);
                seed
            } else {
                seed.unwrap_or_default()
            };
            let valid = if write {
                auth::verify_otp(&tx, &inner.cipher, &user, &code, inner.now())?
            } else {
                false
            };
            if valid {
                tx.execute(
                    "UPDATE users SET otp_module=?1,updated_at=?2 WHERE id=?3",
                    (
                        if user.otp_enabled { 0 } else { 1 },
                        format_time(inner.now()),
                        user.id,
                    ),
                )?;
                user.otp_enabled = !user.otp_enabled;
            }
            let shell = Shell::load(&tx, &inner, &user)?;
            tx.commit()?;
            Ok((user, seed, shell, valid))
        })
        .await?;
    if valid {
        let notice = ctx.t(if user.otp_enabled { "settings.two_fa.enabled" } else { "settings.two_fa.disabled" });
        return super::view::saved_row(&ctx, "two_fa", &super::view::two_fa_widget(&ctx, user.otp_enabled), notice);
    }
    let csrf = ctx.csrf_token();
    let label = ctx.t("helpers.label.settings.enter_two_fa_code");
    let mut input = format!(
        "<input class=\"rinput rinput--outline rinput--text{}\" required=\"required\" pattern=\"\\d{{6}}\" autocomplete=\"one-time-code\" inputmode=\"numeric\" placeholder=\"{label}\" aria-label=\"{label}\" type=\"text\" name=\"user[otp_code_token]\" id=\"user_otp_code_token\" />",
        if write { " is-invalid" } else { "" }
    );
    let mut error = String::new();
    if write {
        // errors.add(:otp_code_token, …): the full message on the form, the message under the field.
        let message = i18n::text(ctx.locale, "errors.messages.wrong_two_fa_token", &[]);
        let name = crate::web::setup::attribute_name(ctx.locale, "otp_code_token");
        let full = i18n::text_chain(ctx.locale, &["errors.format".into()], &[("attribute", i18n::Arg::Text(&name)), ("message", i18n::Arg::Text(&message))])
            .unwrap_or_else(|| format!("{name} {message}"));
        error = format!("<p class=\"sform__error\">{}</p>", escape(&full));
        let mut chars = message.chars();
        let message = chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default();
        input = format!("                   {input}\n             <div class=\"form__info form__info--invalid\">{}</div>\n", escape(&message));
    }
    let button = ctx.t(if user.otp_enabled { "helpers.label.settings.disable_two_fa" } else { "helpers.label.settings.enable_two_fa" });
    let cancel = format!("<a class=\"rbutton rbutton--outline\" href=\"{}\">{}</a>\n", escape(&ctx.path("/settings/account")), ctx.t("button.cancel"));
    let body = format!(
        "<turbo-frame class=\"sitem\" id=\"two_fa\">\n  <div>\n    {}    <form class=\"sform sform--stack\" action=\"{}\" accept-charset=\"UTF-8\" method=\"post\"><input type=\"hidden\" name=\"_method\" value=\"patch\" /><input type=\"hidden\" name=\"authenticity_token\" value=\"{csrf}\" />\n      {}\n      {error}\n      {input}\n      <input type=\"submit\" name=\"commit\" value=\"{button}\" class=\"rbutton rbutton--outline {}\" data-disable-with=\"{button}\" />\n</form>  </div>\n</turbo-frame>\n",
        super::view::row(&ctx.t("settings.index.two_factor_section"), None, None, "", "", Some(&cancel)),
        escape(&ctx.path("/settings/update_two_fa")),
        qr(&ctx, &user, &seed)?,
        if user.otp_enabled { "rbutton--danger" } else { "" }
    );
    shell::application(
        &ctx,
        &csrf,
        &user,
        &shell,
        Page {
            status: if write { StatusCode::UNPROCESSABLE_ENTITY } else { StatusCode::OK },
            body,
            flash_now: vec![],
        },
    )
}
