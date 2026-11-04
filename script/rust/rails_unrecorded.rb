# Records, for rust/tests/rails_unrecorded.rs (A28-1, PLAN-A28 r3.3), what Rails decides about a placement's transport
# failure and the text a failed Transaction keeps of it: each exception is run through Client.network_failure (the one
# place Rails classifies one) and Exchange#ambiguous_placement_error?, or, when it raises, through the placement guard's
# own test (Bot::ExchangeUser#with_placement_guard: ambiguous unless Client.pre_transmission?). Offline, no database rows.
# A28-2a adds `bookings`: on scratch rows, what Bot::OrderCreator#persist_accepted_order! writes for a booked Binance order,
# what Bot::FetchAndUpdateOrderJob's poll then makes of it (bot.get_order, update_with_order_data), and the carry
# (Bot::Accountable#pending_quote_amount) with and without a booked row.
#   s=$(mktemp -d); env -u DATABASE_URL RAILS_ENV=test SKIP_TEST_DATABASE=true PRIMARY_DATABASE_URL=sqlite3:$s/p.sqlite3 \
#     QUEUE_DATABASE_URL=sqlite3:$s/q.sqlite3 CACHE_DATABASE_URL=sqlite3:$s/c.sqlite3 CABLE_DATABASE_URL=sqlite3:$s/w.sqlite3 \
#     sh -c 'bin/rails runner script/rust/schema_template.rb && bin/rails runner script/rust/rails_unrecorded.rb rust/tests/fixtures/rails_unrecorded_vectors.json'
require 'json'
require 'digest'
require 'active_support/testing/time_helpers'

Net::HTTP.prepend(Module.new { def connect = raise("real connection to #{address}:#{port}") })

module RailsUnrecordedVectors
  module_function

  PORTED = %w[app/models/client.rb app/models/exchange.rb app/models/bot/exchange_user.rb
              app/models/bot/composition/order_setter.rb app/models/bot/order_creator.rb
              app/models/bot/accountable.rb app/models/transaction.rb app/models/exchanges/binance.rb].freeze

  def wrapped(outer, inner_class, inner_message, outer_message = nil)
    raise inner_class, inner_message
  rescue StandardError
    begin
      raise outer, (outer_message || $!.message)
    rescue StandardError => e
      e
    end
  end

  def cases
    [
      wrapped(Faraday::TimeoutError, Net::ReadTimeout, 'Net::ReadTimeout'),
      wrapped(Faraday::ConnectionFailed, Errno::ECONNRESET, 'Connection reset by peer'),
      wrapped(Faraday::ConnectionFailed, Errno::ECONNREFUSED, 'connect(2) for 127.0.0.1:443', 'Failed to open TCP connection to api.example:443 (Connection refused - connect(2) for 127.0.0.1:443)'),
      wrapped(Faraday::ConnectionFailed, Net::OpenTimeout, 'execution expired', 'Failed to open TCP connection to api.example:443 (execution expired)'),
      wrapped(Faraday::ConnectionFailed, SocketError, 'getaddrinfo: nodename nor servname provided, or not known',
              'Failed to open TCP connection to api.example:443 (getaddrinfo: nodename nor servname provided, or not known)'),
      wrapped(Faraday::ConnectionFailed, EOFError, 'end of file reached'),
      wrapped(Faraday::SSLError, OpenSSL::SSL::SSLError, 'SSL_read: unexpected eof while reading'),
      wrapped(Faraday::TimeoutError, Net::WriteTimeout, 'Net::WriteTimeout')
    ]
  end

  # [the text a failed row keeps, Rails' verdict]. A raise is the placement guard's verdict on the same message.
  def vectors
    exchange = Exchanges::Binance.new
    cases.map do |error|
      result = Client.network_failure(error)
      { 'errors' => Array(result.errors), 'ambiguous' => exchange.ambiguous_placement_error?(result), 'path' => 'result' }
    rescue Client::TransientNetworkError => e
      { 'errors' => [e.message], 'ambiguous' => !Client.pre_transmission?(e.original_class, e.message), 'path' => 'raised' }
    end
  end
end

# A stand-in for the venue: query_order answers the case's order body, account_trade_list its trades.
class RailsUnrecordedClient
  attr_accessor :order, :trades

  def query_order(**) = Result::Success.new({ raw: order })
  def account_trade_list(**) = Result::Success.new(trades)
  def method_missing(name, *, **) = Result::Failure.new("no synthetic answer for #{name}")
  def respond_to_missing?(*) = true
end

module RailsUnrecordedBookings
  extend ActiveSupport::Testing::TimeHelpers
  module_function

  DAY0 = Time.utc(2026, 9, 1)
  COLUMNS = %w[external_id status external_status side order_type amount quote_amount price amount_exec quote_amount_exec
               base quote base_asset_id quote_asset_id bot_interval bot_quote_amount transaction_type].freeze

  def trade(id, order_id, qty, price, quote, commission, asset, buyer:, maker:)
    { 'symbol' => 'BTCUSDT', 'id' => id, 'orderId' => order_id, 'orderListId' => -1, 'price' => price, 'qty' => qty,
      'quoteQty' => quote, 'commission' => commission, 'commissionAsset' => asset, 'time' => 1_788_220_800_000 + id,
      'isBuyer' => buyer, 'isMaker' => maker, 'isBestMatch' => true }
  end

  def order(id, type, side, status, price:, orig_qty:, orig_quote:, executed:, cumulative:)
    { 'symbol' => 'BTCUSDT', 'orderId' => id, 'orderListId' => -1, 'clientOrderId' => "synthetic#{id}", 'price' => price,
      'origQty' => orig_qty, 'executedQty' => executed, 'cummulativeQuoteQty' => cumulative, 'status' => status,
      'timeInForce' => 'GTC', 'type' => type, 'side' => side, 'stopPrice' => '0.00000000',
      'icebergQty' => '0.00000000', 'time' => 1_788_220_800_000, 'updateTime' => 1_788_220_800_500, 'isWorking' => true,
      'workingTime' => 1_788_220_800_000, 'origQuoteOrderQty' => orig_quote, 'selfTradePreventionMode' => 'EXPIRE_MAKER' }
  end

  # name => [order body, trades, the booking's own denomination: [:quote | :base, size]]. As the Rust booking reads a body
  # (venue::binance): a LIMIT order is sized in base (origQty), a MARKET order with origQuoteOrderQty > 0 in quote.
  def cases
    {
      'market_buy_quote_base_fee' => [
        order(1001, 'MARKET', 'BUY', 'FILLED', price: '0.00', orig_qty: '0.00040000', orig_quote: '20.00000000',
                                                 executed: '0.00040000', cumulative: '19.99800000'),
        [trade(5001, 1001, '0.00020000', '50000.00', '10.00000000', '0.00000020', 'BTC', buyer: true, maker: false),
         trade(5002, 1001, '0.00020000', '49990.00', '9.99800000', '0.00000020', 'BTC', buyer: true, maker: false)],
        [:quote, '20']
      ],
      'limit_buy_open' => [
        order(1002, 'LIMIT', 'BUY', 'NEW', price: '50000.00', orig_qty: '0.00040000', orig_quote: '0.00000000',
                                            executed: '0.00000000', cumulative: '0.00000000'),
        [], [:base, '0.0004']
      ],
      'limit_buy_partial' => [
        order(1003, 'LIMIT', 'BUY', 'PARTIALLY_FILLED', price: '50000.00', orig_qty: '0.00040000', orig_quote: '0.00000000',
                                                         executed: '0.00010000', cumulative: '5.00000000'),
        [trade(5003, 1003, '0.00010000', '50000.00', '5.00000000', '0.00000010', 'BTC', buyer: true, maker: true)],
        [:base, '0.0004']
      ],
      'limit_buy_better_price' => [
        order(1004, 'LIMIT', 'BUY', 'FILLED', price: '50000.00', orig_qty: '0.00040000', orig_quote: '0.00000000',
                                               executed: '0.00040000', cumulative: '19.99600000'),
        [trade(5004, 1004, '0.00040000', '49990.00', '19.99600000', '0.00000040', 'BTC', buyer: true, maker: false)],
        [:base, '0.0004']
      ],
      'market_sell_base_quote_fee' => [
        order(1005, 'MARKET', 'SELL', 'FILLED', price: '0.00', orig_qty: '0.00040000', orig_quote: '0.00000000',
                                                  executed: '0.00040000', cumulative: '20.00400000'),
        [trade(5005, 1005, '0.00020000', '50010.00', '10.00200000', '0.01000200', 'USDT', buyer: false, maker: false),
         trade(5006, 1005, '0.00020000', '50010.00', '10.00200000', '0.01000200', 'USDT', buyer: false, maker: false)],
        [:base, '0.0004']
      ]
    }
  end

  def world
    Rails.configuration.dry_run = false # the test environment's default would answer get_order from the dry store
    @client = RailsUnrecordedClient.new
    client = @client
    Honeymaker.define_singleton_method(:client) { |*, **| client }
    travel_to(DAY0 - 1.day) do
      @user = User.new(name: 'Owner', email: 'owner@example.com', password: 'Synthetic-password-123!',
                       confirmed_at: Time.current, setup_completed: true, time_zone: 'UTC')
      @user.save!(validate: false)
      @binance = Exchanges::Binance.create!(name: 'Binance', maker_fee: '0.1', taker_fee: '0.1')
      @btc = Asset.create!(external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency')
      @usdt = Asset.create!(external_id: 'tether', symbol: 'USDT', name: 'Tether', category: 'Cryptocurrency')
      [@btc, @usdt].each { |asset| ExchangeAsset.create!(exchange: @binance, asset:, available: true) }
      @ticker = Ticker.create!(exchange: @binance, ticker: 'BTCUSDT', base: 'BTC', quote: 'USDT', base_asset: @btc, quote_asset: @usdt,
                               base_decimals: 5, quote_decimals: 8, price_decimals: 2,
                               minimum_base_size: BigDecimal('0.00001'), minimum_quote_size: BigDecimal('5'))
      ApiKey.new(user: @user, exchange: @binance, key: 'k', secret: 's', status: :correct, key_type: :trading).save!(validate: false)
    end
  end

  # A daily 20 USDT BTC/USDT bot, started fresh at DAY0 00:00.
  def bot
    travel_to(DAY0) do
      b = Bots::DcaSingleAsset.new(user: @user, exchange: @binance, settings: {
        'base_asset_id' => @btc.id, 'quote_asset_id' => @usdt.id, 'quote_amount' => 20.0, 'interval' => 'day'
      })
      b.set_missed_quote_amount
      b.save!
      raise "start refused: #{b.errors.full_messages}" unless b.start(start_fresh: true)

      Bot.find(b.id)
    end
  end

  # The row's columns exactly as SQLite returns them, decimals as strings, the asset ids as their symbols.
  def row(id)
    db = ActiveRecord::Base.connection.raw_connection
    values = db.execute("SELECT #{COLUMNS.join(', ')} FROM transactions WHERE id = ?", [id]).first
    values = COLUMNS.zip(values.is_a?(Hash) ? values.values_at(*COLUMNS) : values).to_h
    values['status'] = Transaction.statuses.key(values['status'])
    values['external_status'] = Transaction.external_statuses.key(values['external_status'])
    values['side'] = Transaction.sides.key(values['side'])
    values['order_type'] = Transaction.order_types.key(values['order_type'])
    %w[base_asset_id quote_asset_id].each { |c| values[c] = values[c] && Asset.find(values[c]).symbol }
    %w[amount quote_amount price amount_exec quote_amount_exec bot_quote_amount].each { |c| values[c] = values[c]&.to_s }
    values
  end

  def booking(bot, body, trades, denomination)
    side = body['side'].downcase.to_sym
    order_type = body['type'] == 'MARKET' ? :market_order : :limit_order
    # The booking's price: the limit price, or a market order's gross average fill.
    price = if order_type == :limit_order then body['price'].to_d
            else body['cummulativeQuoteQty'].to_d / body['executedQty'].to_d
            end
    kind, size = denomination
    order_data = { ticker: @ticker, side:, order_type:, price:,
                   amount: kind == :base ? size.to_d : nil, quote_amount: kind == :quote ? size.to_d : nil }
    order_id = "BTCUSDT-#{body['orderId']}"
    transaction = travel_to(DAY0 + 1.hour) { bot.persist_accepted_order!(order_data, order_id) }
    persisted = row(transaction.id)

    # Bot::FetchAndUpdateOrderJob#perform, its venue read and its update, against the stand-in.
    @client.order = body
    @client.trades = trades
    result = travel_to(DAY0 + 1.hour + 5.seconds) { bot.get_order(order_id:) }
    raise "get_order failed: #{result.errors}" if result.failure?

    transaction.reload
    if %i[open closed cancelled].include?(result.data[:status])
      raise "update failed for #{order_id}" unless travel_to(DAY0 + 1.hour + 5.seconds) { transaction.update_with_order_data(result.data) }
    end
    { 'order' => body, 'trades' => trades, 'persisted' => persisted, 'polled' => row(transaction.id) }
  end

  def filled_buy(bot, at, quote)
    bot.transactions.create!(exchange: @binance, status: :submitted, external_status: :closed, external_id: "BTCUSDT-#{bot.id}#{at.to_i}",
                             side: :buy, order_type: :market_order, price: '50000', amount: nil, quote_amount: quote,
                             amount_exec: (quote.to_d / 50_000), quote_amount_exec: quote, base: 'BTC', quote: 'USDT',
                             base_asset_id: @btc.id, quote_asset_id: @usdt.id, bot_interval: 'day', bot_quote_amount: 20,
                             created_at: at, updated_at: at)
  end

  def pending(bot, at) = travel_to(at) { Bot.find(bot.id).pending_quote_amount.to_d.to_s('F') }

  def carry
    plain = bot
    windowed = bot
    windowed.update_column(:settings_changed_at, DAY0 + 2.days)
    [plain, windowed].each do |b|
      filled_buy(b, DAY0, '20')
      filled_buy(b, DAY0 + 2.days, '40')
    end
    readings = lambda do |with|
      suffix = with ? 'with' : 'without'
      out = {}
      # At the checkpoint instant itself Rails has not counted that day's interval yet (last_interval_checkpoint_at is
      # the day before); a tick runs just after it, so each reading is also taken one second later.
      { '' => 0, '_after_tick' => 1 }.each do |tail, s|
        out["day3_#{suffix}#{tail}"] = pending(plain, DAY0 + 3.days + s)
        out["day4_#{suffix}#{tail}"] = pending(plain, DAY0 + 4.days + s) if with
        out["before_window_#{suffix}#{tail}"] = pending(windowed, DAY0 + 3.days + s)
      end
      out
    end
    without = readings.(false)
    [plain, windowed].each { |b| filled_buy(b, DAY0 + 1.day, '20') }
    without.merge(readings.(true))
  end

  def record
    world
    b = bot
    cases_out = cases.to_h { |name, (body, trades, denomination)| [name, booking(b, body, trades, denomination)] }
    { 'cases' => cases_out, 'carry' => carry }
  end
end

sources = RailsUnrecordedVectors::PORTED.to_h { |path| [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest] }
vectors = RailsUnrecordedVectors.vectors
bookings = RailsUnrecordedBookings.record
File.write(ARGV.fetch(0), "#{JSON.pretty_generate({ 'ported_sources' => sources, 'vectors' => vectors, 'bookings' => bookings })}\n")
puts "wrote #{vectors.size} vectors, #{bookings['cases'].size} bookings"
