# The bot fixtures of the page-parity grid (script/rust/pages.rb loads this file): an install like
# the owner's, with stock bots on Alpaca, their orders and activity, and the scheduled jobs Rails
# itself would hold for them. Everything is written through the Rails models, as the wizard and the
# engine's jobs write it; only the order and activity rows are inserted without callbacks, because a
# callback there broadcasts and enqueues, which no page of the grid reads.
module Pages
  module_function

  STOCKS = %w[QQQM IBIT NVDA MSFT AAPL AMZN META AVGO GOOGL TSLA COST NFLX AMD PEP].freeze
  # Every second asset has a colour, as the market-data sync gives it; the others fall back to the neutral one.
  COLORS = { 'QQQM' => '#1A2B3C', 'NVDA' => '#76B900', 'AAPL' => '#F5F5F7', 'META' => '#0668E1', 'GOOGL' => '#050505',
             'COST' => '#E31837', 'AMD' => '#ED1C24' }.freeze

  # 'install' => 'alpaca': the venue, its stock listings in USD, the user's trading key, a second
  # broker that lists the same two ETFs, and the ND100 index as data-api's sync stores it.
  def alpaca(scenario)
    alpaca = Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
    ibkr = Exchanges::Ibkr.create!(name: 'Interactive Brokers', maker_fee: '0.05', taker_fee: '0.05')
    usd = Asset.create!(external_id: 'usd', symbol: 'USD', name: 'US Dollar', category: 'Currency')
    [alpaca, ibkr].each { |exchange| ExchangeAsset.create!(exchange:, asset: usd) }
    STOCKS.each_with_index do |symbol, position|
      asset = Asset.create!(external_id: "#{symbol.downcase}.us", symbol:, name: "#{symbol} Inc", category: 'Stock', color: COLORS[symbol])
      venues = position < 2 ? [alpaca, ibkr] : [alpaca]
      venues.each do |exchange|
        ExchangeAsset.create!(exchange:, asset:)
        Ticker.create!(exchange:, base_asset: asset, quote_asset: usd, base: symbol, quote: 'USD', ticker: symbol, base_decimals: 9,
                       quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1')
      end
    end
    # Two coins with a market cap, which stocks do not have: a basket of them is offered market-cap weights.
    { 'BTC' => ['#F7931A', 1_300_000_000_000], 'ETH' => ['#627EEA', 400_000_000_000] }.each do |symbol, (color, market_cap)|
      asset = Asset.create!(external_id: symbol.downcase, symbol:, name: symbol, category: 'Cryptocurrency', color:, market_cap:)
      ExchangeAsset.create!(exchange: alpaca, asset:)
      Ticker.create!(exchange: alpaca, base_asset: asset, quote_asset: usd, base: symbol, quote: 'USD', ticker: "#{symbol}/USD", base_decimals: 9,
                     quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000027', minimum_quote_size: '1')
    end
    members = STOCKS.drop(2).map { |symbol| "#{symbol.downcase}.us" }
    Index.create!(external_id: 'nasdaq-100', source: 'deltabadger', name: 'Nasdaq 100', top_coins: members,
                  weights: members.each_with_index.to_h { |id, position| [id, 20 - position] })
    { 'market_data_provider' => 'deltabadger', 'market_data_url' => 'http://data-api:3000', 'market_data_token' => 'token' }
      .each { |key, value| AppConfig.set(key, value) }
    more_stocks(alpaca, usd) if scenario['more_stocks']
    keys = scenario.fetch('api_keys', { 'alpaca' => 'correct' })
    { 'alpaca' => alpaca, 'ibkr' => ibkr }.each do |name, exchange|
      # The status comes from ApiKey's own list, never straight from scenario data.
      status = ApiKey.statuses.keys.find { |known| known == keys[name] }
      next unless status

      ApiKey.create!(user: User.first, exchange:, key: 'PKTEST', secret: 'secret', passphrase: 'paper', status:, key_type: :trading)
    end
  end

  MORE_STOCKS = %w[ADBE AMAT ASML BKNG CMCSA CSCO CSX DXCM EA EXC FAST FTNT GILD HON IDXX INTC INTU ISRG KDP KLAC LRCX MAR MDLZ MELI MNST
                   MU ORLY PANW PYPL QCOM].freeze

  # 'more_stocks' => true: thirty more Alpaca listings, every third one ranked by market cap, so an asset search runs past its
  # first page of 20 and orders ranked rows before unranked ones.
  def more_stocks(alpaca, usd)
    MORE_STOCKS.each_with_index do |symbol, position|
      asset = Asset.create!(external_id: "#{symbol.downcase}.us", symbol:, name: "#{symbol} Inc", category: 'Stock',
                            market_cap_rank: (position % 3).zero? ? 300 - position : nil)
      ExchangeAsset.create!(exchange: alpaca, asset:)
      Ticker.create!(exchange: alpaca, base_asset: asset, quote_asset: usd, base: symbol, quote: 'USD', ticker: symbol, base_decimals: 9,
                     quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1')
    end
  end

  MORE_COINS = %w[ADA AVAX DOT LINK LTC XRP DOGE TRX BCH XLM ATOM UNI AAVE NEAR FIL ALGO ICP ETC XTZ EOS MKR COMP SNX CRV GRT SAND MANA AXS
                  CHZ ENJ].freeze

  # 'more_coins' => true: thirty more coins on Kraken and Binance, every third one ranked by market cap, so an asset search on
  # either venue runs past its first page of 20, blank or searched for "coin".
  def more_coins(venues)
    MORE_COINS.each_with_index do |symbol, position|
      asset = Asset.create!(external_id: "#{symbol.downcase}-coin", symbol:, name: "#{symbol} Coin", category: 'Cryptocurrency',
                            market_cap_rank: (position % 3).zero? ? 300 - position : nil)
      venues.each do |exchange, quote_asset|
        ExchangeAsset.create!(exchange:, asset:)
        Ticker.create!(exchange:, base_asset: asset, quote_asset:, base: symbol, quote: quote_asset.symbol, ticker: "#{symbol}#{quote_asset.symbol}",
                       base_decimals: 8, quote_decimals: 5, price_decimals: 4, minimum_base_size: '0.1', minimum_quote_size: '5')
      end
    end
  end

  # 'venues' => true: Kraken and Binance beside Alpaca, listing bitcoin, ether and solana as their catalogs do: Kraken in EUR
  # under its own code XBT, Binance in USDT; the user's trading key on each ('venue_keys' says which and how). Solana is on
  # neither Alpaca nor any bot of the owner's, so a basket of it stays on its venue, and a basket without it can add it.
  def venues(scenario)
    kraken = Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4')
    binance = Exchanges::Binance.create!(name: 'Binance', maker_fee: '0.1', taker_fee: '0.1')
    eur = Asset.create!(external_id: 'EUR.FOREX', symbol: 'EUR', name: 'Euro', category: 'Currency')
    usdt = Asset.create!(external_id: 'tether', symbol: 'USDT', name: 'Tether', category: 'Cryptocurrency', color: '#26A17B')
    btc, eth = Asset.find_by!(symbol: 'BTC'), Asset.find_by!(symbol: 'ETH')
    sol = Asset.create!(external_id: 'solana', symbol: 'SOL', name: 'Solana', category: 'Cryptocurrency', color: '#9945FF', market_cap: 80_000_000_000)
    listings = [[kraken, btc, eur, 'XBT', 'XBTEUR', 8, 5, 1, '0.00005', '0.5'], [kraken, eth, eur, 'ETH', 'ETHEUR', 8, 5, 2, '0.002', '0.5'],
                [binance, btc, usdt, 'BTC', 'BTCUSDT', 5, 8, 2, '0.00001', '5'], [binance, eth, usdt, 'ETH', 'ETHUSDT', 4, 8, 2, '0.0001', '5'],
                [kraken, sol, eur, 'SOL', 'SOLEUR', 8, 5, 2, '0.02', '0.5'], [binance, sol, usdt, 'SOL', 'SOLUSDT', 3, 8, 2, '0.001', '5']]
    listings.each do |exchange, base_asset, quote_asset, base, ticker, base_decimals, quote_decimals, price_decimals, minimum_base_size, minimum_quote_size|
      [base_asset, quote_asset].each { |asset| ExchangeAsset.find_or_create_by!(exchange:, asset:) }
      Ticker.create!(exchange:, base_asset:, quote_asset:, base:, quote: quote_asset.symbol, ticker:, base_decimals:, quote_decimals:, price_decimals:,
                     minimum_base_size:, minimum_quote_size:)
    end
    # 'kraken_usdt' => true: Kraken lists bitcoin and ether in USDT too, so a USDT basket of them can move between the two venues.
    if scenario['kraken_usdt']
      [[btc, 'XBT', 'XBTUSDT', '0.00005'], [eth, 'ETH', 'ETHUSDT', '0.002']].each do |base_asset, base, ticker, minimum_base_size|
        ExchangeAsset.find_or_create_by!(exchange: kraken, asset: usdt)
        Ticker.create!(exchange: kraken, base_asset:, quote_asset: usdt, base:, quote: 'USDT', ticker:, base_decimals: 8, quote_decimals: 5,
                       price_decimals: 2, minimum_base_size:, minimum_quote_size: '5')
      end
    end
    more_coins([[kraken, eur], [binance, usdt]]) if scenario['more_coins']
    keys = scenario.fetch('venue_keys', { 'kraken' => 'correct', 'binance' => 'correct' })
    { 'kraken' => kraken, 'binance' => binance }.each do |name, exchange|
      # The status comes from ApiKey's own list, never straight from scenario data.
      status = ApiKey.statuses.keys.find { |known| known == keys[name] }
      next unless status

      ApiKey.create!(user: User.first, exchange:, key: "#{name}-key", secret: 'c2VjcmV0', status:, key_type: :trading)
    end
  end

  def asset_id(symbol) = Asset.find_by!(symbol:).id

  # The three bots of the owner's instance, as the wizard stores them.
  def bot_settings(kind)
    usd = asset_id('USD')
    case kind
    when 'basket'
      { 'quote_asset_id' => usd, 'quote_amount' => 50, 'interval' => 'day',
        'allocations' => { asset_id('QQQM').to_s => 0.6, asset_id('IBIT').to_s => 0.4 },
        'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.01, 'smart_intervaled' => true, 'smart_interval_quote_amount' => 5 }
    when 'coins'
      { 'quote_asset_id' => usd, 'quote_amount' => 20, 'interval' => 'hour', 'allocations' => { asset_id('BTC').to_s => 0.7, asset_id('ETH').to_s => 0.3 } }
    when 'wide'
      { 'quote_asset_id' => usd, 'quote_amount' => 200, 'interval' => 'month',
        'allocations' => %w[QQQM IBIT NVDA MSFT].to_h { |symbol| [asset_id(symbol).to_s, 0.25] } }
    when 'single'
      { 'quote_asset_id' => usd, 'quote_amount' => 25.5, 'interval' => 'day', 'allocations' => { asset_id('QQQM').to_s => 1.0 },
        'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.005, 'quote_amount_limited' => true, 'quote_amount_limit' => 1000 }
    when 'kraken'
      { 'quote_asset_id' => asset_id('EUR'), 'quote_amount' => 25, 'interval' => 'day', 'allocations' => { asset_id('BTC').to_s => 1.0 } }
    when 'binance'
      { 'quote_asset_id' => asset_id('USDT'), 'quote_amount' => 15.5, 'interval' => 'week', 'allocations' => { asset_id('ETH').to_s => 1.0 } }
    # Baskets on each venue: bitcoin (XBT on Kraken) and ether in EUR, which Alpaca lists too; ether and solana in USDT, which it does not.
    when 'kraken_basket'
      { 'quote_asset_id' => asset_id('EUR'), 'quote_amount' => 40, 'interval' => 'day', 'allocations' => { asset_id('BTC').to_s => 0.6, asset_id('ETH').to_s => 0.4 } }
    when 'binance_basket'
      { 'quote_asset_id' => asset_id('USDT'), 'quote_amount' => 30, 'interval' => 'week', 'allocations' => { asset_id('ETH').to_s => 0.6, asset_id('SOL').to_s => 0.4 } }
    # Crypto index bots (plan CI): Top coins on Kraken in EUR, Layer 1 on Binance in USDT.
    when 'kraken_index'
      { 'quote_asset_id' => asset_id('EUR'), 'quote_amount' => 30, 'interval' => 'day', 'num_coins' => 2, 'hold_all' => false,
        'index_type' => 'top', 'allocation_flattening' => 0.0 }
    when 'binance_index'
      { 'quote_asset_id' => asset_id('USDT'), 'quote_amount' => 40, 'interval' => 'week', 'num_coins' => 2, 'hold_all' => false,
        'index_type' => 'category', 'index_category_id' => 'layer-1', 'index_name' => 'Layer 1 <L1>', 'allocation_flattening' => 0.5 }
    when 'index'
      { 'quote_asset_id' => usd, 'quote_amount' => 100, 'interval' => 'week', 'num_coins' => 10, 'hold_all' => false,
        'index_type' => 'category', 'index_category_id' => 'nasdaq-100', 'index_name' => 'Nasdaq 100', 'index_name_prefix' => 'ND',
        'allocation_flattening' => 0.25 }
    end
  end

  VENUE_ERRORS = {
    'kraken' => ['EAccount:Invalid permissions:XBT trading restricted for US.', 'EAPI:Invalid nonce'],
    'binance' => ['Invalid API-key, IP, or permissions for action.', 'Timestamp for this request is outside of the recvWindow.']
  }.freeze

  VENUE_KINDS = { 'kraken' => 'Kraken', 'binance' => 'Binance', 'kraken_basket' => 'Kraken', 'binance_basket' => 'Binance',
                  'kraken_index' => 'Kraken', 'binance_index' => 'Binance' }.freeze
  # The crypto indices a venue index bot follows, as the syncs store them (Top coins from the image's seed, Layer 1 from
  # CoinGecko), over the venues' own coins.
  CRYPTO_INDICES = { 'top-coins' => ['internal', 'Top Coins', %w[btc eth solana]], 'layer-1' => ['coingecko', 'Layer 1 <L1>', %w[eth solana btc]] }.freeze

  # One bot: 'kind' (basket, single, index, coins, wide; kraken and binance on their venue), then what the scenario says about it.
  # 'settings' and 'transient' are merged over the wizard's; 'stored' is merged into the settings
  # past the validations; 'without' takes settings out of the saved row; 'columns' are written as
  # they are (status, started_at, stop_message_key, label); 'orders' and 'logs' name a history
  # below; 'delist' withdraws a listing afterwards.
  def bot(spec)
    klass = spec['kind'].end_with?('index') ? Bots::DcaIndex : Bots::DcaMultiAsset
    crypto_indices if spec['kind'].end_with?('_index')
    owner = User.find_by!(email: spec.fetch('owner', 'owner@example.com'))
    exchange = Exchange.find_by!(name: spec.fetch('exchange', VENUE_KINDS.fetch(spec['kind'], 'Alpaca')))
    record = klass.new(user: owner, exchange:, settings: bot_settings(spec['kind']).merge(spec.fetch('settings', {})))
    record.set_missed_quote_amount
    record.save!
    index_members(record) if spec['kind'] == 'index'
    crypto_index_members(record) if spec['kind'].end_with?('_index')
    columns = spec.fetch('columns', {}).to_h { |name, value| [name, name.end_with?('_at') && value ? Time.iso8601(value) : value] }
    record.update_columns(columns.merge('transient_data' => record.transient_data.merge(spec.fetch('transient', {}))))
    # What no form would save, written past the validations: a row an older version or a migration left behind.
    record.update_columns(settings: record.settings.merge(spec['stored'])) if spec['stored']
    # A row from before a rule existed. A save through Rails stores every default (each concern's
    # after_initialize), so the keys are taken out of the saved row, past the model.
    record.update_columns(settings: record.settings.except(*spec['without'])) if spec['without']
    orders(record, spec['orders']) if spec['orders']
    logs(record, spec['logs']) if spec['logs']
    # A listing the venue withdrew after the bot was made.
    Ticker.where(exchange:, base: spec['delist']).update_all(available: false) if spec['delist']
    record
  end

  # Action-only additions run at seed time, before either browser signs in.
  def action_fixture(scenario)
    return unless scenario.key?('action_extra')

    extra = scenario.fetch('action_extra')
    record = Bot.find(1)
    ApiKey.delete_all if extra['no_key']
    record.user.update_columns(wash_sale_enabled: false) if extra['wash_off']
    if extra['paid'] || extra['partial']
      attrs = { price: '500', quote_amount: '5', quote_amount_exec: '5', amount: '0.01', amount_exec: '0.01' }
      attrs.merge!(external_status: 'cancelled', price: '125', quote_amount_exec: '1.25', amount: '0.04') if extra['partial']
      Transaction.insert_all!([order_row(record, 'QQQM', '2026-09-10T11:00:01Z', attrs)])
    end
    if extra['exited']
      record.set_missed_quote_amount
      record.update!(allocations: { '2' => 1.0 })
      raise 're-entry fixture lost its exited member' unless BotIndexAsset.exists?(bot_id: 1, asset_id: 3, in_index: false)
    end
    record.update_columns(user_id: User.find_by!(email: 'other@example.com').id) if extra['foreign']
    raise 'CSRF list needs an archived basket' unless Bot.find(2).archived?
  end

  # What Bot::ResyncIndexCompositionJob leaves behind: the first ten index members that trade here,
  # weighted by the index's own weights. Written directly: the job prices every candidate on the venue.
  def crypto_indices
    CRYPTO_INDICES.each do |external_id, (source, name, coins)|
      Index.find_or_create_by!(external_id:, source:) do |index|
        index.assign_attributes(name:, top_coins: coins, weight: 0, available_exchanges: { 'Exchanges::Kraken' => 3, 'Exchanges::Binance' => 3 })
      end
    end
  end

  # The first num_coins coins of its index as members, by market cap, and the next one out of the index (exited a week ago).
  def crypto_index_members(record)
    external_id = record.index_type == 'top' ? 'top-coins' : 'layer-1'
    coins = Index.find_by!(external_id:).top_coins.map { |id| Asset.find_by!(external_id: id) }
    members = coins.first(record.num_coins)
    caps = members.to_h { |asset| [asset.id, asset.market_cap.to_f] }
    weights = Bot::Composition::Weightable.blend(market_caps: caps, flattening: record.allocation_flattening)
    members.each do |asset|
      BotIndexAsset.create!(bot: record, asset:, ticker: record.exchange.tickers.find_by!(base_asset: asset), target_allocation: weights[asset.id].round(6),
                            in_index: true, entered_at: Time.current)
    end
    exited = coins[record.num_coins]
    BotIndexAsset.create!(bot: record, asset: exited, ticker: record.exchange.tickers.find_by!(base_asset: exited), target_allocation: 0.1,
                          in_index: false, entered_at: Time.current - 2.weeks, exited_at: Time.current - 1.week)
  end

  def index_members(record)
    ids = Index.find_by!(external_id: 'nasdaq-100').top_coins.first(record.num_coins)
    assets = Asset.where(external_id: ids).index_by(&:external_id)
    weights = (0...ids.size).map { |position| 20 - position }
    ids.each_with_index do |external_id, position|
      asset = assets.fetch(external_id)
      ticker = record.exchange.tickers.find_by!(base_asset: asset)
      BotIndexAsset.create!(bot: record, asset:, ticker:, target_allocation: (weights[position].to_d / weights.sum).round(6),
                            in_index: true, entered_at: Time.current)
    end
  end

  def order_row(record, symbol, at, attrs)
    asset = Asset.find_by!(symbol:)
    { bot_id: record.id, exchange_id: record.exchange_id, base: symbol, quote: 'USD', base_asset_id: asset.id, quote_asset_id: asset_id('USD'),
      side: 'buy', status: 'submitted', external_status: 'closed', order_type: 'market_order', transaction_type: 'REGULAR',
      bot_interval: record.interval, bot_quote_amount: record.quote_amount, error_messages: [], created_at: Time.iso8601(at),
      updated_at: Time.iso8601(at), external_id: "order-#{record.id}-#{symbol}-#{at}", price: nil, amount: nil, quote_amount: nil, amount_exec: nil,
      quote_amount_exec: nil }.merge(attrs)
  end

  # Buys only, of members of the composition only: what the owner's bots have. 'history' is fourteen
  # rows over two weeks, enough for two pages of the feed, with every kind of row the feed can show.
  def orders(record, name)
    symbol = record.is_a?(Bots::DcaIndex) ? 'NVDA' : 'QQQM'
    rows = case name
           when 'one_fill'
             [order_row(record, symbol, '2026-09-09T13:30:01Z', price: '412.37', amount: '0.121250333', amount_exec: '0.121250333',
                                                                quote_amount: '50', quote_amount_exec: '49.999999')]
           when 'sold'
             [order_row(record, symbol, '2026-09-09T13:30:01Z', side: 'sell', price: '412.37', amount: '0.1', amount_exec: '0.1', quote_amount: nil,
                                                                quote_amount_exec: '41.24')]
           when 'failed'
             [order_row(record, symbol, '2026-09-10T11:59:00Z', status: 'failed', external_status: nil, external_id: nil, amount: nil,
                                                                quote_amount: '25.5', error_messages: ['insufficient buying power', 'forbidden <b>'])]
           when 'open'
             [order_row(record, symbol, '2026-09-10T11:00:00Z', external_status: 'open', order_type: 'limit_order', price: '408.25',
                                                                amount: '0.12247', amount_exec: '0', quote_amount: nil, quote_amount_exec: '0')]
           when 'open_coin'
             # A limit buy of the bot's one coin, resting.
             [order_row(record, 'BTC', '2026-09-10T11:00:00Z', external_status: 'open', order_type: 'limit_order', price: '60000',
                                                               amount: '0.0003', amount_exec: '0', quote_amount: nil, quote_amount_exec: '0')]
           when 'history'
             (1..9).map do |day|
               order_row(record, symbol, "2026-09-0#{day}T13:30:0#{day}Z", price: (400 + day).to_s, amount: "0.12#{day}",
                                                                           amount_exec: "0.12#{day}", quote_amount: '50',
                                                                           quote_amount_exec: ((400 + day) * "0.12#{day}".to_d).to_s)
             end + [
               order_row(record, symbol, '2026-09-09T13:30:09Z', status: 'failed', external_status: nil, external_id: nil, amount: nil,
                                                                 quote_amount: '50', error_messages: ['insufficient buying power']),
               order_row(record, symbol, '2026-09-09T14:00:00Z', status: 'skipped', external_status: nil, external_id: nil, amount: nil,
                                                                 quote_amount: '0.4'),
               order_row(record, symbol, '2026-09-09T15:00:00Z', external_status: 'cancelled', order_type: 'limit_order', price: '399.5',
                                                                 amount: '0.125156', amount_exec: '0', quote_amount_exec: '0'),
               order_row(record, symbol, '2026-09-10T11:00:00Z', external_status: 'open', order_type: 'limit_order', price: '408.25',
                                                                 amount: '0.12247', amount_exec: '0', quote_amount: nil, quote_amount_exec: '0'),
               order_row(record, symbol, '2026-09-10T11:30:00Z', external_status: 'unknown', price: nil, amount: nil, quote_amount: '50',
                                                                 amount_exec: nil, quote_amount_exec: nil)
             ]
           when 'engine_legs'
             # What the Rust engine leaves on a basket mid-run: one filled leg per member, the next run's NVDA leg accepted and
             # not yet swept, MSFT's leg sent and unresolved (its intent names MSFT's own ticker), and the keys only that
             # engine writes, which Rails reads past.
             msft = record.exchange.tickers.find_by!(base: 'MSFT')
             record.update_columns(transient_data: record.transient_data.merge(
               'rust_placement' => { 'cl_ord_id' => '9b1d2c3e-1111-4000-8000-000000000001', 'deadline' => '2026-09-09T13:30:12Z', 'at' => '2026-09-09T13:30:02Z',
                                     'ticker_id' => msft.id, 'base_asset_id' => msft.base_asset_id, 'limit' => false, 'price' => '505.1',
                                     'amount' => '0.098990299', 'quote_amount' => '50.0', 'quote_type' => true, 'volume' => '50.0',
                                     'exchange_id' => record.exchange_id, 'quote_asset_id' => asset_id('USD'), 'allocations' => record.settings['allocations'] },
               'rust_defer_until' => { 'until' => '2026-10-01T10:00:00.000000Z', 'schedule' => 'month' },
               'rust_amount_limit_stops_pending' => { 'count' => 1, 'key' => 'x' },
               'rust_limit_mail_pending' => { 'stamped_at' => '2026-09-09T13:30:01.000Z' }))
             %w[QQQM IBIT NVDA MSFT].each_with_index.map do |sym, i|
               order_row(record, sym, "2026-09-0#{i + 1}T13:30:01Z", price: '400', amount: '0.125', amount_exec: '0.125', quote_amount: '50',
                                                                    quote_amount_exec: '50')
             end + [order_row(record, 'NVDA', '2026-09-09T13:30:01Z', external_status: 'unknown', price: nil, amount: nil, quote_amount: '50',
                                                                      amount_exec: nil, quote_amount_exec: nil)]
           when 'beyond_buys'
             # What a figures page admits: a sale in every state the feed shows, a liquidation, a redeploy leg and a
             # rebalance leg, beside one scheduled buy.
             sell = { side: 'sell', quote_amount: nil }
             [order_row(record, symbol, '2026-09-08T13:30:01Z', price: '410', amount: '0.12', amount_exec: '0.12', quote_amount: '49.2',
                                                                quote_amount_exec: '49.2'),
              order_row(record, symbol, '2026-09-09T13:30:01Z', sell.merge(transaction_type: 'LIQUIDATION', price: '412.37', amount: '0.05',
                                                                           amount_exec: '0.05', quote_amount_exec: '20.6185')),
              order_row(record, symbol, '2026-09-09T14:00:00Z', sell.merge(transaction_type: 'LIQUIDATION', status: 'failed', external_status: nil,
                                                                           external_id: nil, amount: '0.02',
                                                                           error_messages: ['insufficient qty available for order'])),
              order_row(record, symbol, '2026-09-09T15:00:00Z', transaction_type: 'REDEPLOY', price: '411', amount: '0.05', amount_exec: '0.05',
                                                                quote_amount: '20.55', quote_amount_exec: '20.55'),
              order_row(record, symbol, '2026-09-09T16:00:00Z', sell.merge(transaction_type: 'REBALANCE', external_status: 'cancelled',
                                                                           order_type: 'limit_order', price: '420', amount: '0.01', amount_exec: '0',
                                                                           quote_amount_exec: '0')),
              order_row(record, symbol, '2026-09-10T10:00:00Z', sell.merge(external_status: 'open', order_type: 'limit_order', price: '430.5',
                                                                           amount: '0.01', amount_exec: '0', quote_amount_exec: '0')),
              order_row(record, symbol, '2026-09-10T11:00:00Z', sell.merge(transaction_type: 'LIQUIDATION', external_status: 'abandoned',
                                                                           amount: '0.01')),
              order_row(record, symbol, '2026-09-10T11:30:00Z', sell.merge(transaction_type: 'LIQUIDATION', external_status: 'unknown',
                                                                           amount: '0.03'))]
           when 'index_rotation'
             # An index bot's settled special history, as Rails' liquidation, redeploy and rebalance legs write it: a scheduled buy,
             # a liquidation sale, its proceeds redeployed into another member, and a rebalance that sold one member to buy another.
             sell = { side: 'sell', quote_amount: nil }
             [order_row(record, 'NVDA', '2026-09-08T13:30:01Z', price: '410', amount: '0.12', amount_exec: '0.12', quote_amount: '49.2',
                                                                quote_amount_exec: '49.2'),
              order_row(record, 'NVDA', '2026-09-09T13:30:01Z', sell.merge(transaction_type: 'LIQUIDATION', price: '412.5', amount: '0.04',
                                                                           amount_exec: '0.04', quote_amount_exec: '16.5')),
              order_row(record, 'MSFT', '2026-09-09T15:00:00Z', transaction_type: 'REDEPLOY', price: '412.5', amount: '0.04', amount_exec: '0.04',
                                                                quote_amount: '16.5', quote_amount_exec: '16.5'),
              order_row(record, 'NVDA', '2026-09-09T16:00:00Z', sell.merge(transaction_type: 'REBALANCE', price: '415', amount: '0.02',
                                                                           amount_exec: '0.02', quote_amount_exec: '8.3')),
              order_row(record, 'MSFT', '2026-09-09T16:00:05Z', transaction_type: 'REBALANCE', price: '415', amount: '0.02', amount_exec: '0.02',
                                                                quote_amount: '8.3', quote_amount_exec: '8.3')]
           when 'basket_sold'
             # A basket that bought both members and then sold part of one.
             [order_row(record, 'QQQM', '2026-09-08T13:30:01Z', price: '200', amount: '0.15', amount_exec: '0.15', quote_amount: '30',
                                                                quote_amount_exec: '30'),
              order_row(record, 'IBIT', '2026-09-08T13:30:02Z', price: '50', amount: '0.4', amount_exec: '0.4', quote_amount: '20',
                                                                quote_amount_exec: '20'),
              order_row(record, 'QQQM', '2026-09-09T13:30:01Z', side: 'sell', price: '210', amount: '0.05', amount_exec: '0.05', quote_amount: nil,
                                                                quote_amount_exec: '10.5')]
           when 'venue_history'
             # A fill, a failed order and a limit order resting, in the venue's own spelling of the pair.
             ticker = record.exchange.tickers.find_by!(quote_asset_id: record.quote_asset_id, base_asset_id: record.settings['allocations'].keys.first.to_i)
             pair = { base: ticker.base, quote: ticker.quote, base_asset_id: ticker.base_asset_id, quote_asset_id: ticker.quote_asset_id }
             symbol = ticker.base_asset.symbol
             [order_row(record, symbol, '2026-09-08T13:30:01Z', pair.merge(price: '2412.37', amount: '0.0064', amount_exec: '0.0064', quote_amount: '15.5',
                                                                           quote_amount_exec: '15.439168')),
              order_row(record, symbol, '2026-09-09T13:30:01Z', pair.merge(status: 'failed', external_status: nil, external_id: nil, amount: nil,
                                                                           quote_amount: '15.5', error_messages: ['EOrder:Insufficient funds'])),
              order_row(record, symbol, '2026-09-10T11:00:00Z', pair.merge(external_status: 'open', order_type: 'limit_order', price: '2398.5',
                                                                           amount: '0.0064', amount_exec: '0', quote_amount: nil, quote_amount_exec: '0'))]
           when 'venue_basket_history'
             # Each member filled on its own ticker, in the venue's spelling; then a failed leg and a limit leg resting.
             first, second = record.settings['allocations'].keys.map do |id|
               ticker = record.exchange.tickers.find_by!(quote_asset_id: record.quote_asset_id, base_asset_id: id.to_i)
               [ticker.base_asset.symbol, { base: ticker.base, quote: ticker.quote, base_asset_id: ticker.base_asset_id, quote_asset_id: ticker.quote_asset_id }]
             end
             [order_row(record, first[0], '2026-09-08T13:30:01Z', first[1].merge(price: '52000.5', amount: '0.00046', amount_exec: '0.00046',
                                                                               quote_amount: '24', quote_amount_exec: '23.92023')),
              order_row(record, second[0], '2026-09-08T13:30:02Z', second[1].merge(price: '2412.37', amount: '0.0066', amount_exec: '0.0066',
                                                                                quote_amount: '16', quote_amount_exec: '15.921642')),
              order_row(record, first[0], '2026-09-09T13:30:01Z', first[1].merge(status: 'failed', external_status: nil, external_id: nil, amount: nil,
                                                                               quote_amount: '24', error_messages: ['EOrder:Insufficient funds'])),
              order_row(record, second[0], '2026-09-10T11:00:00Z', second[1].merge(external_status: 'open', order_type: 'limit_order', price: '2398.5',
                                                                                amount: '0.0066', amount_exec: '0', quote_amount: nil, quote_amount_exec: '0'))]
           when 'venue_basket_skipped'
             # Both legs under their ticker's minimum: skipped, and their amounts carried.
             record.settings['allocations'].map do |id, weight|
               ticker = record.exchange.tickers.find_by!(quote_asset_id: record.quote_asset_id, base_asset_id: id.to_i)
               order_row(record, ticker.base_asset.symbol, '2026-09-09T13:30:01Z',
                         base: ticker.base, quote: ticker.quote, base_asset_id: ticker.base_asset_id, quote_asset_id: ticker.quote_asset_id, status: 'skipped',
                         external_status: nil, external_id: nil, amount: nil, quote_amount: (record.quote_amount * weight).to_s)
             end
           when 'venue_errors'
             # Two failed orders in the venue's own words, the latest one last: Kraken's regional restriction then its
             # nonce (Honeymaker's own sentences), Binance's key rejection then its recvWindow (the generic buckets).
             ticker = record.exchange.tickers.find_by!(quote_asset_id: record.quote_asset_id, base_asset_id: record.settings['allocations'].keys.first.to_i)
             pair = { base: ticker.base, quote: ticker.quote, base_asset_id: ticker.base_asset_id, quote_asset_id: ticker.quote_asset_id, status: 'failed',
                      external_status: nil, external_id: nil, quote_amount: '15.5' }
             errors = VENUE_ERRORS.fetch(record.exchange.name_id)
             errors.each_with_index.map do |error, index|
               order_row(record, ticker.base_asset.symbol, "2026-09-0#{8 + index}T13:30:01Z", pair.merge(error_messages: [error]))
             end
           else raise "unknown orders #{name}"
           end
    Transaction.insert_all!(rows)
  end

  def logs(record, name)
    rows = case name
           when Array then name
           when 'history'
             [['started', {}, '2026-09-01T09:00:00Z'], ['market_closed', { 'next_market_open_at' => '2026-09-08T13:30:00Z' }, '2026-09-07T13:30:00Z'],
              ['execution_failed', { 'error' => 'insufficient buying power', 'kind' => 'insufficient_funds' }, '2026-09-09T13:30:09Z'],
              ['execution_failed', {}, '2026-09-09T13:31:00Z'], ['order_skipped', {}, '2026-09-09T14:00:00Z'],
              ['orders_below_minimum', { 'count' => 2, 'bases' => 'QQQM, IBIT' }, '2026-09-09T14:00:00Z'],
              ['stopped', { 'stop_message_key' => 'bot.settings.extra_amount_limit.amount_spent' }, '2026-09-09T16:00:00Z'],
              ['stopped', {}, '2026-09-09T17:00:00Z'], ['started', {}, '2026-09-10T08:00:00Z']]
           else raise "unknown logs #{name}"
           end
    BotActivityLog.insert_all!(rows.map do |event, details, at|
      { bot_id: record.id, event:, level: event == 'execution_failed' ? 'error' : 'info', details:, created_at: Time.iso8601(at) }
    end)
  end

  # The job Rails holds for each working bot, enqueued the way Bot::ActionJob's
  # schedule_next_action_job does it: at next_interval_checkpoint_at, read at the scenario's own time.
  # A time instead puts it at that time (a market that opens later, a retry in progress). 'ready' and
  # 'blocked' are a job that is due now: Solid Queue holds the first such job of a venue ready and
  # the others blocked behind it (BotJob's limits_concurrency), and the scenario says which it
  # expects, so that a job in another state than the page was written for fails here.
  def enqueue_jobs(jobs)
    SolidQueue::Job.destroy_all
    SolidQueue::Semaphore.delete_all
    jobs.each do |id, at|
      record = Bot.find(id)
      due = %w[ready blocked].include?(at)
      job = due ? Bot::ActionJob : Bot::ActionJob.set(wait_until: at == 'checkpoint' ? record.next_interval_checkpoint_at : Time.iso8601(at))
      enqueued = job.perform_later(record)
      next unless due

      held = SolidQueue::Job.find_by!(active_job_id: enqueued.job_id)
      state = %w[ready blocked scheduled claimed].find { |name| held.public_send("#{name}_execution") }
      raise "bot #{id}: its job is #{state.inspect}, and the scenario says #{at}" unless state == at
    end
  end
end
