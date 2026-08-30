require 'test_helper'

# The trading-condition controls on a basket bot. The partials and the subject picker are shared
# with the single-asset bot; what is new here is that the option list is the composition, and that
# a condition cannot be left watching an asset the basket is dropping.
class Bots::DcaMultiAssets::ConditionSettingsTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
    @quote = create(:asset, :usd)
    @exchange = create(:binance_exchange)
    [@btc, @eth].each do |asset|
      create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @quote)
    end
    create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)
    @bot = create(:dca_multi_asset, user: @user, exchange: @exchange,
                                    base_assets: [@btc, @eth], quote_asset: @quote)
    @bot.refresh_composition
  end

  # MIN_ASSETS is 2, so removing from a two-asset basket is refused by the size validation whatever
  # the conditions say — the removal tests below would prove nothing without a third member.
  def add_third_member!
    sol = create(:asset, symbol: 'SOL', name: 'Solana')
    create(:ticker, exchange: @exchange, base_asset: sol, quote_asset: @quote)
    patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: { add_asset_id: sol.id.to_s,
                                                                   normalize_allocations: '1' } },
                                 as: :turbo_stream
    @bot.reload.refresh_composition
    sol
  end

  test 'the settings page offers every basket member as a condition subject' do
    get bot_path(id: @bot.id)

    assert_response :success
    @bot.composition_tickers.each do |ticker|
      assert_select "select[name='bots_dca_multi_asset[price_limit_in_ticker_id]'] " \
                    "option[value='#{ticker.id}']"
    end
  end

  test 'the option list is the composition, not every quote-matching ticker on the venue' do
    create(:ticker, exchange: @exchange, base_asset: create(:asset, symbol: 'SOL', name: 'Solana'), quote_asset: @quote)

    get bot_path(id: @bot.id)

    assert_select "select[name='bots_dca_multi_asset[price_limit_in_ticker_id]'] option", count: 2
  end

  test 'the trigger mode field survives strong parameters' do
    ticker = @bot.composition_tickers.first

    patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: {
      price_limited: '1', price_limit: '50000',
      price_limit_mode: 'restrict',
      price_limit_value_condition: 'below',
      price_limit_in_ticker_id: ticker.id.to_s
    } }, as: :turbo_stream

    assert_equal 'while', @bot.reload.price_limit_timing_condition
    assert_equal 'pause', @bot.price_limit_action
  end

  test 'a ticker outside the basket is refused' do
    outsider = create(:ticker, exchange: @exchange, base_asset: create(:asset, symbol: 'ADA', name: 'Cardano'),
                               quote_asset: @quote)

    patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: {
      price_limited: '1', price_limit: '50000',
      price_limit_in_ticker_id: outsider.id.to_s
    } }, as: :turbo_stream

    assert_not_equal outsider.id, @bot.reload.price_limit_in_ticker_id
  end

  test 'removing an asset watched by an ENABLED condition is refused, not silently orphaned' do
    add_third_member!
    watched = @bot.composition_tickers.find { |ticker| ticker.base_asset_id == @eth.id }
    patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: {
      price_limited: '1', price_limit: '50000', price_limit_in_ticker_id: watched.id.to_s
    } }, as: :turbo_stream
    assert_equal watched.id, @bot.reload.price_limit_in_ticker_id

    patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: { remove_asset_id: @eth.id.to_s } },
                                 as: :turbo_stream

    assert_includes @bot.reload.base_asset_ids, @eth.id
  end

  test 'removing an asset a DISABLED condition merely defaulted to is allowed' do
    # Each concern's set_*_in_ticker_id callback picks a default subject even when the condition is
    # off. That must not make a basket member unremovable.
    add_third_member!
    watched = @bot.composition_tickers.find { |ticker| ticker.base_asset_id == @eth.id }
    @bot.settings['price_limit_in_ticker_id'] = watched.id
    @bot.set_missed_quote_amount
    @bot.save!
    assert_not @bot.price_limited?

    patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: { remove_asset_id: @eth.id.to_s } },
                                 as: :turbo_stream

    assert_not_includes @bot.reload.base_asset_ids, @eth.id
  end
end
