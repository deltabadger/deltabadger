# Broker tax report

A calculation basis for the German **Anlage KAP** and **KAP-INV** forms from a connected Alpaca account: which amount belongs on which line, and the worksheets behind each figure. It covers stocks and ETFs; crypto held at Alpaca goes into the [Crypto tax report](25-crypto-tax-report.md). It is a calculation aid, not tax advice — the file says so, and every figure is yours to check.

## What you need

- An Alpaca account connected on the Tracker (see [Stocks and ETFs](29-stocks-and-etfs.md)). The option appears only then.
- Market data configured, as for the crypto report (see [Market data](35-market-data.md)).
- Germany only. Supported years: 2023, 2024 and 2025.

## Generating the report

1. Open the Tracker, press **Tax Report** and choose **Broker tax report (Germany — Anlage KAP / KAP-INV)**. The country field disappears and **Year** narrows to the supported years.
2. Work through **Classify securities**. Every security needs a type: **Share**, **Fund** — with a category: **Equity funds**, **Mixed funds**, **Real estate funds**, **Foreign real estate funds** or **Other funds** — or **Other security**. Stocks are pre-set to Share. ETFs are marked **Default applied** and set to Fund / Other funds (0% Teilfreistellung, because US funds publish no InvStG terms); change that only if you can show the fund's equity quota. **Blocks the report** means the security was not found in market data and must be typed by hand. **Figures withheld** means it stays in the worksheets but is left out of the summaries, with the reason shown. Each change is saved as you make it and remembered for later reports.
3. Press **Generate**. It stays disabled until every security is classified.

The report runs in the background and downloads on its own when ready, or the next time you open the Tracker. The file is `deltabadger-broker-tax-report-de-<year>.csv`.

## What is in the file

The file is written in German, as the form is.

- **ZUSAMMENFASSUNG — ANLAGE KAP** (summary) — lines 19, 20, 22, 23 and 41, in EUR.
- **ZUSAMMENFASSUNG — ANLAGE KAP-INV** — per fund category: distributions, Vorabpauschale and result from disposal, each before Teilfreistellung, with the form's line number.
- **AUFSTELLUNG — VERÄUSSERUNGEN** (disposals) — each sale with units, USD proceeds, ECB rate, EUR proceeds, fees, acquisition cost, Vorabpauschale deducted and gain/loss, then one line per matched lot.
- **AUFSTELLUNG — ERTRÄGE** (income) — dividends, interest, fund distributions, withholding tax and return of capital exceeding acquisition cost; regulatory fees as a total, for information only.
- **HINWEISE** (warnings) and **RECHTLICHE HINWEISE** (legal notices).

## How it is computed

Lots are matched first in, first out, and every USD amount is converted at the ECB euro reference rate of its own date, or the previous banking day's when none was published. Dividends are grossed up by the tax withheld; interest is reported net. The full amount withheld goes to line 41. Return of capital reduces the cost basis, and anything beyond it counts as income. Stock splits rescale the lots. For a fund, a Vorabpauschale is worked out for every year-end a lot was held across, from year-end prices, the published Basiszins (known to the app for 2018–2026) and distributions; the previous year's amount is reported in the current year, and what has accrued is deducted from the gain when the lot is sold.

A security whose history cannot be fully substantiated is left out of the summaries rather than guessed at; its worksheet rows stay, marked. Reasons: a disposal that cannot be matched to acquisitions, a transaction type the report does not model (a merger, a spin-off), fund units bought before 2018, a missing year-end price, a missing ECB rate, or a year without a published Basiszins. Withholding tax on such a security is still credited on line 41, with a warning.

> **Note:** If any warning is present, the file's second row — under the title row — is a WARNUNG line saying the report is incomplete and must not be filed as-is.
