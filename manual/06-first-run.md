# First run

On the [desktop app](04-desktop-app.md) there is no setup page: it opens signed in, and this page doesn't apply. Its first start asks for the language before anything else when the computer's language is English or one Deltabadger doesn't have; otherwise it starts in the computer's language.

The setup page creates the admin account. Until the admin account exists, every page leads to it. If your browser's language is one of Deltabadger's other than English, you land straight on the account form in that language; otherwise a language step comes first: pick a language and press **Continue**.

If the page says **Setup is locked** instead, the server was started with `SETUP_TOKEN` (see [Configuration](29-configuration.md)). Open it from the dashboard of the service that hosts it, which links to the setup page with the token; the token is then dropped from the address.

## Admin Account

The form is in the language you picked; it is saved with the account and can be changed later under [Account settings](24-account-settings.md). To pick another one, click the logo at the top of the form: it leads back to the language step.

Fill in **Email** and **Password**. The password needs at least 8 characters, an uppercase and a lowercase letter, a digit and a symbol; the checklist under the field turns green as you type. Press **Create**.

This account is the admin: confirmed immediately, no email sent, and the only account that can set up market data, email notifications, the stock catalog and sign-up for other people (see [Multiple users](27-multiple-users.md)). After **Create** you are signed in and land on the **Bots** page. **New bot** opens the new-bot wizard when you're ready.

## Have a Deltabadger.com subscription?

If you started the container with `CLAIM_TOKEN` (see [Quick start](02-quick-start.md)), the code is redeemed on its own when the setup page opens: **Connected ✓ — market data and exchange proxies active** (or **Connected ✓ — market data active; exchange proxies unavailable**) shows under the form, and **Email** is prefilled from your subscription.

Otherwise, create the account first and paste the connect code afterwards, under **Settings → Connect → Market Data** (see [Market data](26-market-data.md)); the bot wizard offers it too where market data is needed.

## Syncing assets

Once a market data provider is connected, by connect code now or under Settings later, a **Syncing assets…** banner appears at the top of every page once you are signed in, while Deltabadger loads the list of exchanges and assets and syncs each exchange's markets. With Deltabadger.com this takes a few minutes. With CoinGecko it pauses about a minute between exchanges to stay within the free plan's rate limit, so expect around a quarter of an hour. The banner disappears on its own.

You can use the app in the meantime: the bot wizard works from the asset list that shipped with the app, and anything listed since appears once the sync is done. If you never connect a provider, the banner never shows and the shipped list is what you have.

## What next

- Create a bot: [Creating a new bot](09-new-bot.md).
- Connect the Tracker with read-only keys: [API keys](21-supported-exchanges.md#api-keys).
- Set up market data if you skipped it: [Market data](26-market-data.md).
- Set up email so bots can alert you: [Email notifications](28-email-notifications.md).
- Protect the account: [Two-factor authentication](25-two-factor-authentication.md).
