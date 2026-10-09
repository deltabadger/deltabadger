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

  test 'a one-asset basket is named after its asset, as the single-asset bot it replaces was' do
    bot = create(:dca_multi_asset, user: @user, base_assets: [@btc], quote_asset: @usd)

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

  # Loading a bot is a read (the page, MCP get_bot / list_bots): a missing name is computed for
  # display, never saved, so the row and its updated_at stay as they were.
  test 'a bot loaded without a name shows one but the read saves nothing' do
    bot = create(:dca_single_asset, user: @user, base_asset: @btc, quote_asset: @usd)
    ['', nil].each do |blank|
      bot.update_columns(label: blank, updated_at: 1.day.ago.round)
      stamp = bot.reload.updated_at

      loaded = Bot.find(bot.id)

      assert_equal 'Bitcoin', loaded.label
      assert_not loaded.label_changed?, 'nothing for a later save to write'
      assert_equal [blank, stamp], Bot.where(id: bot.id).pick(:label, :updated_at)
    end
  end

  # The shown name is the bot's name: saving it on purpose writes it, so later settings changes
  # cannot rename the bot behind the user's back.
  test 'saving the shown name of a bot loaded without one writes it' do
    bot = create(:dca_single_asset, user: @user, base_asset: @btc, quote_asset: @usd)
    bot.update_columns(label: nil)

    loaded = Bot.find(bot.id)
    loaded.update!(label: loaded.label)

    assert_equal 'Bitcoin', Bot.where(id: bot.id).pick(:label)
  end

  test 'a save that names nothing leaves a bot loaded without a name unnamed' do
    bot = create(:dca_single_asset, user: @user, base_asset: @btc, quote_asset: @usd)
    bot.update_columns(label: nil)

    Bot.find(bot.id).update!(status: :stopped)

    assert_nil Bot.where(id: bot.id).pick(:label)
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

  test 'a count-named index carries the count inside its own name, no space, whatever the feed calls it' do
    Index.create!(external_id: 'nasdaq-100', source: Index::SOURCE_DELTABADGER,
                  name: 'Nasdaq 20', top_coins: (1..100).map { |i| "s#{i}" })
    bot = build(:dca_index, user: @user, quote_asset: @usd)
    bot.index_type = Bots::DcaIndex::INDEX_TYPE_CATEGORY
    bot.index_category_id = 'nasdaq-100'
    bot.index_name = 'Nasdaq 20'
    bot.num_coins = 7
    bot.set_missed_quote_amount
    bot.save!

    assert_equal 'ND7', bot.label
  end

  test 'a bot holding the whole universe takes the index name itself' do
    Index.create!(external_id: 'nasdaq-100', source: Index::SOURCE_DELTABADGER,
                  name: 'ND100', top_coins: (1..101).map { |i| "s#{i}" })
    bot = build(:dca_index, user: @user, quote_asset: @usd)
    bot.index_type = Bots::DcaIndex::INDEX_TYPE_CATEGORY
    bot.index_category_id = 'nasdaq-100'
    bot.num_coins = 101
    bot.hold_all = true
    bot.set_missed_quote_amount
    bot.save!

    assert_equal 'ND100', bot.label, 'the nominal name, not ND101'
  end

  test 'the count in the name is the count the bot will actually buy' do
    Index.create!(external_id: 'nasdaq-100', source: Index::SOURCE_DELTABADGER,
                  name: 'ND100', top_coins: (1..20).map { |i| "s#{i}" })
    bot = build(:dca_index, user: @user, quote_asset: @usd)
    bot.index_type = Bots::DcaIndex::INDEX_TYPE_CATEGORY
    bot.index_category_id = 'nasdaq-100'
    bot.num_coins = 50
    bot.hold_all = false
    bot.set_missed_quote_amount
    bot.save!

    assert_equal 'ND20', bot.label, 'clamped to what the feed publishes today; a fixed count, so named by it'
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
