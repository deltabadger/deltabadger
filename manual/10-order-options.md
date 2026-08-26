# Order options

Three toggles sit under the main sentence of every DCA bot: **Smart Intervals**, **FeeCutter** and a starting time. Each is off by default. Switch one on, set its value, and it saves on its own. All three are locked while the bot runs; press **Stop** to change them.

## Smart Intervals

The toggle reads **Smart Intervals** Split the amount into 10 USD units. It splits each contribution into smaller, equal orders spread evenly across the interval, so you buy more often at more prices. The line under it spells out the result: "Splitting your 100 USD / Week into units of 10 USD will result in one investment every about 17 hours."

The one parameter is the unit size, in the spend currency; it is prefilled with a tenth of the amount, or more when a tenth would sit under the floor described next. The field has a floor: a unit cannot be so small that orders would come more often than every five minutes, or finer than the currency's precision on the exchange.

Check the unit against the exchange minimum order size. A unit below it is skipped and buffered into the next one, so orders arrive less often than the line says, see [Amount and interval](09-amount-and-interval.md).

On a bot that sells a fixed amount of the asset the split is in the asset; a bot that sells for a fixed currency amount does not offer Smart Intervals, see [Selling](12-selling.md).

## FeeCutter

The toggle reads **FeeCutter** Cut fees using limit orders 0.1% below the price. It replaces market orders with limit orders placed at the last traded price minus the percentage you set (a selling bot places them above the price). An order that waits in the order book pays the exchange's maker fee instead of the taker fee.

The one parameter is the distance below the price, in percent; the default is 0.1. The line under the toggle names the exchange's maker fee: set the distance to it to cut the fee to zero, lower for faster execution, higher for a potentially better price.

A limit order fills only if the price reaches it. Until then it stays open on the exchange; the bot checks open orders at each run and counts them as invested, so it does not buy again in their place. Open orders are listed in the orders table, where you can cancel them, see [Charts, statistics and orders](14-charts-statistics-and-orders.md).

On Hyperliquid the toggle is locked on: Hyperliquid does not support market orders on spot.

## Starting time

The third toggle reads **Start on Monday** or **Start at**, depending on its mode. It delays the first order to a moment you choose; after it, the interval counts from that moment.

| Mode | Sentence | Meaning |
|---|---|---|
| **Monday** … **Sunday** | **Start on Monday** 09:30 | The next such weekday at that time |
| **date** | **Start at** a date and time | That exact moment; it must be in the future when you press **Start** |
| **hour** | **Start at** 00:00 | The next time the clock shows that hour, today or tomorrow |

Times are in your time zone, set under [Account settings](33-account-settings.md); the zone's abbreviation is shown after the field. The default is the next Monday at 09:30 New York time converted to your zone — the NYSE opening bell, which suits stock bots.

The setting applies to the first order only. Once that order runs, the toggle switches itself off and the bot continues at its interval. Switch it on again with a new value to plan another delayed start.
