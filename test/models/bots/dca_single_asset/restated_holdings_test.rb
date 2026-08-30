require 'test_helper'

# The single-asset walk keeps a running base amount and a running weighted average of what was paid
# for it. A split multiplies the first and divides the second; without folding it in, the bot shows
# a tenth of its position at ten times the price it paid.
class Bots::DcaSingleAsset::RestatedHoldingsTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:alpaca_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
    @usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    @klac = create(:asset, external_id: 'klac', symbol: 'KLAC')
    @bot = create(:dca_single_asset, user: @user, exchange: @exchange, with_api_key: false,
                                     base_asset: @klac, quote_asset: @usd)
  end

  def buy(amount:, price:, at:, side: :buy)
    create(:transaction, bot: @bot, exchange: @exchange, base: 'KLAC', quote: 'USD', side: side,
                         amount: amount, amount_exec: amount, price: price,
                         quote_amount: amount * price, quote_amount_exec: amount * price,
                         created_at: at)
  end

  def split(ratio: '10:1', at: 5.days.ago, **overrides)
    create(:account_transaction, { user: @user, api_key: @api_key, exchange: @exchange,
                                   entry_type: :adjustment, base_currency: 'KLAC', base_amount: 90,
                                   quote_currency: nil, quote_amount: nil, transacted_at: at,
                                   raw_data: { 'corporate_action' => 'split',
                                               'split_ratio' => ratio }.compact }.merge(overrides))
  end

  def metrics = @bot.metrics(force: true)

  test 'a bot with no restatement is untouched' do
    buy(amount: 2, price: 100, at: 8.days.ago)

    assert_equal 2.to_d, metrics[:total_base_amount]
    assert_equal 100.to_d, metrics[:average_buy_price]
  end

  test 'the position is multiplied and the money is not' do
    buy(amount: 2, price: 1000, at: 8.days.ago)
    split(at: 5.days.ago)

    assert_equal 20.to_d, metrics[:total_base_amount]
    assert_equal 2000.to_d, metrics[:total_quote_amount_invested]
  end

  test 'the average buy price is restated with the position' do
    buy(amount: 2, price: 1000, at: 8.days.ago)
    split(at: 5.days.ago)

    assert_equal 100.to_d, metrics[:average_buy_price], '2000 paid over 20 shares'
  end

  test 'buys either side of a split average in one unit' do
    buy(amount: 2, price: 1000, at: 8.days.ago)  # 2000 for what is now 20 shares
    split(at: 5.days.ago)
    buy(amount: 30, price: 100, at: 2.days.ago)  # 3000 for 30 shares

    assert_equal 50.to_d, metrics[:total_base_amount]
    assert_equal 100.to_d, metrics[:average_buy_price], '5000 paid over 50 shares'
  end

  test 'a reverse split divides the position and lifts the average' do
    buy(amount: 80, price: 10, at: 8.days.ago)
    split(at: 5.days.ago, ratio: '1:8')

    assert_equal 10.to_d, metrics[:total_base_amount]
    assert_equal 80.to_d, metrics[:average_buy_price]
  end

  test 'the value does not jump when a split arrives with no trade after it' do
    buy(amount: 2, price: 1000, at: 8.days.ago)
    before = metrics[:total_amount_value_in_quote]

    split(at: 5.days.ago)

    assert_equal before, metrics[:total_amount_value_in_quote]
  end

  test 'the split takes a chart point of its own and the curve does not step' do
    buy(amount: 2, price: 1000, at: 8.days.ago)
    points_before = metrics[:chart][:labels].size

    split(at: 5.days.ago)
    chart = metrics[:chart]

    assert_equal points_before + 1, chart[:labels].size
    assert_equal chart[:series][0][-2], chart[:series][0][-1]
    assert_equal 20.to_d, chart[:extra_series][0].last
  end

  test 'a sale after the split is measured against the restated position' do
    buy(amount: 2, price: 1000, at: 9.days.ago)
    split(at: 6.days.ago)
    buy(amount: 20, price: 120, at: 3.days.ago, side: :sell)

    assert_equal 0.to_d, metrics[:total_base_amount], 'twenty shares is the whole position'
    assert_equal 2400.to_d, metrics[:total_realized_proceeds]
  end

  test 'a split with no readable factor restates nothing' do
    buy(amount: 2, price: 1000, at: 8.days.ago)
    split(at: 5.days.ago, ratio: '')

    assert_equal 2.to_d, metrics[:total_base_amount]
  end

  # The accumulators behind `average_buy_price` cover every buy the bot ever made, not just the
  # ones still held — so they have to be restated even when the position is momentarily flat.
  test 'a position closed before the split and reopened after it averages in one unit' do
    buy(amount: 1, price: 100, at: 9.days.ago)
    buy(amount: 1, price: 110, at: 8.days.ago, side: :sell)
    split(at: 6.days.ago)
    buy(amount: 10, price: 10, at: 3.days.ago)

    assert_equal 10.to_d, metrics[:average_buy_price], 'ten paid for one share, then ten for ten'
    assert_equal 10.to_d, metrics[:total_base_amount]
  end
end
