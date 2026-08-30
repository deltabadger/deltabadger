# Withdrawal rules

A withdrawal rule sends an asset from an exchange to a wallet address registered on that exchange, on its own, once the balance is worth moving. Use it to keep what your bots buy off the exchange without doing it by hand. Rules live under **Rules** in the left menu.

## What a rule says

A rule is one sentence: **Withdraw** N **% of** ASSET **from** EXCHANGE **to** ADDRESS **when** … The percentage defaults to 100. The threshold is one of two:

- **fee is less than** N **%** — waits until the balance is large enough that the withdrawal fee is under N % of the amount sent. Default 0.5 %. Offered only when the exchange reports a fee for the asset; the tile then says at which balance the withdrawal triggers.
- **the amount crosses** N ASSET — waits until the share you withdraw would be at least N (with 100 % that is a balance of N). The tile shows the fee as a percentage of that amount, in red above 1 %.

**or at least every** N **days** is optional: a withdrawal also goes out once N days have passed since the last successful one, even below the threshold. Leave it empty to turn it off.

Each withdrawal takes N % of the balance not tied up in open orders and sends it minus the fee. If the exchange offers several networks for the asset, the **Settings** step adds **on** NETWORK with the fee per network; pick the one your wallet expects — it cannot be changed after the rule is created.

## Creating a rule

Press **Add** on the Rules page:

1. **Pick asset** — a crypto asset.
2. **Pick exchange** — only exchanges that support withdrawals are listed: Binance, Binance.US, Kraken, Gemini and MEXC.
3. **Connect exchange** — a withdrawal key, separate from any trading key. Skipped when you already have one for that exchange.
4. **Address** — chosen from the exchange's own address book; you cannot type one in.
5. **Settings** — the sentence above, then **Create Rule**.

Steps 3 and 4 are covered in [Withdrawal keys and addresses](20-withdrawal-keys-and-addresses.md). One rule per asset and exchange. A new rule starts switched off.

## The rule tile

The toggle switches the rule on and off. Switching it on shows "The rule is active." and checks the balance right away. Settings and the address can only be changed while the rule is off. The red X next to the toggle deletes a rule that has been switched on and off again (a rule that has never been on has no X yet), after asking "Do you want to delete this withdrawal rule?". When the rule has a network, a memo or an address label, hovering the address shows the full address with those details.

While the rule is on, a bar under the sentence shows how close the last balance the rule saw is to the trigger amount. It drops to 0 % after a withdrawal.

## When rules are checked

Every active rule is evaluated every 4 hours, and immediately when you switch it on. Each check reads the balance, refreshes the withdrawal fee if it is older than a day, then either withdraws or waits.

## The log

The table under the tiles lists the last 50 entries across your rules: **Date**, **Status**, **Message**. `success` is a withdrawal ("Withdrew 0.01 BTC"), `failed` is an error from the exchange, and `transient` (gray) is a temporary problem the next check retries on its own. Checks that found the balance below the threshold are not listed.
