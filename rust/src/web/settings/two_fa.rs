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
use askama::Template;
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use qrcode::Color;
use rusqlite::{Transaction, TransactionBehavior};
#[derive(Template)]
#[template(path = "settings/two_fa.html")]
struct TwoFa<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    enabled: bool,
    invalid: bool,
    status_label: String,
    button: String,
    qr: String,
}
fn new_seed() -> String {
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
        flash::set(
            &ctx.session,
            flash::NOTICE,
            ctx.t(if user.otp_enabled {
                "settings.two_fa.enabled"
            } else {
                "settings.two_fa.disabled"
            }),
        );
        return Ok((
            [(header::CONTENT_TYPE, turbo::CONTENT_TYPE)],
            turbo::refresh(),
        )
            .into_response());
    }
    let csrf = ctx.csrf_token();
    let body = TwoFa {
        v: &ctx,
        csrf: &csrf,
        enabled: user.otp_enabled,
        invalid: write,
        status_label: ctx.t(if user.otp_enabled {
            "helpers.label.settings.enabled"
        } else {
            "helpers.label.settings.disabled"
        }),
        button: ctx.t(if user.otp_enabled {
            "helpers.label.settings.disable_two_fa"
        } else {
            "helpers.label.settings.enable_two_fa"
        }),
        qr: qr(&ctx, &user, &seed)?,
    }
    .render()?;
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
