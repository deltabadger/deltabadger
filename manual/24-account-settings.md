# Account settings

Account settings hold what is yours alone: name, email, password, language, time zone and display currency. Open the **Settings** menu at the bottom of the sidebar and choose **Account**; what you change there applies to your own login only. The number at the top of the page is the Deltabadger version you are running.

## Name, email and password

- **Name** — type a **New name** and press **Update**.
- **Email** — enter the **New email** and your **Current password**, then **Update**. A confirmation link goes to the new address, and the page shows "Currently, waiting confirmation for: …" until you click it. The old address stays in use until then. This needs a working email provider (see [Email notifications](28-email-notifications.md)).
- **Password** — enter your **Current password**, the **New password** and **Confirm new password**, then **Update**. The checklist under the fields shows what is required: at least 8 characters, an uppercase and a lowercase letter, a digit and a symbol. You stay signed in.

## Language, Timezone & Currency

Three pickers, each saved as soon as you change it.

- Language — English, Deutsch, Nederlands, Français, Español, Português, Italiano, Polski, Русский, Čeština, Slovenčina, Dansk, Svenska, Ελληνικά, Български. A language picked on the login page before signing in is used for that session; the one saved here is used whenever the URL does not name one.
- Timezone — used for every time Deltabadger shows you: bot logs and orders, chart labels, the Starting time picker and Tracker transaction dates. The line under the pickers shows the current time in the selected zone so you can check it.
- Currency — USD, EUR, GBP, CHF or PLN. Profits and portfolio totals are shown in this currency; bots keep trading in their own quote asset. The rate comes from your market data provider (see [Market data](26-market-data.md)); while it cannot be fetched, figures fall back to USD rather than showing the wrong symbol.

## Second Factor Authentication

The **Enable 2FA** / **Disable 2FA** button lives on this page. See [Two-factor authentication](25-two-factor-authentication.md).

## Wash sale protection

Some tax systems disallow a loss if you buy the same asset back too soon. Switch this on, pick the jurisdiction whose window applies to you, and every bot on the account leaves an asset out of every buy for that long after a sale that realised a loss — the money carries to the next asset instead. Selling is never blocked.

It is set here rather than per bot because what counts as a loss is a fact about the taxpayer: two bots holding the same asset would otherwise undo each other's harvest. The box lists every asset inside a window right now, across all bots, with the days remaining.

Deltabadger asks you to choose the first time you arm anything that can sell, and "don't apply this rule" is one of the answers. Until you answer one way or the other it keeps asking, because a switch left off cannot say whether you decided against it or never saw it.

Sales you make on the exchange yourself count too, from the next account sync. See [Direct indexing](13-direct-indexing.md) for the limits.

## Hide balances

A switch in the **Settings** menu, not on the Account page, that removes money figures from the Bots and Tracker pages. It is per user.

## Admin widgets

The admin's Account page carries two more widgets: **Multiple Accounts** (see [Multiple users](27-multiple-users.md)) and **Email Notifications** (see [Email notifications](28-email-notifications.md)). Other users do not see them.

## Logout

**Logout** is the last item in the **Settings** menu. It ends the session and invalidates every **Remember me** cookie for your account, so a browser that was remembered elsewhere has to sign in again.
