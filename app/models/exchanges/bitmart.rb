# BitMart shut down, and honeymaker 0.10.0 removed the exchange — there is no client left to build.
# This class survives ONLY so an install that still holds Bitmart bots and trade history keeps
# loading (dropping the STI class would raise ActiveRecord::SubclassNotFound on every
# `Exchange.available` load) and can move those bots to a live venue. See Exchange::RETIRED_TYPES.
#
# Every call that would have reached the API returns a failure Result rather than raising. That is
# deliberate: the callers that keep a stranded bot usable — the bot page's metrics
# (Bots::DcaSingleAsset::Measurable#metrics_with_current_prices) and the order-fetch jobs — branch
# on `result.failure?`, and a NotImplementedError would break the very pages this class exists to
# keep alive.
class Exchanges::Bitmart < Exchange
  COINGECKO_ID = 'bitmart'.freeze

  # ensure_exchange_authenticated reads and writes these on every bot action, and the base class
  # defines neither.
  attr_reader :api_key

  def coingecko_id
    COINGECKO_ID
  end

  # Empty rather than absent: Exchange#transient_error? reads known_errors[:transient] and still
  # applies NETWORK_TRANSIENT_PATTERNS, and #throttled_error? short-circuits on an empty list.
  def known_errors
    {}
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = nil
  end

  # Exchange#symbols builds an ExchangeMarket, whose initializer calls
  # Honeymaker.exchange('bitmart') — gone in honeymaker 0.10.0.
  def symbols
    Result::Success.new([])
  end

  def get_tickers_info(force: false) = retired_failure
  def get_tickers_prices(force: false, symbols: nil) = retired_failure
  def get_balances(asset_ids: nil) = retired_failure
  def get_last_price(ticker:, force: false) = retired_failure
  def get_bid_price(ticker:, force: false) = retired_failure
  def get_ask_price(ticker:, force: false) = retired_failure
  def get_candles(ticker:, start_at:, timeframe:) = retired_failure
  def get_order(order_id:) = retired_failure
  def get_orders(order_ids:) = retired_failure
  def cancel_order(order_id:) = retired_failure
  def get_api_key_validity(api_key:) = retired_failure
  def market_buy(ticker:, amount:, amount_type:) = retired_failure
  def market_sell(ticker:, amount:, amount_type:) = retired_failure
  def limit_buy(ticker:, amount:, amount_type:, price:) = retired_failure
  def limit_sell(ticker:, amount:, amount_type:, price:) = retired_failure
  def withdraw(asset:, amount:, address:, network: nil, address_tag: nil) = retired_failure
  def fetch_withdrawal_fees! = retired_failure
  def get_ledger(api_key:, start_time: nil) = retired_failure

  # These two are NOT Result-returning contracts — their callers expect a plain value, and a
  # Result::Failure would read as a truthy address list / amount rule.
  def list_withdrawal_addresses(asset:)
    nil
  end

  def minimum_amount_logic(**)
    :base
  end

  private

  def retired_failure
    Result::Failure.new(I18n.t('errors.exchange_retired'))
  end
end
