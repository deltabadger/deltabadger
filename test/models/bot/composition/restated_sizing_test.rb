require 'test_helper'

# The ledger this restates is not a display: it is what every order the bot places is sized
# against. A bot reading a tenth of its position buys as if it were 90% underweight, and sells a
# tenth of what it means to.
class Bot::Composition::RestatedSizingTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:alpaca_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
    @usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    @klac = create(:asset, external_id: 'klac', symbol: 'KLAC')
    @aapl = create(:asset, external_id: 'aapl', symbol: 'AAPL')
    @bot = create(:dca_multi_asset, user: @user, exchange: @exchange, with_api_key: false,
                                    base_assets: [@klac, @aapl], quote_asset: @usd)
    # Equal money into each: 2000 of KLAC at 1000 (2 shares), 2000 of AAPL at 100 (20 shares).
    buy('KLAC', amount: 2, price: 1000)
    buy('AAPL', amount: 20, price: 100)
    Ticker.any_instance.stubs(:get_ask_price).returns(Result::Success.new(100.to_d))
    Ticker.any_instance.stubs(:get_last_price).returns(Result::Success.new(100.to_d))
  end

  def buy(symbol, amount:, price:)
    create(:transaction, bot: @bot, exchange: @exchange, base: symbol, quote: 'USD', side: :buy,
                         amount: amount, amount_exec: amount, price: price,
                         quote_amount: amount * price, quote_amount_exec: amount * price,
                         created_at: 8.days.ago)
  end

  def split!(symbol: 'KLAC', ratio: '10:1')
    create(:account_transaction, user: @user, api_key: @api_key, exchange: @exchange,
                                 entry_type: :adjustment, base_currency: symbol, base_amount: 18,
                                 quote_currency: nil, quote_amount: nil, transacted_at: 5.days.ago,
                                 raw_data: { 'corporate_action' => 'split', 'split_ratio' => ratio })
  end

  def contribution_split
    result = @bot.send(:get_orders_data, 200)
    assert_predicate result, :success?
    result.data.to_h { |order| [order[:ticker].base, order[:quote_amount].to_d] }
  end

  test 'without the restatement the bot pours a contribution into the split asset' do
    orders = contribution_split

    # KLAC reads as 2 shares x 100 = 200 against AAPL's 2000: badly underweight, and it is not.
    assert_operator orders['KLAC'].to_d, :>, (orders['AAPL'] || 0).to_d
  end

  test 'a restated position is sized as the balanced portfolio it is' do
    split!

    orders = contribution_split

    assert_in_delta 100.to_d, orders['KLAC'].to_d, 1.to_d
    assert_in_delta 100.to_d, orders['AAPL'].to_d, 1.to_d
  end

  test 'a liquidation offers the restated amount, still capped by what is on the exchange' do
    split!
    holding = { symbol: 'KLAC', ticker: @bot.tickers.find { |t| t.base == 'KLAC' } }
    @bot.stubs(:side_price).returns(100.to_d)

    @bot.stubs(:live_free_balance).returns(1_000.to_d)
    assert_equal 20.to_d, @bot.send(:liquidation_order_data, holding, @bot.metrics(force: true))[:amount]

    @bot.stubs(:live_free_balance).returns(6.to_d)
    assert_equal 6.to_d, @bot.send(:liquidation_order_data, holding, @bot.metrics(force: true))[:amount],
                 'shares the user moved out are still not sellable'
  end
end
