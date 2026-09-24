## Bot Creation Wizard

Every DCA bot the wizard creates is a basket (`Bots::DcaMultiAsset`), one asset included.

Exchange-first (the default order):
1. `DcaSingleAssets::PickExchangesController` — before any asset
2. `DcaSingleAssets::AddApiKeysController` — before any asset
3. `DcaSingleAssets::PickBuyableAssetsController` — collects the basket; Next hands it to the multi namespace
4. `DcaMultiAssets::PickSpendableAssetsController` — picks the quote and saves the bot

Asset-first (`DcaSingleAssets::OrdersController` switches): the asset step, then
`DcaMultiAssets::PickExchangesController` (or `PickStockBrokersController` for stocks),
`DcaMultiAssets::AddApiKeysController`, `DcaMultiAssets::PickSpendableAssetsController`.

Once a basket is chosen, the single-namespace exchange and key steps redirect to their multi
counterparts.
