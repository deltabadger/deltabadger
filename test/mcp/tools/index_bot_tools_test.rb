# frozen_string_literal: true

require 'test_helper'

# list_indices and create_index_bot share the fixture the index-bot service test uses: a Kraken
# venue with three EUR pairs, which is the minimum a category index needs to offer that quote.
class IndexBotToolsTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @exchange = create(:kraken_exchange)
    @eur = create(:asset, :eur)
    members = [create(:asset, :bitcoin), create(:asset, :ethereum),
               create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana', category: 'Cryptocurrency')]
    members.each { |asset| create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @eur) }
    create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)
    create(:index, external_id: 'layer-1', source: Index::SOURCE_COINGECKO, name: 'Layer 1',
                   top_coins: %w[bitcoin ethereum solana], available_exchanges: { 'Exchanges::Kraken' => 3 })
    create(:index, external_id: 'top-coins', source: Index::SOURCE_INTERNAL, name: 'Top coins',
                   top_coins: %w[bitcoin ethereum solana], available_exchanges: { 'Exchanges::Kraken' => 3 })
    MarketData.stubs(:configured?).returns(true)
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    MarketData.stubs(:get_top_coins).returns(Result::Success.new(%w[bitcoin ethereum solana]))
    Bot::ActionJob.stubs(:perform_later)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
    %w[list_indices create_index_bot].each { |tool| @user.set_mcp_tool_enabled(tool, true) }
    stub_mcp_client(@user)
  end

  teardown { ActionMCP::Current.reset }

  test 'list_indices names each index and the venues it is available on' do
    text = ListIndicesTool.new.execute.contents.first.text

    assert_match(/Indices \(2\)/, text)
    assert_match(/layer-1 \| Layer 1 \| 3 assets \| Kraken/, text)
  end

  test 'list_indices can be narrowed to one exchange' do
    assert_match(/Indices \(2\)/, ListIndicesTool.new(exchange_name: 'Kraken').execute.contents.first.text)
  end

  test 'create_index_bot creates and starts a DcaIndex bot' do
    text = CreateIndexBotTool.new(exchange_name: 'Kraken', quote_asset: 'EUR', quote_amount: 50,
                                  interval: 'week', index: 'layer-1').execute.contents.first.text

    assert_match(/created and started/, text)
    bot = @user.bots.last
    assert_equal 'Bots::DcaIndex', bot.type
    assert_equal 'layer-1', bot.index_category_id
  end

  test 'create_index_bot reports a refusal rather than raising' do
    text = CreateIndexBotTool.new(exchange_name: 'Kraken', quote_asset: 'EUR', quote_amount: 50,
                                  interval: 'week', index: 'nope').execute.contents.first.text

    assert_match(/not found or not available on Kraken/, text)
    assert_equal 0, @user.bots.count
  end
end
