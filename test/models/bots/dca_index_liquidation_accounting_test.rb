require 'test_helper'

# What a liquidation does to the numbers. The rules live in Bot::RebalanceAccounting and are shared
# with the dual-asset bot; these check the index bot applies them over its per-symbol breakdown.
#
# The load-bearing claim: selling does not move P/L. The gain is locked in, not created — so the
# holding leaves and its value reappears as realised cash, and the headline is unchanged.
class Bots::DcaIndexLiquidationAccountingTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_index, user: create(:user))
    %w[AAA BBB].each do |symbol|
      asset = create(:asset, symbol: symbol, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
      create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
    end
  end

  test 'selling a quitter at a gain does not move the reported P/L' do
    buy('AAA', quote: 100, price: 100)
    buy('BBB', quote: 100, price: 100)
    # AAA has doubled: 1 unit now worth 200.
    before = @bot.metrics(force: true)

    liquidate('AAA', amount: 1, quote: 200, price: 200)

    after = @bot.metrics(force: true)
    assert_in_delta before[:total_quote_amount_invested].to_f, after[:total_quote_amount_invested].to_f, 0.0001
    assert_in_delta 100, after[:realised_pnl].to_f, 0.0001, 'sold for 200, cost 100'
  end

  test 'realised P/L is negative when the quitter sold below cost' do
    buy('AAA', quote: 100, price: 100)

    liquidate('AAA', amount: 1, quote: 60, price: 60)

    assert_in_delta(-40, @bot.metrics(force: true)[:realised_pnl].to_f, 0.0001)
  end

  test 'the sold holding leaves the breakdown' do
    buy('AAA', quote: 100, price: 100)

    liquidate('AAA', amount: 1, quote: 150, price: 150)

    assert_in_delta 0, @bot.metrics(force: true).dig(:asset_breakdown, 'AAA', :amount).to_f, 0.0001
  end

  test 'the proceeds stay counted in value until the bot re-spends them' do
    buy('AAA', quote: 100, price: 100)
    buy('BBB', quote: 100, price: 100)

    liquidate('AAA', amount: 1, quote: 150, price: 150)

    metrics = @bot.metrics(force: true)
    # 100 of BBB still held, plus 150 of realised cash. Dropping the cash would read as a
    # withdrawal and show a loss that never happened.
    assert_in_delta 250, metrics[:total_amount_value_in_quote].to_f, 0.0001
    assert_in_delta 150, metrics[:rebalance_cash].to_f, 0.0001
  end

  test 'recycling the proceeds does not count them as new money invested' do
    buy('AAA', quote: 100, price: 100)
    buy('BBB', quote: 100, price: 100)
    liquidate('AAA', amount: 1, quote: 150, price: 150)

    buy('BBB', quote: 150, price: 100) # the DCA leg spends the proceeds

    metrics = @bot.metrics(force: true)
    assert_in_delta 200, metrics[:total_quote_amount_invested].to_f, 0.0001,
                    'still only the 200 the user actually contributed'
    assert_in_delta 250, metrics[:total_amount_value_in_quote].to_f, 0.0001
    assert_in_delta 50, metrics[:realised_pnl].to_f, 0.0001, 'recycling profit does not un-realise it'
  end

  test 'a buy larger than the proceeds counts only the excess as contributed' do
    buy('AAA', quote: 100, price: 100)
    liquidate('AAA', amount: 1, quote: 100, price: 100)

    buy('BBB', quote: 160, price: 100)

    assert_in_delta 160, @bot.metrics(force: true)[:total_quote_amount_invested].to_f, 0.0001,
                    '100 recycled + 60 of new money, on top of the original 100'
  end

  test 'the bought asset books what it actually cost, not the recycled basis' do
    buy('AAA', quote: 100, price: 100)
    liquidate('AAA', amount: 1, quote: 150, price: 150)

    buy('BBB', quote: 150, price: 150)

    breakdown = @bot.metrics(force: true)[:asset_breakdown]
    assert_in_delta 150, breakdown.dig('BBB', :quote_invested).to_f, 0.0001,
                    'its average price must be what was paid'
  end

  test 'a rebalance dust remainder is drained by the next contribution instead of double-counting' do
    # A rebalance buy leg that ends short leaves the residue in the flight bucket forever. Left
    # undrained, every later DCA buy counts that money twice: once as cash in the value, once as
    # fresh capital.
    buy('AAA', quote: 100, price: 100)
    rebalance_sell('AAA', amount: 0.5, quote: 50, price: 100)
    rebalance_buy('BBB', quote: 40, price: 100) # 10 left over as dust
    assert_in_delta 10, @bot.metrics(force: true)[:rebalance_cash].to_f, 0.0001

    buy('BBB', quote: 30, price: 100)

    metrics = @bot.metrics(force: true)
    assert_in_delta 0, metrics[:rebalance_cash].to_f, 0.0001
    assert_in_delta 120, metrics[:total_quote_amount_invested].to_f, 0.0001,
                    '100 originally + 20 of genuinely new money; the other 10 was already the bots'
  end

  test 'a partial dust drain leaves contributed and total P/L unchanged' do
    # The ledger's aggregate basis deliberately re-bases here — invested reads `contributed`, not the
    # ledger — so the invariant to hold is the headline, not the sum of the parts.
    buy('AAA', quote: 100, price: 100)
    rebalance_sell('AAA', amount: 0.5, quote: 75, price: 150) # sold above cost
    before = @bot.metrics(force: true)

    buy('BBB', quote: 25, price: 100) # smaller than the 75 in flight: a partial drain

    after = @bot.metrics(force: true)
    assert_in_delta before[:total_quote_amount_invested].to_f,
                    after[:total_quote_amount_invested].to_f, 0.0001
    assert_in_delta 0, after[:realised_pnl].to_f, 0.0001, 'a swap realises nothing'
  end

  test 'a rebalance sell realises nothing, because the swap moves the basis instead' do
    buy('AAA', quote: 100, price: 100)

    rebalance_sell('AAA', amount: 1, quote: 200, price: 200)

    assert_in_delta 0, @bot.metrics(force: true)[:realised_pnl].to_f, 0.0001
  end

  test 'the chart cash series carries realised cash so marking does not draw a cliff' do
    buy('AAA', quote: 100, price: 100)
    liquidate('AAA', amount: 1, quote: 150, price: 150)

    assert_in_delta 150, @bot.metrics(force: true)[:chart][:cash_series].last.to_f, 0.0001
  end

  test 'liquidation rows are not contributions' do
    buy('AAA', quote: 100, price: 100)
    liquidate('AAA', amount: 1, quote: 150, price: 150)

    assert_equal 1, @bot.transactions.regular.count
    assert_equal 1, @bot.transactions.liquidation.count
  end

  test 'a swap gain recycled by a DCA buy is still realised when the asset is finally sold' do
    # The dust carries basis 50 against 75 of cash — a 25 embedded gain. Booking the CASH onto the
    # bought asset would write that gain off silently, and the eventual sale would realise nothing.
    buy('AAA', quote: 100, price: 100)
    rebalance_sell('AAA', amount: 0.5, quote: 75, price: 150)
    buy('BBB', quote: 75, price: 75) # the DCA leg recycles the whole remainder

    liquidate('BBB', amount: 1, quote: 75, price: 75)

    assert_in_delta 25, @bot.metrics(force: true)[:realised_pnl].to_f, 0.0001
  end

  test 'a rebalance buy and a regular buy book the same basis for the same recycled cash' do
    # Two paths spend the same dust; if they disagree the accounting depends on which leg happened
    # to run, which is not a thing a user could ever reason about.
    regular = books_after('REGULAR')
    swapped = books_after('REBALANCE')

    assert_in_delta swapped, regular, 0.0001
  end

  private

  # Sets up 75 of flight cash carrying 50 of basis, spends it the given way, and returns what BBB
  # ended up booking. Reuses the one bot — a second :dca_index collides on the index's external id.
  def books_after(type)
    # reload first: destroy_all marks the association loaded, so a second call would destroy a cached
    # empty target and quietly stack the two runs on top of each other.
    @bot.transactions.reload.destroy_all
    buy('AAA', quote: 100, price: 100)
    rebalance_sell('AAA', amount: 0.5, quote: 75, price: 150)
    create_order('BBB', amount: 1, quote: 75, price: 75, side: :buy, type: type)
    @bot.metrics(force: true).dig(:asset_breakdown, 'BBB', :quote_invested).to_f
  end

  def buy(symbol, quote:, price:)
    create_order(symbol, amount: quote.to_d / price, quote:, price:, side: :buy, type: 'REGULAR')
  end

  def rebalance_buy(symbol, quote:, price:)
    create_order(symbol, amount: quote.to_d / price, quote:, price:, side: :buy, type: 'REBALANCE')
  end

  def rebalance_sell(symbol, amount:, quote:, price:)
    create_order(symbol, amount:, quote:, price:, side: :sell, type: 'REBALANCE')
  end

  def liquidate(symbol, amount:, quote:, price:)
    create_order(symbol, amount:, quote:, price:, side: :sell, type: 'LIQUIDATION')
  end

  def create_order(symbol, amount:, quote:, price:, side:, type:)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: :closed, external_id: "i-#{SecureRandom.hex(4)}",
                         side: side, transaction_type: type,
                         base: symbol, quote: @bot.quote_asset.symbol,
                         price: price, amount: amount, amount_exec: amount,
                         quote_amount: quote, quote_amount_exec: quote)
  end
end
