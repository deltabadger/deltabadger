## Your Funds Are Safe

Deltabadger always asks only for minimum required scope for your API keys. Trading keys don't allow for withdrawing your funds. They can only buy and sell assets on your behalf. You may notice that [automatic withdrawals](00-automatic-withdrawals.md) are not offered for all exchanges, but only for those that offer it safely. It means that the API doesn't allow to add a new withdrawal address, and you can only withdraw to an address you added in the exchange dashboard. 







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

# Withdrawal keys and addresses

A withdrawal rule needs two things from the exchange: a key that may withdraw but not trade, and an address the exchange already knows. Deltabadger never accepts a typed-in address, so the exchange's own allowlist decides where funds can go.

## Withdrawal keys

A withdrawal key is a separate API key from the one a bot trades with. Create it with these permissions and nothing else — in particular, leave trading off:

| Exchange | Enable |
|---|---|
| Binance, Binance.US | Enable Reading, Enable Withdrawals, IP restriction |
| Kraken | Funds: Query, Withdraw; IP restriction |
| Gemini | Account Balance, Fund Management; key scope Primary |
| MEXC | View Order Details, Withdraw; Link IP Address |

The **Connect exchange** step of the rule wizard shows the exact clicks for each exchange under **How to get API keys from …**, including the IP to whitelist (see [API keys](28-api-keys.md)). Paste the key and secret and press **Connect**. Binance and Binance.US reject a key whose permissions differ from the table above — trading still on, IP restriction off, or anything extra. On Kraken and Gemini a missing permission shows up as "Failed to validate API key permissions" rather than a rejection.

Withdrawal keys appear under **Settings → Connect → API keys** in the **Withdrawal keys** list. Click the exchange name to see the required permissions again; the X removes the key. Removing it does not stop the rule — the rule fails at its next check.

## Addresses

Deltabadger reads the destination from the exchange's address book (Binance, Binance.US, MEXC), pre-registered withdrawal addresses (Kraken, verified ones only) or approved addresses (Gemini, active ones only). Add the address on the exchange before creating the rule; where the exchange has a waiting period for new addresses, wait it out. The in-app guide also recommends switching on the exchange's withdrawal whitelist, so a compromised key cannot send funds anywhere else.

The **Address** step picks the first listed address for you. On the **Settings** step, and on the rule tile once the rule has been switched on and off again, the address is a dropdown of everything the exchange lists for that asset. Anything the exchange does not list is refused: the wizard returns you to the Address step, and the tile says "That address is not on the exchange's withdrawal allowlist".

Two things can go wrong on the Address step:

- "There is no defined withdrawal address for BTC. Set it up on Kraken first." — add the address on the exchange, then press **Check again**.
- "Could not fetch withdrawal addresses from Kraken. Please try again later." — the exchange did not answer, the key cannot read the list, or Deltabadger cannot list addresses for that asset on that exchange (Gemini covers a fixed set of coins). The key form is shown again so you can re-enter it; if the key is fine, wait and retry.

> **Note:** The address list is only as safe as your exchange account. Check that the address you register there is your own wallet on the right network; a withdrawal to a wrong address or network cannot be reversed.
