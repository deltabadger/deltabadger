# Account settings

Account settings hold what is yours alone: name, email, password, language, time zone and display currency. Open the **Settings** menu at the bottom of the sidebar and choose **Account**; what you change there applies to your own login only. The number at the top of the page is the Deltabadger version you are running.

## Name, email and password

- **Name** — type a **New name** and press **Update**.
- **Email** — enter the **New email** and your **Current password**, then **Update**. A confirmation link goes to the new address, and the page shows "Currently, waiting confirmation for: …" until you click it. The old address stays in use until then. This needs a working email provider (see [Email notifications](37-email-notifications.md)).
- **Password** — enter your **Current password**, the **New password** and **Confirm new password**, then **Update**. The checklist under the fields shows what is required: at least 8 characters, an uppercase and a lowercase letter, a digit and a symbol. You stay signed in.

## Language, Timezone & Currency

Three pickers, each saved as soon as you change it.

- Language — English, Deutsch, Nederlands, Français, Español, Português, Italiano, Polski, Русский, Čeština, Slovenčina, Dansk, Svenska, Ελληνικά, Български. A language picked on the login page before signing in is used for that session; the one saved here is used whenever the URL does not name one.
- Timezone — used for every time Deltabadger shows you: bot logs and orders, chart labels, the Starting time picker and Tracker transaction dates. The line under the pickers shows the current time in the selected zone so you can check it.
- Currency — USD, EUR, GBP, CHF or PLN. Profits and portfolio totals are shown in this currency; bots keep trading in their own quote asset. The rate comes from your market data provider (see [Market data](35-market-data.md)); while it cannot be fetched, figures fall back to USD rather than showing the wrong symbol.

## Second Factor Authentication

The **Enable 2FA** / **Disable 2FA** button lives on this page. See [Two-factor authentication](34-two-factor-authentication.md).

## Hide balances

A switch in the **Settings** menu, not on the Account page, that removes money figures from the Bots and Tracker pages. It is per user. What it hides is described on [Portfolio overview](22-portfolio-overview.md).

## Admin widgets

The admin's Account page carries two more widgets: **Multiple Accounts** (see [Multiple users](36-multiple-users.md)) and **Email Notifications** (see [Email notifications](37-email-notifications.md)). Other users do not see them.

## Logout

**Logout** is the last item in the **Settings** menu. It ends the session and invalidates every **Remember me** cookie for your account, so a browser that was remembered elsewhere has to sign in again.
