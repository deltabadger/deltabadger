require 'test_helper'

class Bot::RankableTest < ActiveSupport::TestCase
  test 'an exited index member does not count toward popularity' do
    bot = create(:dca_index, status: :scheduled, started_at: Time.current)
    quote = bot.quote_asset
    included_asset = create(:asset, symbol: 'INCLUDED')
    exited_asset = create(:asset, symbol: 'EXITED')
    included_ticker = create(:ticker, exchange: bot.exchange, base_asset: included_asset, quote_asset: quote)
    exited_ticker = create(:ticker, exchange: bot.exchange, base_asset: exited_asset, quote_asset: quote)
    BotIndexAsset.create!(bot: bot, asset: included_asset, ticker: included_ticker, in_index: true)
    BotIndexAsset.create!(bot: bot, asset: exited_asset, ticker: exited_ticker, in_index: false)

    symbols = Bot.send(:most_popular_bots, 5).to_h.keys

    assert_includes symbols, 'INCLUDED'
    assert_not_includes symbols, 'EXITED'
  end
end
