require 'test_helper'

# Placing the redeploy. Everything here is about NOT trading when we should not, and about never
# spending more than the offer — the two ways this leg could cost the user money.
class Bots::DcaIndexRedeployPlacementTest < ActiveSupport::TestCase
  include ExchangeMockHelpers

  def setup
    @bot = create(:dca_index, user: create(:user), with_api_key: true)
    @assets = %w[AAA BBB].to_h do |symbol|
      asset = create(:asset, symbol: symbol, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
      ticker = create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
      [symbol, { asset: asset, ticker: ticker }]
    end
    @bot.instance_variable_set(:@tickers, @assets.values.map { |a| a[:ticker] })
    @bot.stubs(:refresh_composition).returns(Result::Success.new)
    @bot.stubs(:redeploy_offer).returns(100.to_d)
    index_membership('AAA', 'BBB')
    price_all(100)
    # A distinct id per call: Mocha evaluates `returns` arguments ONCE, so one generated id would
    # make the second order collide with the first in persist_accepted_order!'s idempotency lookup.
    @bot.exchange.stubs(:market_buy).returns(*(1..6).map { |i| Result::Success.new(order_id: "r-#{i}") })
    stub_exchange_balances(@bot.exchange, { @bot.quote_asset_id => { free: 10_000, locked: 0 } })
  end

  # == what it buys ==

  test 'the whole offer is spent across the composition' do
    stub_holdings('AAA' => 0, 'BBB' => 0)

    @bot.redeploy!

    assert_in_delta 100, @bot.transactions.redeploy.sum(:quote_amount).to_f, 0.01
  end

  test 'buys go to the underweight member, not evenly by target weight' do
    # AAA already holds its half of the eventual total; BBB holds nothing, so the whole offer is its
    # shortfall. Splitting by raw target weight would put 50 into an asset that needs none.
    stub_holdings('AAA' => 100, 'BBB' => 0)

    @bot.redeploy!

    by_symbol = @bot.transactions.redeploy.group(:base).sum(:quote_amount)
    assert_in_delta 100, by_symbol['BBB'].to_f, 0.01
    assert_nil by_symbol['AAA']
  end

  test 'the rows are REDEPLOY, never REGULAR' do
    stub_holdings('AAA' => 0, 'BBB' => 0)

    @bot.redeploy!

    assert_equal 0, @bot.transactions.regular.count
    assert_operator @bot.transactions.redeploy.count, :>, 0
  end

  test 'the offer is capped by what the venue actually holds' do
    # The books say 100 is redeployable, but the user withdrew most of it.
    stub_exchange_balances(@bot.exchange, { @bot.quote_asset_id => { free: 30, locked: 0 } })
    stub_holdings('AAA' => 0, 'BBB' => 0)

    @bot.redeploy!

    assert_in_delta 30, @bot.transactions.redeploy.sum(:quote_amount).to_f, 0.01
  end

  test 'market orders even on a bot that trades with limits' do
    @bot.stubs(:limit_ordered?).returns(true)
    @bot.stubs(:limit_order_pcnt_distance_decimal).returns(0.05.to_d)
    stub_holdings('AAA' => 0, 'BBB' => 0)

    @bot.redeploy!

    assert @bot.transactions.redeploy.all?(&:market_order?),
           'a resting limit redeploy on a stopped bot would never be swept'
  end

  # Sizing and placement read the price at different moments, and a venue whose market order is an
  # emulated crossing limit reads it a third time inside the placement itself.
  test 'the order is priced where it will actually fill, not where it was sized' do
    stub_holdings('AAA' => 0, 'BBB' => 0)
    @bot.stubs(:get_orders_data).returns(Result::Success.new(sized_orders(50, 50)))
    price_all(130)

    @bot.redeploy!

    assert(@bot.transactions.redeploy.all? { |t| t.price.to_d == 130 })
    assert_in_delta 100, @bot.transactions.redeploy.sum(:quote_amount).to_f, 0.01,
                    'the quote spend is held, so the base amount shrinks instead'
  end

  # get_orders_data sizes to fit, so this is the invariant breaking rather than a routine case — but
  # it is the batch's only backstop against spending more than the user was offered.
  test 'the batch stops at the budget even if sizing hands it more than the offer' do
    stub_holdings('AAA' => 0, 'BBB' => 0)
    @bot.stubs(:get_orders_data).returns(Result::Success.new(sized_orders(80, 80)))

    @bot.redeploy!

    assert_in_delta 100, @bot.transactions.redeploy.sum(:quote_amount).to_f, 0.01,
                    '80 + 80 offered, 100 budgeted'
  end

  # A base amount cannot express a quote cap: the venue crosses at whatever its book says when it
  # gets there. The haircut is what keeps an adverse fill inside the budget.
  test 'a base-denominated order is sized with headroom' do
    order_data = { ticker: @assets['AAA'][:ticker], price: 100.to_d, amount: 1.to_d }

    assert_in_delta 1 / 1.01, @bot.send(:base_headroom_amount, order_data).to_f, 0.0001
  end

  # Checking the minimum first and haircutting after let an order that had just cleared the floor
  # drop back under it — and an undersized rejection is not transient, so it halted the batch.
  test 'the venue minimum is checked against the haircut amount, not the amount before it' do
    stub_holdings('AAA' => 0, 'BBB' => 0)
    @bot.stubs(:get_orders_data).returns(Result::Success.new(sized_orders(50, 50)))
    # Base-denominated, and a floor that only the un-haircut amount would clear.
    @bot.stubs(:calculate_best_amount_info).with { |data| data[:ticker].present? }
        .returns({ amount_type: :base, amount: 0.5.to_d, below_minimum_amount: false },
                 { amount_type: :base, amount: 0.495.to_d, below_minimum_amount: true })

    @bot.redeploy!

    assert_equal 0, @bot.transactions.redeploy.count, 'skipped, not sent and halted'
    assert_not @bot.reload.redeploy_ambiguous?
  end

  # == guards ==

  test 'nothing is placed while a rebalance is mid-swap' do
    @bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING)
    stub_holdings('AAA' => 0, 'BBB' => 0)

    result = @bot.redeploy!

    assert result.failure?
    assert_equal 0, @bot.transactions.redeploy.count
  end

  test 'nothing is placed while a liquidation sell is still working' do
    create_waiting('AAA', type: 'LIQUIDATION', side: :sell)
    stub_holdings('AAA' => 0, 'BBB' => 0)

    result = @bot.redeploy!

    assert result.failure?
    assert_equal 0, @bot.transactions.redeploy.count
  end

  test 'a second click while the first batch is working places nothing' do
    create_waiting('AAA', type: 'REDEPLOY', side: :buy)
    stub_holdings('AAA' => 0, 'BBB' => 0)
    @bot.stubs(:advance_waiting_redeploys!)

    result = @bot.redeploy!

    assert result.failure?
    assert_equal 1, @bot.transactions.redeploy.count, 'only the row that was already there'
  end

  test 'a halted redeploy blocks the next one until it is resolved' do
    @bot.start_redeploy_placement!
    @bot.flag_redeploy_ambiguous!
    stub_holdings('AAA' => 0, 'BBB' => 0)

    result = @bot.redeploy!

    assert result.failure?
    assert_equal 0, @bot.transactions.redeploy.count
  end

  test 'a stopped bot can still redeploy' do
    # The schedule is only one of the ways money reaches a composition bot; this leg is a peer of
    # rebalancing, which also runs while the DCA leg is stopped.
    @bot.update!(status: :stopped, started_at: nil)
    stub_holdings('AAA' => 0, 'BBB' => 0)

    @bot.redeploy!

    assert_operator @bot.transactions.redeploy.count, :>, 0
  end

  test 'nothing is placed into a closed market' do
    stub_holdings('AAA' => 0, 'BBB' => 0)
    @bot.exchange.stubs(:market_open?).returns(false)

    result = @bot.redeploy!

    assert result.failure?
    assert_equal 0, @bot.transactions.redeploy.count
  end

  # The refresh can rotate a closed-market member in, so asking the old member list would let the
  # batch through on a check that was true a moment earlier.
  test 'the market check asks about the refreshed composition' do
    stub_holdings('AAA' => 0, 'BBB' => 0)
    sequence = sequence('refresh_then_check')
    @bot.expects(:refresh_composition).returns(Result::Success.new).in_sequence(sequence)
    @bot.exchange.expects(:market_open?).returns(true).in_sequence(sequence)

    @bot.redeploy!
  end

  # == outcomes we cannot know ==

  test 'an unknown placement outcome halts the batch' do
    stub_holdings('AAA' => 0, 'BBB' => 0)
    @bot.exchange.stubs(:market_buy).raises(Client::AmbiguousPlacementError, 'connection reset')

    @bot.redeploy!

    assert @bot.reload.redeploy_ambiguous?, 'the outcome is unknown, so nothing may trade after it'
  end

  test 'an accepted order with no id halts rather than being forgotten' do
    stub_holdings('AAA' => 0, 'BBB' => 0)
    @bot.exchange.stubs(:market_buy).returns(Result::Success.new(order_id: nil))

    @bot.redeploy!

    assert @bot.reload.redeploy_ambiguous?
  end

  test 'a placement intent that outlived its worker is promoted to a halt' do
    @bot.start_redeploy_placement!
    stub_holdings('AAA' => 0, 'BBB' => 0)

    @bot.redeploy!

    assert @bot.reload.redeploy_ambiguous?,
           'holding the semaphore proves no placement of ours is running'
  end

  test 'a pre-trade rejection is recorded without halting' do
    stub_holdings('AAA' => 0, 'BBB' => 0)
    @bot.exchange.stubs(:market_buy).returns(Result::Failure.new('Timestamp for this request'))
    @bot.exchange.stubs(:placement_transient_error?).returns(true)

    @bot.redeploy!

    assert_not @bot.reload.redeploy_ambiguous?
    assert_operator @bot.transactions.where(status: :failed).count, :>, 0
  end

  private

  def index_membership(*symbols)
    symbols.each do |symbol|
      BotIndexAsset.create!(bot: @bot, asset: @assets[symbol][:asset], ticker: @assets[symbol][:ticker],
                            target_allocation: 1.0 / symbols.size, in_index: true, entered_at: Time.current)
    end
  end

  def stub_holdings(values)
    @bot.stubs(:metrics).returns(
      asset_breakdown: values.transform_values { |v| { amount: v.to_d / 100, quote_invested: v.to_d } },
      realised_cash: 100.to_d
    )
  end

  def sized_orders(*quote_amounts)
    @assets.values.zip(quote_amounts).map do |a, quote|
      { ticker: a[:ticker], price: 100.to_d, amount: quote.to_d / 100, quote_amount: quote.to_d,
        side: :buy, order_type: :market_order }
    end
  end

  def price_all(price)
    %i[get_ask_price get_last_price].each do |method|
      Exchanges::Kraken.any_instance.stubs(method).returns(Result::Success.new(price.to_d))
    end
  end

  def create_waiting(symbol, type:, side:)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: :open, external_id: "w-#{SecureRandom.hex(4)}",
                         side: side, transaction_type: type, base: symbol,
                         quote: @bot.quote_asset.symbol, price: 100, amount: 1,
                         amount_exec: 0, quote_amount: 100, quote_amount_exec: 0)
  end
end
