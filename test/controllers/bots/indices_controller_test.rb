# frozen_string_literal: true

require 'test_helper'

# The bot menu's index entries: which bot gets which, what the picker offers, and what a pick does.
class Bots::IndicesControllerTest < ActionDispatch::IntegrationTest
  setup do
    Bot::UpdateMetricsJob.stubs(:perform_later)
    Bot::ResyncIndexCompositionJob.stubs(:perform_later)
    MarketData.stubs(:configured?).returns(true)
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @usd = create(:asset, :usd)
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
    @sol = create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana')
    @doge = create(:asset, symbol: 'DOGE', name: 'Dogecoin', external_id: 'dogecoin')
    [@btc, @eth, @sol].each { |asset| create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @usd) }
    @layer1 = Index.create!(external_id: 'layer-1', source: Index::SOURCE_COINGECKO, name: 'Layer 1',
                            top_coins: %w[bitcoin ethereum solana])
    # Only one member trades on Binance against USD: not enough to follow.
    @memes = Index.create!(external_id: 'meme-token', source: Index::SOURCE_COINGECKO, name: 'Meme',
                           top_coins: %w[dogecoin bitcoin])
    @basket = create(:dca_multi_asset, user: @user, exchange: @exchange, quote_asset: @usd, base_assets: [@btc],
                                       status: :stopped)
    sign_in @user
  end

  test 'a portfolio menu offers Follow index, an index bot Change the index and Custom allocation' do
    get bot_path(id: @basket.id)
    assert_select "a[href*='/bots/#{@basket.id}/index/new']", text: I18n.t('bot.index_switch.follow_index')
    assert_select "form[action*='/bots/#{@basket.id}/custom_allocation']", count: 0

    index_bot = Bot::IndexSwitch.follow!(@basket, @layer1)
    get bot_path(id: index_bot.id)
    assert_select "a[href*='/bots/#{index_bot.id}/index/new']", text: I18n.t('bot.index_switch.change_index')
    assert_select "form[action*='/bots/#{index_bot.id}/custom_allocation'] button",
                  text: I18n.t('bot.index_switch.custom_allocation')
  end

  test 'the picker offers only indices enough of whose members trade on the venue at the bots currency' do
    get new_bot_index_path(bot_id: @basket.id)

    assert_response :success
    assert_select '.itile[data-index-category-id=layer-1]'
    assert_select '.itile[data-index-category-id=meme-token]', count: 0
  end

  test 'picking an index switches the bot and sends the browser to it' do
    post bot_index_path(bot_id: @basket.id), params: { index_type: 'category', index_category_id: 'layer-1' }, as: :turbo_stream

    assert_response :success
    assert_match "/bots/#{@basket.id}", response.body
    assert_instance_of Bots::DcaIndex, Bot.find(@basket.id)
  end

  test 'an index the picker did not offer is refused' do
    post bot_index_path(bot_id: @basket.id), params: { index_type: 'category', index_category_id: 'meme-token' }, as: :turbo_stream

    assert_response :unprocessable_entity
    assert_match ERB::Util.html_escape(I18n.t('errors.bots.index_switch.not_offered')), response.body
    assert_instance_of Bots::DcaMultiAsset, Bot.find(@basket.id)
  end

  test 'a running bot gets the reason, not a switch' do
    @basket.update_columns(status: Bot.statuses[:scheduled])

    post bot_index_path(bot_id: @basket.id), params: { index_type: 'category', index_category_id: 'layer-1' }, as: :turbo_stream

    assert_response :unprocessable_entity
    assert_match I18n.t('errors.bots.index_switch.stop_first'), response.body
  end

  test 'Custom allocation turns an index bot into a portfolio' do
    index_bot = Bot::IndexSwitch.follow!(@basket, @layer1)
    BotIndexAsset.where(bot_id: index_bot.id).update_all(in_index: true, target_allocation: 1.0)

    post bot_custom_allocation_path(bot_id: index_bot.id), as: :turbo_stream

    assert_response :success
    assert_instance_of Bots::DcaMultiAsset, Bot.find(index_bot.id)
  end
end
