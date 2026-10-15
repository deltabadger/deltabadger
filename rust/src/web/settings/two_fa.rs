use crate::{
    codec::format_time,
    web::{
        auth::{self, User},
        flash,
        i18n::escape,
        layout::{Ctx, Page},
        shell::{self, Shell},
        turbo, App, WebError,
    },
};
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
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
        let flash = flash::render(&flash::take(
            &ctx.session,
            &[(flash::NOTICE, ctx.t(if user.otp_enabled { "settings.two_fa.enabled" } else { "settings.two_fa.disabled" }))],
        ))?;
        return Ok((
            [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)],
            turbo::stream("replace", "two_fa", &super::sections::two_fa(&ctx, user.otp_enabled)) + &turbo::prepend_flash(flash.trim_end()),
        )
            .into_response());
    }
    let csrf = ctx.csrf_token();
    // The wrong code is the field's own error, as `errors.add(:otp_code_token, …)` puts it.
    let error = write.then(|| {
        let text = ctx.t("errors.messages.wrong_two_fa_token");
        let mut chars = text.chars();
        chars.next().map(|first| first.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
    });
    let body = super::sections::two_fa_open(&ctx, &csrf, user.otp_enabled, &qr(&ctx, &user, &seed)?, error.as_ref());
    shell::application(
        &ctx,
        &csrf,
        &user,
        &shell,
        Page {
            status: if write {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::OK
            },
            body,
            flash_now: vec![],
        },
    )
}
