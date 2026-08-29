require 'test_helper'

# A new bot names itself after what it holds, so the list reads as a portfolio instead of a
# kennel of random two-word names. The user renames it from the edit modal; nothing here ever
# overwrites a name that is already set.
class BotDefaultLabelTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
    @usd = create(:asset, :usd)
  end

  # --- one asset ---------------------------------------------------------------

  test 'a single-asset bot is named after the asset it buys' do
    bot = create(:dca_single_asset, user: @user, base_asset: @btc, quote_asset: @usd)

    assert_equal 'Bitcoin', bot.label
  end

  test 'a signal bot is named after its asset too' do
    bot = create(:signal_bot, user: @user, base_asset: @btc, quote_asset: @usd)

    assert_equal 'Bitcoin', bot.label
  end

  test 'a name the user picked is never overwritten' do
    bot = create(:dca_single_asset, user: @user, base_asset: @btc, quote_asset: @usd, label: 'Retirement')

    bot.quote_amount = 50
    bot.set_missed_quote_amount
    bot.save!

    assert_equal 'Retirement', bot.label
  end

  # --- a basket of assets ------------------------------------------------------

  test 'a two-asset bot is named by its tickers' do
    bot = create(:dca_multi_asset, user: @user, base_assets: [@btc, @eth], quote_asset: @usd)

    assert_equal 'BTC, ETH', bot.label
  end

  test 'a multi-asset bot is named by its first three tickers and the count of the rest' do
    xrp = create(:asset, symbol: 'XRP', name: 'XRP', external_id: 'ripple')
    sol = create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana')
    bot = create(:dca_multi_asset, user: @user, base_assets: [@btc, @eth, xrp, sol], quote_asset: @usd)

    assert_equal 'BTC, ETH, XRP + 1', bot.label
  end

  test 'a basket names its first three assets and counts the rest' do
    xrp = create(:asset, symbol: 'XRP', name: 'XRP', external_id: 'ripple')
    sol = create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana')
    ada = create(:asset, symbol: 'ADA', name: 'Cardano', external_id: 'cardano')
    bot = build(:dca_multi_asset, user: @user, base_assets: [@btc, @eth], quote_asset: @usd)

    assert_equal 'BTC, ETH, XRP + 2', bot.send(:basket_label, @btc.id, @eth.id, xrp.id, sol.id, ada.id)
  end

  test 'a basket skips assets that no longer exist' do
    bot = build(:dca_multi_asset, user: @user, base_assets: [@btc, @eth], quote_asset: @usd)

    assert_equal 'BTC', bot.send(:basket_label, @btc.id, nil, -1)
  end

  # --- an index ----------------------------------------------------------------

  test 'a category index bot is named after the index and how many coins it holds' do
    bot = build(:dca_index, user: @user, quote_asset: @usd)
    bot.index_type = Bots::DcaIndex::INDEX_TYPE_CATEGORY
    bot.index_category_id = 'layer-1'
    bot.index_name = 'Layer 1'
    bot.num_coins = 20
    bot.set_missed_quote_amount
    bot.save!

    assert_equal 'Layer 1 · 20', bot.label
  end

  test 'a count-named index carries the count inside its own name' do
    Index.create!(external_id: 'nasdaq-100', source: Index::SOURCE_DELTABADGER,
                  name: 'Nasdaq 100', top_coins: (1..100).map { |i| "s#{i}" })
    bot = build(:dca_index, user: @user, quote_asset: @usd)
    bot.index_type = Bots::DcaIndex::INDEX_TYPE_CATEGORY
    bot.index_category_id = 'nasdaq-100'
    bot.index_name = 'Nasdaq 100'
    bot.index_name_prefix = 'Nasdaq'
    bot.num_coins = 7
    bot.set_missed_quote_amount
    bot.save!

    assert_equal 'Nasdaq 7', bot.label
  end

  test 'the count in the name is the count the bot will actually buy' do
    Index.create!(external_id: 'nasdaq-100', source: Index::SOURCE_DELTABADGER,
                  name: 'Nasdaq 20', top_coins: (1..20).map { |i| "s#{i}" })
    bot = build(:dca_index, user: @user, quote_asset: @usd)
    bot.index_type = Bots::DcaIndex::INDEX_TYPE_CATEGORY
    bot.index_category_id = 'nasdaq-100'
    bot.index_name_prefix = 'Nasdaq'
    bot.num_coins = 50
    bot.set_missed_quote_amount
    bot.save!

    assert_equal 'Nasdaq 20', bot.label, 'the clamp runs before the name is taken'
  end

  test 'a top-coins index bot is named by its size' do
    bot = build(:dca_index, user: @user, quote_asset: @usd)
    bot.num_coins = 6
    bot.set_missed_quote_amount
    bot.save!

    assert_equal 'Top 6', bot.label
  end

  # --- nothing to name it after ------------------------------------------------

  test 'a bot whose asset has gone missing still gets a name' do
    bot = Bots::DcaSingleAsset.new(user: @user, settings: { 'quote_amount' => 10 })

    bot.validate

    assert_predicate bot.label, :present?
    assert_empty bot.errors[:label]
  end
end
