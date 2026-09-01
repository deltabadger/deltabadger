# Email notifications

Deltabadger emails you when a bot needs attention, and uses the same channel for sign-up confirmations and password resets. Nothing is sent until an SMTP server is configured. Open **Settings → Account → Email Notifications** — admin only — to set one up.

## Custom SMTP

The widget shows ON or OFF and two choices, **None** and **Custom SMTP**. Pick **Custom SMTP** and fill in:

| Field | Default |
|---|---|
| **SMTP host** | `smtp.gmail.com` |
| **Port** | `587` |
| **Username** | — |
| **Password** | — |

Username and password are required. For Gmail, use an App Password rather than your account password. Press **Set**. The connection uses STARTTLS with plain authentication.

Once saved, the widget reads "Configured with *host*." and offers two buttons:

- **Send Test Email** — sends "Deltabadger Test Email" to your own address and reports "Test email sent to …" or the server's error.
- **Disconnect** — forgets the host, port, username and password.

Choosing **None** stops sending but keeps the details, so selecting **Custom SMTP** again turns them back on without retyping.

## What is sent

Every email goes to the account's own address. Bot alerts and the test email use the account's language; sign-up, confirmation and password mails use the language of the page that triggered them.

| Subject | When |
|---|---|
| Something went wrong with *bot* | An order failed with an error that will not be retried, or the exchange kept rate-limiting the bot until it gave up. Repeated network failures are not emailed; the bot just tries again at the next interval. The mail quotes the exchange's error and points to the bot log. |
| Your *exchange* account is running out of *QUOTE* | A buying bot's spendable balance dropped below about three days of scheduled buys, or the exchange rejected an order for insufficient funds. The low-balance warning is sent at most once a day per quote asset; a rejected order emails every time it happens. |
| *bot* invested full amount / *bot* sold full amount | The amount limit was reached and the bot stopped itself (see [Triggers](11-triggers.md)). |
| Confirm your email | A new user signed up, or you changed your address (see [Multiple users](36-multiple-users.md)). |
| Reset password instructions | You used **Forgot password?** on the login page. |
| Email already taken | Someone tried to sign up with your address. |
| Deltabadger Test Email | You pressed **Send Test Email**. |

There is no per-user opt-out: with SMTP configured, every user gets their own alerts. Email is the only channel — there is no Telegram, push or webhook delivery.

## Sender address

Emails come from `NOTIFICATIONS_SENDER` if that is set in the environment, otherwise from the SMTP username, otherwise from `noreply@localhost` (see [Configuration](38-configuration.md)).

## Locked by the environment

When `SMTP_ADDRESS` is set in the environment, the widget shows that provider — named by `SMTP_PROVIDER_NAME`, or the address itself — greyed out, with only **Send Test Email** available. The `SMTP_*` variables are then the place to change anything.
