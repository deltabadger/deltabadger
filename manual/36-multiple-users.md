# Multiple users

One Deltabadger can serve several people, each with their own login, exchange keys and bots. The account created on first run is the admin (see [First run](06-first-run.md)); everyone else signs up through a page the admin opens.

## Enable Sign Up

Open **Settings → Account → Multiple Accounts**. The widget shows ON or OFF. Press **Enable Sign Up** to open registration; press **Disable Sign Up** to close it again. Closing it does not remove anyone — existing users keep signing in.

While it is on, the login page shows a **Sign up** link leading to the **New Account** form: **Name**, **Email**, **Password**, then **Sign Up**. The new user lands on a "Thanks!" page asking them to click the confirmation link in their mailbox. The account cannot sign in until that link is clicked. If the address is already registered, the page still says Thanks! and an "Email already taken" notice goes to the existing owner instead.

Confirmation is an email, so sign-up only works with an email provider configured (see [Email notifications](37-email-notifications.md)). A user who did not get the email can ask for it again with **Resend link** on the login page.

## What every user has

Each user works in their own space. Bots, withdrawal rules, the Tracker, API keys, the REST API token, connected MCP clients, two-factor authentication, language, timezone, display currency, **Hide balances** and the Interactive Brokers connection are all per user. One user cannot see another user's keys, bots or transactions.

Stocks follow the same rule: the admin's Alpaca key only builds the shared catalog, and each user trades with their own (see [Stocks and ETFs](29-stocks-and-etfs.md)).

## What stays with the admin

Settings that affect the whole instance are hidden from other users and refused if they try anyway:

| Setting | Where |
|---|---|
| Market data provider | **Settings → Connect → Market Data** (see [Market data](35-market-data.md)) |
| Stocks catalog (Alpaca) | **Settings → Connect → Stocks** |
| Email notifications (SMTP) | **Settings → Account → Email Notifications** |
| Multiple Accounts | **Settings → Account** |
| Deltabadger.com connect code | Market Data widget and the setup page |

A non-admin who reaches a feature that needs one of these sees a message to that effect — for example "The CoinGecko API key is managed by your instance admin. Ask them to add it under Settings → Connect." when opening a tax report, or "Ask your admin to activate stock trading." in the bot wizard.

There is one admin, and the role cannot be handed over from the interface.
