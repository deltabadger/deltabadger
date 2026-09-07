# Advanced bot settings

Advanced triggers were designed for single-asset bots. On a multi-asset bot you also pick which asset the trigger watches.

A **Start buying** trigger fires once and is then deactivated, while **Buy only** works continuously.

## Market Cap based allocation

Multi asset bot allocations can be set either manually or based their market capitalisations:

<p><img width="441" height="30" alt="Screenshot" src="https://github.com/user-attachments/assets/098c84b3-19eb-4b46-8f87-d1a92179aee4" /></p>


> [!NOTE]
> **Market capitalisation** is a way to measure how big or valuable a coin is. It's calculated by multiplying the total number of coins by the current price of > one coin. In your bot, the coin with the bigger market cap gets a larger share of your investment.
> 
> Example:
> 
> Coin A: 2 millions coins exist, each worth $10. The market cap equals 2M × $10 = $20M.
> 
> Coin B: 1 million coins exist, each worth $1. Market cap = 1M × $1 = $1M.
> 
> If you invest $100, the bot will split it proportionally to their market caps: $95.2 to Coin A, $4.8 to Coin B.
> 
> But if Coin B's price rises to $3, its market cap becomes $3M. Now, the next $100 will allocate $87 to Coin A and $13 to Coin B.



## Pick a starting time

You can pick a day of the week, a calendar date, or just an hour.

<p><img width="464" height="30" alt="Screenshot" src="https://github.com/user-attachments/assets/188abc72-d5ed-4582-b061-de1e0ae4c2b9" /></p>

## Price threshold

Buy only in a certain price range.

<p><img width="464" height="54" alt="Screenshot" src="https://github.com/user-attachments/assets/97e0aa97-a69e-4cf6-861a-1c7f6a6d3c5d" /></p>

## Buy the dip

Activate your bot after big price drop, either from all-time-high, or in the last 24h.

<p><img width="464" height="75" alt="Screenshot" src="https://github.com/user-attachments/assets/d9ca884e-f8bd-4a64-8111-001ae4cdd7e9" /></p>

## Moving averages

Use a certain EMA or SMA as a trigger.

<p><img width="464" height="53" alt="Screenshot" src="https://github.com/user-attachments/assets/54ad7e58-ab3d-4cf1-bba0-37041e4d08fb" /></p>

## RSI

Use a certain RSI level as a trigger. The app uses the standard 14-candle setting for RSI.

<p><img width="464" height="53" alt="Screenshot" src="https://github.com/user-attachments/assets/e5b50e15-6319-489b-a40f-aae90f74119e" /></p>

## Set a spending limit

The bot stops buying after investing a certain amount.

<p><img width="464" height="33" alt="Screenshot" src="https://github.com/user-attachments/assets/83140222-983e-428e-9372-e654c9b1e69e" /></p>
