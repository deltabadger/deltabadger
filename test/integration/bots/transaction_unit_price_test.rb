require 'test_helper'

class Bots::TransactionUnitPriceTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    @bot = create(:dca_single_asset, user: @user, status: :waiting)
    sign_in @user
  end

  test 'shows unit price column header on bot show page' do
    get bot_path(id: @bot.id)
    assert_response :ok
    assert_select 'th', text: I18n.t('data_labels.unit_price')
    assert_select 'th[data-order-filter-target=columnHeader]', text: I18n.t('data_labels.unit_price')
  end

  test 'closed submitted order renders unit price from executed amounts' do
    create(:transaction, bot: @bot, status: :submitted, external_status: :closed,
                         amount: 0.001, amount_exec: 0.00694239,
                         quote_amount: 50, quote_amount_exec: 27.000331, price: 3889.15)
    decimals = { @bot.base_asset.symbol => 8, @bot.quote_asset.symbol => 4 }
    get bot_path(id: @bot.id, format: :turbo_stream), params: { decimals: decimals }
    assert_response :ok
    # 27.000331 / 0.00694239 = ~3889.1505... rounded to 4 quote decimals
    expected = (27.000331.to_d / 0.00694239.to_d).round(4)
    assert_match(/#{expected}\s+#{@bot.quote_asset.symbol}/, response.body)
  end

  test 'pending order renders unit price from configured amounts' do
    create(:transaction, :pending, bot: @bot,
                                   amount: 0.5, quote_amount: 100,
                                   amount_exec: nil, quote_amount_exec: nil, price: 200)
    decimals = { @bot.base_asset.symbol => 8, @bot.quote_asset.symbol => 2 }
    get bot_path(id: @bot.id, format: :turbo_stream), params: { decimals: decimals }
    assert_response :ok
    # 100 / 0.5 = 200.00
    assert_match(/200\.0+\s+#{@bot.quote_asset.symbol}/, response.body)
  end

  test 'unfilled order with nil quote_amount falls back through price * amount' do
    # Open order: amount + price set, quote_amount nil; partial computes quote_amount = price * amount
    create(:transaction, :open, bot: @bot,
                                amount: 0.5, price: 200, quote_amount: nil)
    decimals = { @bot.base_asset.symbol => 8, @bot.quote_asset.symbol => 2 }
    get bot_path(id: @bot.id, format: :turbo_stream), params: { decimals: decimals }
    assert_response :ok
    # quote_amount derived as 200 * 0.5 = 100; unit price = 100 / 0.5 = 200
    assert_match(/200\.0+\s+#{@bot.quote_asset.symbol}/, response.body)
  end

  test 'order with zero amount renders bare dash (no quote suffix), no ZeroDivisionError' do
    # Skipped orders have nil amount; assert no crash AND no "- QUOTE" output
    create(:transaction, :skipped, bot: @bot)
    decimals = { @bot.base_asset.symbol => 8, @bot.quote_asset.symbol => 2 }
    get bot_path(id: @bot.id, format: :turbo_stream), params: { decimals: decimals }
    assert_response :ok
    refute_match(/-\s+#{@bot.quote_asset.symbol}/, response.body,
                 'dash should not be followed by a quote-asset suffix when unit price is not computable')
  end

  test 'orders placeholder colspan is 5 (accommodates new column)' do
    # No transactions → placeholder row renders
    get bot_path(id: @bot.id)
    assert_response :ok
    assert_select '#orders_list_placeholder td[colspan=?]', '5'
  end

  test 'order_timeline row spans the new column (colspan=3 for Amount+Value+UnitPrice)' do
    create(:transaction, bot: @bot, amount: 0.001, quote_amount: 50,
                         amount_exec: 0.001, quote_amount_exec: 50, price: 50_000)
    decimals = { @bot.base_asset.symbol => 8, @bot.quote_asset.symbol => 2 }
    get bot_path(id: @bot.id, format: :turbo_stream), params: { decimals: decimals }
    assert_response :ok
    # _order_timeline renders the sentence-style row under the "All" view;
    # its combined cell must span Amount + Value + UnitPrice (3 columns).
    assert_match(/colspan=["']3["']/, response.body,
                 'expected timeline summary cell to span 3 columns after adding Unit price')
  end
end
