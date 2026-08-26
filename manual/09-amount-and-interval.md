# Amount and interval

The amount and the interval decide how much the bot spends and how often. Together they are the sentence at the top of a bot's page — **Invest 100 USD / Week into BTC** — and everything below it is optional.

## The sentence

The amount is a number field in the currency you picked when creating the bot. Type a new value and it saves on its own. Any amount above zero is accepted; the exchange minimum is handled separately, see below.

The interval is a dropdown: **Hour**, **Day**, **Week** or **Month**. A monthly bot orders on the same day of each month.

Both fields are locked while the bot runs. Press **Stop**, change them, then **Start** again; a bot that has run before then asks how to handle the missed part of the schedule, see [Managing bots](13-managing-bots.md).

The asset and the currency cannot be changed once the bot exists. To trade another pair, create another bot.

## How the schedule runs

When you press **Start**, the first order is placed right away and the interval counts from that moment. To delay the first order, set a starting time in [Order options](10-order-options.md).

An order that could not be placed is not lost. The bot keeps track of how much it should have invested since it started, and adds the shortfall to its next order.

## Exchange minimum order size

Every exchange sets a minimum order size for each pair, in the asset, in the currency, or both. When the amount is below it, the bot does not send the order. The orders table shows it as skipped, with the explanation "This order has been skipped because the amount was below the … minimum order size. The amount has been buffered and added to the next order." The buffered amount is added to the next scheduled order, so orders combine until they clear the minimum.

When this happens on the very first order, a modal titled **Congratulations! Your bot is running.** explains it and quotes the exchange minimum for your pair, in both the asset and the currency. Press **Got it!**; nothing needs to change unless you would rather raise the amount.

Splitting the amount with **Smart Intervals** makes each order smaller, so check the unit against the minimum too.

## Market orders

By default the bot buys at market price: the order fills immediately at the exchange's current ask and pays the taker fee. To place limit orders instead, switch on **FeeCutter** in [Order options](10-order-options.md).

## Running out of funds

When the balance in the spend currency falls below about three days of contributions, the bot emails you, see [Email notifications](37-email-notifications.md).
