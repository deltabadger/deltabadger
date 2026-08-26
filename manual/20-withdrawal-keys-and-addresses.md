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
