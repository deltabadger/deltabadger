# The Rails half of the venue-catalogue parity (rust/tests/catalogue.rs, PLAN-A17 A17-2): each crypto venue's real
# `get_tickers_info(force: true)` against scripted venue answers, served beneath honeymaker's Faraday adapter, so
# honeymaker's middleware (json, raise_error) and Client#with_rescue run as in production. No real connection is made.
#   bin/rails runner script/rust/catalogue_vectors.rb [rust/tests/fixtures/catalogue_vectors.json]
# Run with every *_DATABASE_URL pointing at scratch files (as rust/tests/common rails() does) and APP_ROOT_URL set;
# script/rust/repin re-runs it when a pinned source or the honeymaker version moves.
# Writes the fixture: `sources` (sha256 of the Rails files the rows come from), `gems` (honeymaker's version, checked
# against Gemfile.lock), `honeymaker_files` (sha256 of the honeymaker files this process loaded, for the record), and per scenario the script ("METHOD /path?query" => replies, the key
# rust/src/venue/http.rs's ScriptedTransport answers by), the requests Rails made (method, host, path, query, body, and
# `at`: the seconds Rails had slept before sending it) and Rails' result (rows, or the failure's errors and sentence).
require 'json'
require 'tmpdir'
require 'digest'
require_relative 'schema_template'

module Harness
  # An Exception, not a StandardError: with_rescue would turn a harness gap into an ordinary Failure.
  class Unscripted < Exception; end # rubocop:disable Lint/InheritException
end

# The backstop: no real Net::HTTP connection, ever.
Net::HTTP.prepend(Module.new { def connect = raise(Harness::Unscripted, "real connection to #{address}:#{port}") })

module ScriptedVenue
  mattr_accessor :script, :requests, :slept

  def self.query(env) = URI.decode_www_form(env.url.query.to_s).sort

  def self.key(env)
    q = query(env).map { |k, v| "#{k}=#{v}" }.join('&')
    ["#{env.method.to_s.upcase} #{env.url.path}", q].reject(&:empty?).join('?')
  end

  module Adapter
    def call(env)
      raise Harness::Unscripted, "unscripted HTTP call #{env.url}" unless ScriptedVenue.script

      body = env.body.to_s
      ScriptedVenue.requests << { 'method' => env.method.to_s.upcase, 'host' => "#{env.url.scheme}://#{env.url.host}",
                                  'path' => env.url.path, 'query' => ScriptedVenue.query(env),
                                  'body' => body.empty? ? nil : JSON.parse(body), 'at' => ScriptedVenue.slept }
      key = ScriptedVenue.key(env)
      queue = ScriptedVenue.script[key] or raise Harness::Unscripted, "unscripted venue call #{key}"
      reply = queue.size > 1 ? queue.shift : queue.first
      body = reply['body'].is_a?(String) ? reply['body'] : JSON.generate(reply['body'])
      env.response = Faraday::Response.new
      save_response(env, reply.fetch('status'), body, { 'Content-Type' => 'application/json' })
      @app.call(env)
    end
  end
end

module Catalogue
  module_function

  NAMES = { 'Exchanges::Binance' => 'Binance', 'Exchanges::BinanceUs' => 'Binance.US', 'Exchanges::Bingx' => 'BingX',
            'Exchanges::Bitget' => 'Bitget', 'Exchanges::Bitrue' => 'Bitrue', 'Exchanges::Bitvavo' => 'Bitvavo',
            'Exchanges::Bybit' => 'Bybit', 'Exchanges::Coinbase' => 'Coinbase', 'Exchanges::Gemini' => 'Gemini',
            'Exchanges::Hyperliquid' => 'Hyperliquid', 'Exchanges::Kraken' => 'Kraken', 'Exchanges::Kucoin' => 'KuCoin',
            'Exchanges::Mexc' => 'MEXC' }.freeze

  def ok(body) = { 'status' => 200, 'body' => body }
  def error(status, body) = { 'status' => status, 'body' => body }

  # Binance's filters as /api/v3/exchangeInfo lists them.
  def binance_symbol(symbol, base, quote, status: 'TRADING', notional: 'NOTIONAL', step: '0.00001000', tick: '0.01000000')
    notional_filter = { 'filterType' => notional, 'minNotional' => '5.00000000' }
    notional_filter['maxNotional'] = '9000000.00000000' if notional == 'NOTIONAL'
    { 'symbol' => symbol, 'status' => status, 'baseAsset' => base, 'baseAssetPrecision' => 8, 'quoteAsset' => quote,
      'quotePrecision' => 8, 'quoteAssetPrecision' => 8, 'orderTypes' => %w[LIMIT MARKET], 'isSpotTradingAllowed' => true,
      'filters' => [{ 'filterType' => 'PRICE_FILTER', 'minPrice' => '0.01000000', 'maxPrice' => '1000000.00000000', 'tickSize' => tick },
                    { 'filterType' => 'LOT_SIZE', 'minQty' => step, 'maxQty' => '9000.00000000', 'stepSize' => step },
                    notional_filter],
      'permissions' => [], 'permissionSets' => [['SPOT']] }
  end

  def binance_info(*symbols) = { 'timezone' => 'UTC', 'serverTime' => 1_761_000_000_000, 'rateLimits' => [], 'symbols' => symbols }

  def gemini_detail(symbol, base, quote, extra = {})
    { 'symbol' => symbol.upcase, 'base_currency' => base, 'quote_currency' => quote, 'tick_size' => 1e-08,
      'quote_increment' => 0.01, 'min_order_size' => '0.00001', 'status' => 'open', 'wrap_enabled' => false,
      'product_type' => 'spot', 'contract_type' => 'vanilla', 'contract_price_currency' => quote }.merge(extra)
  end

  def kraken_pair(altname, wsname, base, quote, extra = {})
    { 'altname' => altname, 'wsname' => wsname, 'aclass_base' => 'currency', 'base' => base, 'aclass_quote' => 'currency',
      'quote' => quote, 'lot' => 'unit', 'cost_decimals' => 5, 'pair_decimals' => 1, 'lot_decimals' => 8,
      'lot_multiplier' => 1, 'ordermin' => '0.00005', 'costmin' => '0.5', 'tick_size' => '0.1', 'status' => 'online' }.merge(extra)
  end

  def scenarios
    gemini_symbols = '/v1/symbols'
    [
      { 'name' => 'binance_normal', 'venue' => 'Exchanges::Binance', 'script' => {
        'GET /api/v3/exchangeInfo?permissions=["SPOT"]' => [ok(binance_info(
          binance_symbol('BTCUSDT', 'BTC', 'USDT'),
          binance_symbol('ETHBTC', 'ETH', 'BTC', status: 'BREAK', notional: 'MIN_NOTIONAL', step: '0.00010000', tick: '0.00001000'),
          binance_symbol('PEPEUSDT', 'PEPE', 'USDT', step: '1.00000000', tick: '0.00000001')
        ))]
      } },
      { 'name' => 'binance_us_normal', 'venue' => 'Exchanges::BinanceUs', 'script' => {
        'GET /api/v3/exchangeInfo?permissions=["SPOT"]' => [ok(binance_info(binance_symbol('BTCUSD', 'BTC', 'USD', tick: '0.01000000')))]
      } },
      { 'name' => 'binance_error', 'venue' => 'Exchanges::Binance', 'script' => {
        'GET /api/v3/exchangeInfo?permissions=["SPOT"]' => [error(400, { 'code' => -1121, 'msg' => 'Invalid symbol.' })]
      } },
      { 'name' => 'bingx_normal', 'venue' => 'Exchanges::Bingx', 'script' => {
        'GET /openApi/spot/v1/common/symbols' => [ok({ 'code' => 0, 'msg' => '', 'debugMsg' => '', 'data' => { 'symbols' => [
          { 'symbol' => 'BTC-USDT', 'minQty' => 0.0001, 'maxQty' => 100, 'minNotional' => 0.5, 'maxNotional' => 20_000,
            'status' => 1, 'tickSize' => 0.01, 'stepSize' => 0.00001, 'apiStateSell' => true, 'apiStateBuy' => true, 'timeOnline' => 0 },
          { 'symbol' => 'ETH-USDT', 'minNotional' => 0.5, 'maxNotional' => 20_000, 'status' => 0, 'tickSize' => 0.01, 'stepSize' => 0.0001 },
          { 'symbol' => 'NODASH', 'minNotional' => 1, 'maxNotional' => 1, 'status' => 1, 'tickSize' => 0.1, 'stepSize' => 1 }
        ] } })]
      } },
      # D-CG-DECIMALS-EXPONENT: BingX answers numbers, and Float#to_s writes 1e-06 as "1.0e-06" (Rails counts 5 places).
      { 'name' => 'bingx_exponent_steps', 'venue' => 'Exchanges::Bingx', 'script' => {
        'GET /openApi/spot/v1/common/symbols' => [ok({ 'code' => 0, 'msg' => '', 'data' => { 'symbols' => [
          { 'symbol' => 'SHIB-USDT', 'minNotional' => 0.5, 'maxNotional' => 20_000, 'status' => 1, 'tickSize' => 1e-08, 'stepSize' => 1e-06 },
          { 'symbol' => 'PEPE-USDT', 'minNotional' => 0.5, 'maxNotional' => 20_000, 'status' => 1, 'tickSize' => 1.5e-11, 'stepSize' => 1e-05 },
          { 'symbol' => 'DOGE-USDT', 'minNotional' => 0.5, 'maxNotional' => 20_000, 'status' => 1, 'tickSize' => '1e-7', 'stepSize' => 0.0001 }
        ] } })]
      } },
      { 'name' => 'bingx_code_error', 'venue' => 'Exchanges::Bingx', 'script' => {
        'GET /openApi/spot/v1/common/symbols' => [ok({ 'code' => 100_410, 'msg' => 'rate limited', 'data' => {} })]
      } },
      { 'name' => 'bitget_normal', 'venue' => 'Exchanges::Bitget', 'script' => {
        'GET /api/v2/spot/public/symbols' => [ok({ 'code' => '00000', 'msg' => 'success', 'requestTime' => 1, 'data' => [
          { 'symbol' => 'BTCUSDT', 'baseCoin' => 'BTC', 'quoteCoin' => 'USDT', 'minTradeAmount' => '0', 'maxTradeAmount' => '900000000',
            'takerFeeRate' => '0.001', 'makerFeeRate' => '0.001', 'pricePrecision' => '2', 'quantityPrecision' => '6',
            'quotePrecision' => '8', 'status' => 'online', 'minTradeUSDT' => '1' },
          { 'symbol' => 'ETHBTC', 'baseCoin' => 'ETH', 'quoteCoin' => 'BTC', 'minTradeAmount' => '0.0001', 'maxTradeAmount' => '10000',
            'pricePrecision' => '6', 'quantityPrecision' => '4', 'quotePrecision' => '8', 'status' => 'online', 'minTradeUSDT' => '1' },
          { 'symbol' => 'BTCEUR', 'baseCoin' => 'BTC', 'quoteCoin' => 'EUR', 'minTradeAmount' => '0', 'pricePrecision' => '2',
            'quantityPrecision' => '6', 'quotePrecision' => '2', 'status' => 'halt', 'minTradeUSDT' => '5' },
          { 'symbol' => 'FOOBAR', 'baseCoin' => 'FOO', 'quoteCoin' => 'BAR', 'minTradeAmount' => '1', 'pricePrecision' => 4,
            'quantityPrecision' => 2, 'quotePrecision' => 4, 'status' => 'gray', 'minTradeUSDT' => '1' },
          { 'symbol' => 'XUSDT', 'baseCoin' => 'X', 'quoteCoin' => 'USDT', 'minTradeAmount' => '1', 'pricePrecision' => '4',
            'quantityPrecision' => '2', 'quotePrecision' => '4', 'status' => 'online' }
        ] })],
        'GET /api/v2/spot/market/tickers' => [ok({ 'code' => '00000', 'msg' => 'success', 'data' => [
          { 'symbol' => 'BTCUSDT', 'lastPr' => '60000', 'bidPr' => '59999' },
          { 'symbol' => 'USDTEUR', 'lastPr' => '0.92' },
          { 'symbol' => 'BTCUSDT', 'lastPr' => '61000' }
        ] })]
      } },
      { 'name' => 'bitget_price_failure', 'venue' => 'Exchanges::Bitget', 'script' => {
        'GET /api/v2/spot/public/symbols' => [ok({ 'code' => '00000', 'data' => [
          { 'symbol' => 'ETHBTC', 'baseCoin' => 'ETH', 'quoteCoin' => 'BTC', 'minTradeAmount' => '0.0001', 'pricePrecision' => '6',
            'quantityPrecision' => '4', 'quotePrecision' => '8', 'status' => 'online', 'minTradeUSDT' => '1' },
          { 'symbol' => 'BTCUSDT', 'baseCoin' => 'BTC', 'quoteCoin' => 'USDT', 'minTradeAmount' => '0', 'pricePrecision' => '2',
            'quantityPrecision' => '6', 'quotePrecision' => '8', 'status' => 'online', 'minTradeUSDT' => '1' }
        ] })],
        'GET /api/v2/spot/market/tickers' => [error(500, { 'code' => '40015', 'msg' => 'system error' })]
      } },
      { 'name' => 'bitrue_normal', 'venue' => 'Exchanges::Bitrue', 'script' => {
        'GET /api/v1/exchangeInfo' => [ok({ 'timezone' => 'CTT', 'symbols' => [
          { 'symbol' => 'BTCUSDT', 'status' => 'TRADING', 'baseAsset' => 'btc', 'baseAssetPrecision' => 5, 'quoteAsset' => 'usdt',
            'quotePrecision' => 2, 'filters' => [
              { 'filterType' => 'PRICE_FILTER', 'minPrice' => '0.01', 'maxPrice' => '1000000', 'tickSize' => '0.01' },
              { 'filterType' => 'LOT_SIZE', 'minQty' => '0.00001', 'minVal' => '5', 'maxQty' => '1000', 'stepSize' => '0.00001' }
            ] },
          { 'symbol' => 'XRPUSDT', 'status' => 'HALT', 'baseAsset' => 'xrp', 'baseAssetPrecision' => 1, 'quoteAsset' => 'usdt',
            'quotePrecision' => 5 },
          { 'symbol' => 'ETHUSDT', 'status' => 'TRADING', 'baseAsset' => 'eth', 'baseAssetPrecision' => 4, 'quoteAsset' => 'usdt',
            'quotePrecision' => 2, 'filters' => [{ 'filterType' => 'PRICE_FILTER', 'tickSize' => '0.10' }] }
        ] })]
      } },
      { 'name' => 'bitvavo_normal', 'venue' => 'Exchanges::Bitvavo', 'script' => {
        'GET /v2/markets' => [ok([
          { 'market' => 'BTC-EUR', 'status' => 'trading', 'base' => 'BTC', 'quote' => 'EUR', 'pricePrecision' => 5, 'tickSize' => '1',
            'minOrderInBaseAsset' => '0.000061', 'minOrderInQuoteAsset' => '5', 'maxOrderInBaseAsset' => '1000000000',
            'maxOrderInQuoteAsset' => '1000000000', 'quantityDecimals' => 8, 'notionalDecimals' => 2, 'orderTypes' => %w[market limit] },
          { 'market' => 'ETH-EUR', 'status' => 'halted', 'tickSize' => '0.01', 'minOrderInBaseAsset' => '0.001',
            'minOrderInQuoteAsset' => '5', 'quantityDecimals' => 6, 'notionalDecimals' => 2 },
          { 'market' => 'SHIB-EUR', 'status' => 'auction', 'tickSize' => '0.000000001', 'minOrderInBaseAsset' => '100000',
            'minOrderInQuoteAsset' => '5' }
        ])]
      } },
      { 'name' => 'bybit_normal', 'venue' => 'Exchanges::Bybit', 'script' => {
        'GET /v5/market/instruments-info?category=spot' => [ok({ 'retCode' => 0, 'retMsg' => 'OK', 'result' => { 'category' => 'spot', 'list' => [
          { 'symbol' => 'BTCUSDT', 'baseCoin' => 'BTC', 'quoteCoin' => 'USDT', 'innovation' => '0', 'status' => 'Trading',
            'marginTrading' => 'both', 'lotSizeFilter' => { 'basePrecision' => '0.000001', 'quotePrecision' => '0.00000001',
                                                             'minOrderQty' => '0.000048', 'maxOrderQty' => '71.73956243',
                                                             'minOrderAmt' => '1', 'maxOrderAmt' => '2000000' },
            'priceFilter' => { 'tickSize' => '0.01' } },
          { 'symbol' => 'NEWUSDT', 'baseCoin' => 'NEW', 'quoteCoin' => 'USDT', 'status' => 'PreLaunch',
            'lotSizeFilter' => { 'basePrecision' => '0.01', 'quotePrecision' => '0.000001', 'minOrderQty' => '1',
                                 'maxOrderQty' => '1000000', 'minOrderAmt' => '5', 'maxOrderAmt' => '100000' },
            'priceFilter' => { 'tickSize' => '0.0001' } }
        ] }, 'retExtInfo' => {}, 'time' => 1 })]
      } },
      { 'name' => 'bybit_error', 'venue' => 'Exchanges::Bybit', 'script' => {
        'GET /v5/market/instruments-info?category=spot' => [ok({ 'retCode' => 10_001, 'retMsg' => 'params error', 'result' => {} })]
      } },
      { 'name' => 'coinbase_flags_and_blacklist', 'venue' => 'Exchanges::Coinbase', 'script' => {
        'GET /api/v3/brokerage/market/products' => [ok({ 'num_products' => 8, 'products' => [
          coinbase_product('BTC-USD'),
          coinbase_product('ETH-USD', 'trading_disabled' => true),
          coinbase_product('SOL-USD', 'limit_only' => true),
          coinbase_product('ADA-USD', 'status' => 'delisted'),
          coinbase_product('RNDR-USD', 'status' => 'online'),
          coinbase_product('XRP-USD', 'cancel_only' => true, 'status' => 'online'),
          coinbase_product('DOGE-USD', 'is_disabled' => true),
          coinbase_product('USDC-EUR', 'status' => 'online', 'trading_disabled' => false)
        ] })]
      } },
      { 'name' => 'coinbase_failure', 'venue' => 'Exchanges::Coinbase', 'script' => {
        'GET /api/v3/brokerage/market/products' => [error(503, 'upstream unavailable')]
      } },
      { 'name' => 'gemini_normal', 'venue' => 'Exchanges::Gemini', 'script' => {
        "GET #{gemini_symbols}" => [ok(%w[btcusd ethbtc btcgusdperp solusd])],
        'GET /v1/symbols/details/btcusd' => [ok(gemini_detail('btcusd', 'BTC', 'USD', 'tick_size' => 1e-08, 'quote_increment' => 0.01))],
        'GET /v1/symbols/details/ethbtc' => [ok(gemini_detail('ethbtc', 'ETH', 'BTC', 'status' => 'closed', 'tick_size' => 1e-06,
                                                              'quote_increment' => 1e-05, 'min_order_size' => '0.001'))],
        'GET /v1/symbols/details/btcgusdperp' => [ok(gemini_detail('btcgusdperp', 'BTC', 'GUSD', 'product_type' => 'swap'))],
        'GET /v1/symbols/details/solusd' => [ok(gemini_detail('solusd', 'sol', 'usd', 'min_order_size' => '0.01').except('tick_size', 'quote_increment'))]
      } },
      { 'name' => 'gemini_retry_success', 'venue' => 'Exchanges::Gemini', 'script' => {
        "GET #{gemini_symbols}" => [error(503, { 'result' => 'error', 'reason' => 'System', 'message' => 'busy' }), ok(%w[btcusd])],
        'GET /v1/symbols/details/btcusd' => [error(429, { 'result' => 'error', 'reason' => 'RateLimited', 'message' => 'slow down' }),
                                             error(502, 'bad gateway'), ok(gemini_detail('btcusd', 'BTC', 'USD'))]
      } },
      { 'name' => 'gemini_detail_failure', 'venue' => 'Exchanges::Gemini', 'script' => {
        "GET #{gemini_symbols}" => [ok(%w[btcusd ethusd])],
        'GET /v1/symbols/details/btcusd' => [error(503, { 'result' => 'error', 'reason' => 'Maintenance', 'message' => 'down' })]
      } },
      { 'name' => 'gemini_detail_refused', 'venue' => 'Exchanges::Gemini', 'script' => {
        "GET #{gemini_symbols}" => [ok(%w[btcusd ethusd])],
        'GET /v1/symbols/details/btcusd' => [ok(gemini_detail('btcusd', 'BTC', 'USD'))],
        'GET /v1/symbols/details/ethusd' => [error(400, { 'result' => 'error', 'reason' => 'InvalidSymbol', 'message' => 'no such symbol' })]
      } },
      { 'name' => 'gemini_missing_product_type', 'venue' => 'Exchanges::Gemini', 'script' => {
        "GET #{gemini_symbols}" => [ok(%w[btcusd ethusd])],
        'GET /v1/symbols/details/btcusd' => [ok(gemini_detail('btcusd', 'BTC', 'USD').except('product_type'))]
      } },
      { 'name' => 'gemini_duplicate_pair', 'venue' => 'Exchanges::Gemini', 'script' => {
        "GET #{gemini_symbols}" => [ok(%w[btcusd ethusd btcusd2])],
        'GET /v1/symbols/details/btcusd' => [ok(gemini_detail('btcusd', 'BTC', 'USD'))],
        'GET /v1/symbols/details/ethusd' => [ok(gemini_detail('ethusd', 'ETH', 'USD'))],
        'GET /v1/symbols/details/btcusd2' => [ok(gemini_detail('btcusd2', 'btc', 'usd'))]
      } },
      # The plan's D-CG-DECIMALS-EXPONENT example: Gemini passes `tick_size.to_d.to_s`, which ActiveSupport writes in plain
      # notation ("0.00000001"), so Rails reads these as Rust does.
      { 'name' => 'gemini_exponent_ticks', 'venue' => 'Exchanges::Gemini', 'script' => {
        "GET #{gemini_symbols}" => [ok(%w[shibusd pepeusd])],
        'GET /v1/symbols/details/shibusd' => [ok(gemini_detail('shibusd', 'SHIB', 'USD', 'tick_size' => 1, 'quote_increment' => 1e-09,
                                                               'min_order_size' => '1000'))],
        'GET /v1/symbols/details/pepeusd' => [ok(gemini_detail('pepeusd', 'PEPE', 'USD', 'tick_size' => '1e-8', 'quote_increment' => 1.5e-12,
                                                               'min_order_size' => 100_000))]
      } },
      { 'name' => 'hyperliquid_tokens_by_index', 'venue' => 'Exchanges::Hyperliquid', 'script' => {
        'POST /info' => [ok({ 'tokens' => [
          { 'name' => 'PURR', 'szDecimals' => 0, 'weiDecimals' => 5, 'index' => 1, 'tokenId' => '0xc1fb', 'isCanonical' => true },
          { 'name' => 'HYPE', 'szDecimals' => 2, 'weiDecimals' => 8, 'index' => 150, 'tokenId' => '0x0d01', 'isCanonical' => false },
          { 'name' => 'USDC', 'szDecimals' => 8, 'weiDecimals' => 8, 'index' => 0, 'tokenId' => '0x6d1e', 'isCanonical' => true },
          { 'name' => 'NOSZ', 'weiDecimals' => 8, 'index' => 7 }
        ], 'universe' => [
          { 'tokens' => [1, 0], 'name' => 'PURR/USDC', 'index' => 0, 'isCanonical' => true },
          { 'tokens' => [150, 0], 'name' => '@107', 'index' => 107, 'isCanonical' => false },
          { 'tokens' => [999, 0], 'name' => '@108', 'index' => 108, 'isCanonical' => false },
          { 'tokens' => [7, 0], 'name' => '@109', 'index' => 109, 'isCanonical' => false }
        ] })]
      } },
      { 'name' => 'hyperliquid_failure', 'venue' => 'Exchanges::Hyperliquid', 'script' => {
        'POST /info' => [error(500, 'null')]
      } },
      { 'name' => 'kraken_pairs', 'venue' => 'Exchanges::Kraken', 'script' => {
        'GET /0/public/AssetPairs?aclass_base=all' => [ok({ 'error' => [], 'result' => {
          'XXBTZUSD' => kraken_pair('XBTUSD', 'XBT/USD', 'XXBT', 'ZUSD', 'cost_decimals' => 5, 'pair_decimals' => 1),
          'XETHXXBT' => kraken_pair('ETHXBT', 'ETH/XBT', 'XETH', 'XXBT', 'costmin' => '0.00002', 'pair_decimals' => 5),
          'DOTUSDG' => kraken_pair('DOTUSDG', 'DOT/USDG', 'DOT', 'USDG', 'costmin' => '1', 'status' => 'reduce_only'),
          'ADAXAUT' => kraken_pair('ADAXAUT', 'ADA/XAUT', 'ADA', 'XAUT').except('costmin', 'status'),
          'XBTETH.SPV' => kraken_pair('AAPLxUSD', 'AAPLx/USD', 'AAPLx', 'ZUSD', 'aclass_base' => 'tokenized_asset', 'costmin' => '1',
                                                                                'ordermin' => '0.01'),
          'AAPLxUSD' => kraken_pair('AAPLxUSD', 'AAPLx/USD', 'AAPLx', 'ZUSD', 'aclass_base' => 'tokenized_asset', 'ordermin' => '0.02'),
          'BLANKWS' => kraken_pair('BLANK', ' ', 'BL', 'ZUSD'),
          'NOWS' => kraken_pair('NOWS', nil, 'NO', 'ZUSD'),
          'EURQEUR' => kraken_pair('EURQEUR', 'EURQ/EUR', 'EURQ', 'ZEUR', 'costmin' => '0.45'),
          'ETHJPY' => kraken_pair('ETHJPY', 'ETH/JPY', 'XETH', 'ZJPY', 'costmin' => '50')
        } })]
      } },
      { 'name' => 'kraken_error', 'venue' => 'Exchanges::Kraken', 'script' => {
        'GET /0/public/AssetPairs?aclass_base=all' => [ok({ 'error' => ['EService:Unavailable', 'EGeneral:Temporary lockout'] })]
      } },
      { 'name' => 'kucoin_normal', 'venue' => 'Exchanges::Kucoin', 'script' => {
        'GET /api/v2/symbols' => [ok({ 'code' => '200000', 'data' => [
          { 'symbol' => 'BTC-USDT', 'name' => 'BTC-USDT', 'baseCurrency' => 'BTC', 'quoteCurrency' => 'USDT', 'feeCurrency' => 'USDT',
            'market' => 'USDS', 'baseMinSize' => '0.00001', 'quoteMinSize' => '0.1', 'baseMaxSize' => '10000000000',
            'quoteMaxSize' => '99999999', 'baseIncrement' => '0.00000001', 'quoteIncrement' => '0.000001',
            'priceIncrement' => '0.1', 'priceLimitRate' => '0.1', 'minFunds' => '0.1', 'isMarginEnabled' => true,
            'enableTrading' => true },
          { 'symbol' => 'ETH-BTC', 'baseCurrency' => 'ETH', 'quoteCurrency' => 'BTC', 'baseMinSize' => '0.0001',
            'quoteMinSize' => '0.000001', 'baseMaxSize' => '10000000000', 'quoteMaxSize' => '99999999',
            'baseIncrement' => '0.0000001', 'quoteIncrement' => '0.00000001', 'priceIncrement' => '0.000001', 'enableTrading' => false }
        ] })]
      } },
      { 'name' => 'kucoin_no_data', 'venue' => 'Exchanges::Kucoin', 'script' => {
        'GET /api/v2/symbols' => [ok({ 'code' => '400100', 'msg' => 'busy' })]
      } },
      { 'name' => 'mexc_normal', 'venue' => 'Exchanges::Mexc', 'script' => {
        'GET /api/v3/exchangeInfo' => [ok({ 'timezone' => 'CST', 'serverTime' => 1, 'symbols' => [
          { 'symbol' => 'BTCUSDT', 'status' => '1', 'baseAsset' => 'BTC', 'baseAssetPrecision' => 6, 'quoteAsset' => 'USDT',
            'quotePrecision' => 2, 'quoteAssetPrecision' => 2, 'baseCommissionPrecision' => 6, 'quoteCommissionPrecision' => 2,
            'orderTypes' => %w[LIMIT MARKET], 'isSpotTradingAllowed' => true, 'isMarginTradingAllowed' => false,
            'quoteAmountPrecision' => '1', 'baseSizePrecision' => '0.000001', 'permissions' => ['SPOT'], 'filters' => [],
            'maxQuoteAmount' => '2000000', 'makerCommission' => '0', 'takerCommission' => '0.0005',
            'quoteAmountPrecisionMarket' => '5', 'maxQuoteAmountMarket' => '100000', 'fullName' => 'Bitcoin', 'tradeSideType' => 1 },
          { 'symbol' => 'XYZUSDT', 'status' => '1', 'baseAsset' => 'XYZ', 'baseAssetPrecision' => 2, 'quoteAsset' => 'USDT',
            'quotePrecision' => 6, 'quoteAssetPrecision' => 6, 'isSpotTradingAllowed' => false, 'quoteAmountPrecision' => '1',
            'baseSizePrecision' => '0', 'filters' => [], 'maxQuoteAmount' => '' },
          { 'symbol' => 'ETHUSDC', 'status' => '2', 'baseAsset' => 'ETH', 'baseAssetPrecision' => 4, 'quoteAsset' => 'USDC',
            'quotePrecision' => 2, 'quoteAssetPrecision' => 2, 'filters' => [
              { 'filterType' => 'LOT_SIZE', 'minQty' => '0.0001', 'maxQty' => '9000', 'stepSize' => '0.0001' },
              { 'filterType' => 'MIN_NOTIONAL', 'minNotional' => '1' },
              { 'filterType' => 'PRICE_FILTER', 'tickSize' => '0.01' }
            ] }
        ] })]
      } },
      { 'name' => 'mexc_error', 'venue' => 'Exchanges::Mexc', 'script' => {
        'GET /api/v3/exchangeInfo' => [error(503, 'Service Unavailable')]
      } },
      # String#to_d reads a malformed size by its leading number, else 0; Infinity and NaN are not finite (as_json nil).
      { 'name' => 'kucoin_malformed_sizes', 'venue' => 'Exchanges::Kucoin', 'script' => {
        'GET /api/v2/symbols' => [ok({ 'code' => '200000', 'data' => [
          { 'symbol' => 'BTC-USDT', 'baseCurrency' => 'BTC', 'quoteCurrency' => 'USDT', 'baseMinSize' => '12abc',
            'quoteMinSize' => 'garbage', 'baseMaxSize' => ' 1_000.5x', 'quoteMaxSize' => '-3.5e2z', 'baseIncrement' => '0.00000001',
            'quoteIncrement' => '0.000001', 'priceIncrement' => '0.1', 'enableTrading' => true },
          { 'symbol' => 'ETH-USDT', 'baseCurrency' => 'ETH', 'quoteCurrency' => 'USDT', 'baseMinSize' => '.5',
            'quoteMinSize' => '5.', 'baseMaxSize' => '1__0', 'quoteMaxSize' => '1e', 'baseIncrement' => '0.0001',
            'quoteIncrement' => '0.01', 'priceIncrement' => '0.01', 'enableTrading' => true },
          { 'symbol' => 'SOL-USDT', 'baseCurrency' => 'SOL', 'quoteCurrency' => 'USDT', 'baseMinSize' => '1.2.3',
            'quoteMinSize' => '1,5', 'baseMaxSize' => 'Infinity', 'quoteMaxSize' => 'NaN', 'baseIncrement' => '0.01',
            'quoteIncrement' => '0.01', 'priceIncrement' => '0.001', 'enableTrading' => true },
          { 'symbol' => 'XRP-USDT', 'baseCurrency' => 'XRP', 'quoteCurrency' => 'USDT', 'baseMinSize' => '+7',
            'quoteMinSize' => '0x1A', 'baseMaxSize' => '1e-3x', 'quoteMaxSize' => '  ', 'baseIncrement' => '0.1',
            'quoteIncrement' => '0.0001', 'priceIncrement' => '0.0001', 'enableTrading' => true }
        ] })]
      } },
      # A malformed USDT minimum, converted at a price.
      { 'name' => 'bitget_malformed_minimum', 'venue' => 'Exchanges::Bitget', 'script' => {
        'GET /api/v2/spot/public/symbols' => [ok({ 'code' => '00000', 'data' => [
          { 'symbol' => 'BTCEUR', 'baseCoin' => 'BTC', 'quoteCoin' => 'EUR', 'minTradeAmount' => '0.0001abc', 'pricePrecision' => '2',
            'quantityPrecision' => '6', 'quotePrecision' => '2', 'status' => 'online', 'minTradeUSDT' => '2usd', 'maxTradeAmount' => 'x' }
        ] })],
        'GET /api/v2/spot/market/tickers' => [ok({ 'code' => '00000', 'data' => [{ 'symbol' => 'USDTEUR', 'lastPr' => '0.92 EUR' }] })]
      } }
    ]
  end

  SOURCES = (%w[binance binance_us bingx bitget bitrue bitvavo bybit coinbase gemini hyperliquid kraken kucoin mexc]
               .map { |v| "app/models/exchanges/#{v}.rb" } +
             %w[app/models/exchange.rb app/models/utilities/number.rb app/models/utilities/hash.rb app/services/result.rb
                script/rust/catalogue_vectors.rb]).freeze

  def pins = SOURCES.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }

  # Every honeymaker Ruby file this process loaded (its clients, Client, Result), by path inside the gem.
  def honeymaker_files
    root = Gem.loaded_specs.fetch('honeymaker').full_gem_path
    $LOADED_FEATURES.select { |f| f.start_with?(root) && f.end_with?('.rb') }.sort
                    .to_h { |f| [f.delete_prefix("#{root}/"), Digest::SHA256.file(f).hexdigest] }
  end

  def coinbase_product(id, extra = {})
    base, quote = id.split('-')
    { 'product_id' => id, 'price' => '1.5', 'base_increment' => '0.00000001', 'quote_increment' => '0.01',
      'quote_min_size' => '1', 'quote_max_size' => '150000000', 'base_min_size' => '0.00000001', 'base_max_size' => '3400',
      'base_name' => base, 'quote_name' => quote, 'price_increment' => '0.01', 'product_type' => 'SPOT',
      'base_currency_id' => base, 'quote_currency_id' => quote }.merge(extra)
  end

  def record
    Rails.cache = ActiveSupport::Cache::MemoryStore.new
    dir = Dir.mktmpdir('catalogue-vectors')
    SchemaTemplate.install(dir)
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, SchemaTemplate::PRIMARY))
    Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedVenue::Adapter)
    Faraday::Adapter.lookup_middleware(:net_http).prepend(ScriptedVenue::Adapter)
    out = scenarios.map do |sc|
      exchange = sc['venue'].constantize.create!(name: NAMES.fetch(sc['venue']), maker_fee: '0.1', taker_fee: '0.1')
      # Gemini's pacing and every retry wait, as seconds slept: recorded, never waited.
      exchange.define_singleton_method(:sleep) { |s| ScriptedVenue.slept += s }
      ScriptedVenue.script = sc['script'].transform_values { |replies| replies.map(&:dup) }
      ScriptedVenue.requests = []
      ScriptedVenue.slept = 0
      result = exchange.get_tickers_info(force: true)
      outcome = if result.success?
                  { 'rows' => result.data.as_json }
                else
                  { 'errors' => result.errors.as_json, 'message' => result.errors.to_sentence }
                end
      exchange.destroy!
      sc.merge('requests' => ScriptedVenue.requests, 'slept' => ScriptedVenue.slept, 'rails' => outcome)
    end
    path = Rails.root.join(ARGV[0] || 'rust/tests/fixtures/catalogue_vectors.json')
    File.write(path, "#{JSON.pretty_generate({ 'sources' => pins, 'gems' => { 'honeymaker' => Honeymaker::VERSION },
                                               'honeymaker_files' => honeymaker_files, 'scenarios' => out })}\n")
    puts "recorded #{out.size} scenarios in #{path}"
  ensure
    ActiveRecord::Base.connection_pool.disconnect!
    FileUtils.rm_rf(dir) if dir
  end
end

Catalogue.record
