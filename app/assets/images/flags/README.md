Flags from [flag-icons](https://github.com/lipis/flag-icons) (MIT), square (1x1) variants,
minified with svgo. Only the currencies in `Fiat.currencies` are vendored, not all 250.

They are the logo for fiat: a currency has no vendor logo of any kind, and a self-hosted
install reads its market data from CoinGecko, which has no image for one. Shipping them here
is what makes a currency look the same on every install.

Adding a currency to `Fiat.currencies` means adding its flag here and to `Fiat::FLAGS`;
`test/models/fiat_test.rb` fails until both are done.
