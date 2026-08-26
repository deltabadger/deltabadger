# First run

The setup page creates the admin account and, if you have one, connects a Deltabadger.com subscription. The first time you open Deltabadger it shows this page, titled **Welcome to Deltabadger**, and every other page redirects there until the admin account exists.

## Admin Account

Pick the interface language from the dropdown at the top; it is saved with the account and can be changed later under [Account settings](33-account-settings.md).

Fill in **Name**, **Email** and **Password**. The password needs at least 8 characters, an uppercase and a lowercase letter, a digit and a symbol; the checklist under the field turns green as you type. Press **Continue**.

This account is the admin: confirmed immediately, no email sent, and the only account that can set up market data, email notifications, the stock catalog and sign-up for other people (see [Multiple users](36-multiple-users.md)). After **Continue** you are signed in and land on the **Bots** page with the new-bot wizard already open. Close it if you want to look around first; **New bot** brings it back.

## Have a Deltabadger.com subscription?

Below the form, **Have a Deltabadger.com subscription?** unfolds a single field. Paste the **Connect code** from Deltabadger.com and press **Connect**. On success the field is replaced by **Connected ✓ — market data and exchange proxies active** (or **Connected ✓ — market data active; exchange proxies unavailable**) and **Name** and **Email** are prefilled from your subscription; both stay editable. An invalid or expired code shows an alert.

If you started the container with `CLAIM_TOKEN` (see [Quick start](02-quick-start.md)), the code is redeemed on its own when the page opens and you see the connected message straight away.

This step is optional. Without a subscription, create the account and use a free CoinGecko key later, or paste a connect code later, under **Settings → Connect → Market Data** (see [Market data](35-market-data.md)).

## Syncing assets

Once a market data provider is connected, by connect code now or under Settings later, a **Syncing assets…** banner appears at the top of every page once you are signed in, while Deltabadger loads the list of exchanges and assets and syncs each exchange's markets. With Deltabadger.com this takes a few minutes. With CoinGecko it pauses about a minute between exchanges to stay within the free plan's rate limit, so expect around a quarter of an hour. The banner disappears on its own.

You can use the app in the meantime: the bot wizard works from the asset list that shipped with the app, and anything listed since appears once the sync is done. If you never connect a provider, the banner never shows and the shipped list is what you have.

## What next

- Create a bot: [Creating your first bot](08-creating-your-first-bot.md).
- Connect the Tracker with read-only keys: [Connecting exchanges](21-connecting-exchanges.md).
- Set up market data if you skipped it: [Market data](35-market-data.md).
- Set up email so bots can alert you: [Email notifications](37-email-notifications.md).
- Protect the account: [Two-factor authentication](34-two-factor-authentication.md).
