# Exchange Models

## API Key Validation (`get_api_key_validity`)

Trading keys and withdrawal keys are validated differently on purpose. A valid API key is not enough — we must confirm it has the correct permissions for its intended use.

- **Trading keys** must be validated with a trade-permission endpoint (e.g. `cancel_order` with a fake order ID, or checking permission flags like `canTrade`). A read-only endpoint would accept any valid key, even one without trade permissions.
- **Withdrawal keys** are validated with a read-only endpoint (e.g. `get_balances`, `get_accounts`). We only need to confirm the key is valid — withdrawal permission checks happen at withdrawal time.

Every exchange branches on `api_key.withdrawal?` in `get_api_key_validity` to select the appropriate validation strategy.
