require 'test_helper'

class Bots::DcaMultiAssetTest < ActiveSupport::TestCase
  include ExchangeMockHelpers
  include Turbo::Broadcastable::TestHelper

  def setup
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @quote = create(:asset, :usd)
    @assets = %w[AAA BBB CCC].to_h do |symbol|
      asset = create(:asset, symbol: symbol, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
      ticker = create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @quote)
      [symbol, { asset:, ticker: }]
    end
  end

  test 'a fresh bot splits the wizard asset list into equal allocations and forgets the list' do
    bot = build(:dca_multi_asset, user: @user, exchange: @exchange,
                                  base_assets: @assets.values.map { it[:asset] }, quote_asset: @quote)
    bot.settings = bot.settings.except('allocations').merge(
      'quote_asset_id' => @quote.id,
      'quote_amount' => 100,
      'interval' => 'day',
      'base_asset_ids' => @assets.values.map { it[:asset].id }
    )
    bot.set_missed_quote_amount

    bot.save!

    ids = @assets.values.map { it[:asset].id.to_s }
    assert_equal({ ids[0] => 0.334, ids[1] => 0.333, ids[2] => 0.333 }, bot.allocations)
    assert_equal 1.0, bot.allocations.values.sum
    assert_not bot.settings.key?('base_asset_ids')
  end

  test 'needs at least two and at most twenty assets' do
    bot = build_bot
    bot.allocations = { @assets['AAA'][:asset].id.to_s => 1.0 }
    assert_not bot.valid?
    assert_predicate bot.errors[:allocations], :present?

    twenty_one = Array.new(21) do |index|
      create(:asset, symbol: "X#{index}", external_id: "x-#{index}")
    end
    twenty_one.each { |asset| create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @quote) }
    bot.allocations = bot.equal_allocations(twenty_one.map(&:id))
    assert_not bot.valid?
    assert_predicate bot.errors[:allocations], :present?

    bot.allocations = bot.equal_allocations(twenty_one.first(2).map(&:id))
    assert bot.valid?
    bot.allocations = bot.equal_allocations(twenty_one.first(20).map(&:id))
    assert bot.valid?
  end

  test 'weights are stored as posted; the sum is checked on start, not on save' do
    bot = build_bot
    ids = member_ids

    bot.allocations = { ids[0] => 0.6, ids[1] => 0.6 }
    assert bot.valid?
    assert_equal({ ids[0] => 0.6, ids[1] => 0.6 }, bot.allocations)
    assert_not bot.valid?(:start)

    bot.allocations = { ids[0] => 0.5005, ids[1] => 0.4995 }
    assert bot.valid?
    assert bot.valid?(:start)
  end

  test 'the residual of rounding lands on the largest weight and nothing goes negative' do
    bot = build_bot
    fourth = create(:asset, symbol: 'DDD', external_id: 'ddd')
    ids = @assets.values.map { it[:asset].id } + [fourth.id]

    normalized = bot.normalize_allocations(ids.zip([1, 1, 1, 0.00001]).to_h)

    assert(normalized.values.all? { it >= 0 })
    assert_equal 1.0, normalized.values.sum
    assert_equal 0.0, normalized[ids.last.to_s]
    assert_operator normalized[ids.first.to_s], :>, normalized[ids.second.to_s]
  end

  test 'the quote asset cannot be a member' do
    bot = build_bot
    bot.allocations = { @quote.id.to_s => 0.5, member_ids.first => 0.5 }

    assert_not bot.valid?
    assert_predicate bot.errors[:allocations], :present?
  end

  test 'malformed allocation values are invalid, not a 500' do
    bot = build_bot
    ids = member_ids

    bot.settings['allocations'] = { ids[0] => 'abc', ids[1] => 0.5 }
    assert_not bot.valid?
    assert_predicate bot.errors[:allocations], :present?

    bot.settings['allocations'] = { ids[0] => Float::NAN, ids[1] => 0.5 }
    assert_not bot.valid?
    assert_predicate bot.errors[:allocations], :present?
  end

  test 'an asset with no pair on the exchange is refused on create, on edit and when the exchange changes — even while stopped' do
    missing = create(:asset, symbol: 'MISSING', external_id: 'missing')
    create_bot = build(:dca_multi_asset, user: @user, exchange: @exchange,
                                         base_assets: [@assets['AAA'][:asset], missing], quote_asset: @quote)
    Ticker.find_by(exchange: @exchange, base_asset: missing, quote_asset: @quote).update!(available: false)
    assert_not create_bot.valid?
    assert_match(/MISSING/, create_bot.errors[:allocations].to_sentence)
    assert_match(/#{Regexp.escape(@exchange.name)}/, create_bot.errors[:allocations].to_sentence)

    stopped = create(:dca_multi_asset, :stopped, user: @user, exchange: @exchange,
                                                 base_assets: member_assets, quote_asset: @quote)
    stopped.allocations = stopped.allocations_adding(missing.id)
    stopped.set_missed_quote_amount
    assert_not stopped.save
    assert_match(/MISSING/, stopped.errors[:allocations].to_sentence)

    other_exchange = create(:kraken_exchange)
    create(:ticker, exchange: other_exchange, base_asset: @assets['AAA'][:asset], quote_asset: @quote)
    stopped.reload
    stopped.exchange = other_exchange
    assert_not stopped.save
    assert_match(/BBB/, stopped.errors[:allocations].to_sentence)
  end

  test 'the composition is frozen while the bot works' do
    bot = create(:dca_multi_asset, :started, user: @user, exchange: @exchange,
                                             base_assets: member_assets, quote_asset: @quote)
    bot.assign_attributes(bot.parse_params(remove_asset_id: member_ids.first))
    bot.set_missed_quote_amount
    assert_not bot.save
    assert_match(/running/i, bot.errors[:allocations].to_sentence)

    bot.reload
    bot.quote_amount = 125
    bot.set_missed_quote_amount
    assert bot.save

    other_exchange = create(:kraken_exchange)
    member_assets.each { |asset| create(:ticker, exchange: other_exchange, base_asset: asset, quote_asset: @quote) }
    bot.exchange = other_exchange
    assert_not bot.save
    assert_match(/running/i, bot.errors[:allocations].to_sentence)
  end

  test 'start refuses when a member pair is unavailable' do
    bot = create(:dca_multi_asset, user: @user, exchange: @exchange,
                                   base_assets: member_assets, quote_asset: @quote)
    @assets['BBB'][:ticker].update!(available: false)

    assert_not bot.valid?(:start)
    assert_predicate bot.errors[:allocations], :present?
  end

  test 'parse_params keeps the weights the sliders posted, without normalising' do
    bot = build_bot
    a, b = member_ids

    assert_equal({ a => 0.7, b => 0.3 }, bot.parse_params(allocations: { a => '70', b => '30' })[:allocations])
    bot.allocations = bot.parse_params(allocations: { a => '70', b => '70' })[:allocations]
    assert_equal({ a => 0.7, b => 0.7 }, bot.allocations)
    assert_in_delta 1.4, bot.allocations_total, 0.0001
    assert_not bot.allocations_balanced?
    assert_equal({ a => 0.0, b => 1.0 }, bot.parse_params(allocations: { a => '-5', b => '150' })[:allocations])
  end

  test 'a bot whose allocations do not add up to 100% cannot start' do
    bot = build_bot
    a, b = member_ids

    bot.allocations = { a => 0.7, b => 0.7 }
    assert_not bot.valid?(:start)
    assert_match(/140\.00%/, bot.errors[:allocations].to_sentence)

    bot.allocations = { a => 0.7, b => 0.3 }
    assert bot.valid?(:start)
  end

  test 'add_asset_id joins at zero and remove_asset_id moves nothing' do
    bot = build_bot
    a, b = member_ids
    c = @assets['CCC'][:asset].id.to_s

    added = bot.parse_params(add_asset_id: c)[:allocations]
    assert_equal({ a => 0.5, b => 0.5, c => 0.0 }, added)
    bot.allocations = added
    assert bot.allocations_balanced?

    removed = bot.parse_params(remove_asset_id: a)[:allocations]
    assert_equal({ b => 0.5, c => 0.0 }, removed)
    bot.allocations = removed
    assert_in_delta 0.5, bot.allocations_total, 0.0001
    assert_not bot.allocations_balanced?
  end

  test 'normalize_allocations squeezes the stored proportions to 100' do
    bot = build_bot
    a, b = member_ids
    bot.allocations = { a => 0.5, b => 0.25 }

    result = bot.parse_params(normalize_allocations: '1')[:allocations]

    assert_equal({ a => 0.667, b => 0.333 }, result)
  end

  test 'add and remove in one request compose' do
    bot = build_bot
    a, b = member_ids
    c = @assets['CCC'][:asset].id.to_s

    result = bot.parse_params(add_asset_id: c, remove_asset_id: a)[:allocations]

    assert_equal({ b => 0.5, c => 0.0 }, result)
  end

  test 'Normalize on all-zero weights splits them equally' do
    bot = build_bot
    a, b = member_ids
    bot.allocations = { a => 0.0, b => 0.0 }

    assert_equal({ a => 0.5, b => 0.5 }, bot.parse_params(normalize_allocations: 'true')[:allocations])
  end

  test 'Normalize applies to the sliders posted with it' do
    bot = build_bot
    a, b = member_ids

    result = bot.parse_params(allocations: { a => '30', b => '30' }, normalize_allocations: '1')[:allocations]

    assert_equal({ a => 0.5, b => 0.5 }, result)
  end

  test 'allocations_balanced? uses the same tolerance as the model' do
    bot = build_bot
    a, b = member_ids

    bot.allocations = { a => 0.5, b => 0.4995 }
    assert bot.allocations_balanced?

    bot.allocations = { a => 0.5, b => 0.498 }
    assert_not bot.allocations_balanced?
  end

  test 'interval, quote_amount, label parse like every other type' do
    result = build_bot.parse_params(interval: 'week', quote_amount: '42.5', label: 'Basket')

    assert_equal 'week', result[:interval]
    assert_equal 42.5, result[:quote_amount]
    assert_not_includes result, :label
  end

  test 'available exchanges are the venues where one quote lists every member' do
    exchange_x = @exchange
    exchange_y = create(:kraken_exchange)
    eur = create(:asset, :eur)
    create(:ticker, exchange: exchange_y, base_asset: @assets['AAA'][:asset], quote_asset: @quote)
    create(:ticker, exchange: exchange_y, base_asset: @assets['BBB'][:asset], quote_asset: eur)
    bot = build_bot
    bot.exchange = nil

    assert_equal [exchange_x.id], bot.available_exchanges_for_current_settings.pluck(:id)

    bot.quote_asset_id = eur.id
    assert_empty bot.available_exchanges_for_current_settings

    bot.allocations = { member_ids.first => 1.0 }
    bot.quote_asset_id = nil
    assert_equal [exchange_x.id, exchange_y.id].sort, bot.available_exchanges_for_current_settings.pluck(:id).sort
  end

  test 'available base assets exclude members and the quote and narrow to shared venues; quote candidates pair with every member' do
    candidate = create(:asset, symbol: 'DDD', external_id: 'ddd')
    eur = create(:asset, :eur)
    create(:ticker, exchange: @exchange, base_asset: candidate, quote_asset: @quote)
    member_assets.each { |asset| create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: eur) }
    bot = build_bot

    base_ids = bot.available_assets_for_current_settings(asset_type: :base_asset).pluck(:id)
    assert_includes base_ids, candidate.id
    assert_not_includes bot.available_assets_for_current_settings(asset_type: :base_asset).pluck(:id), @quote.id
    member_assets.each do |asset|
      assert_not_includes bot.available_assets_for_current_settings(asset_type: :base_asset).pluck(:id), asset.id
    end

    bot.quote_asset_id = nil
    assert_includes bot.available_assets_for_current_settings(asset_type: :quote_asset).pluck(:id), eur.id
  end

  test 'base candidates share a quote with every member, not just a venue' do
    eur = create(:asset, :eur)
    candidate_usd = create(:asset, symbol: 'USDONLY', external_id: 'usd-only')
    candidate_eur = create(:asset, symbol: 'EURONLY', external_id: 'eur-only')
    create(:ticker, exchange: @exchange, base_asset: candidate_usd, quote_asset: @quote)
    create(:ticker, exchange: @exchange, base_asset: candidate_eur, quote_asset: eur)
    bot = build(:dca_multi_asset, user: @user, exchange: @exchange,
                                  base_assets: [@assets['AAA'][:asset]], quote_asset: @quote)
    bot.quote_asset_id = nil

    ids = bot.available_assets_for_current_settings(asset_type: :base_asset).pluck(:id)
    assert_includes ids, candidate_usd.id
    assert_not_includes ids, candidate_eur.id

    bot.quote_asset_id = eur.id
    ids = bot.available_assets_for_current_settings(asset_type: :base_asset).pluck(:id)
    assert_not_includes ids, candidate_usd.id
    assert_not_includes ids, candidate_eur.id
  end

  test 'an unrelated quote_amount change of a stopped bot whose venue delisted a member still saves' do
    bot = create(:dca_multi_asset, :stopped, user: @user, exchange: @exchange,
                                             base_assets: member_assets, quote_asset: @quote)
    @assets['BBB'][:ticker].update!(available: false)
    bot.quote_amount = 123
    bot.set_missed_quote_amount
    assert bot.save

    bot.assign_attributes(bot.parse_params(allocations: { member_ids[0] => 80, member_ids[1] => 20 }))
    bot.set_missed_quote_amount
    assert_not bot.save
    assert_match(/BBB/, bot.errors[:allocations].to_sentence)
  end

  test 'a crafted exchange_id with no row behind it is a validation error, not a 500' do
    bot = build_bot
    bot.exchange_id = Exchange.maximum(:id).to_i + 10_000

    assert_not bot.valid?
    assert_predicate bot.errors[:exchange], :present?
  end

  test 'tickers cover every asset ever in the composition, composition_tickers only the current members' do
    # An unrelated pair on the same venue must not be in scope: Alpaca decides market hours and
    # buying power from bot.tickers, so a crypto-only basket must not drag equities in.
    extra = create(:asset, symbol: 'DDD', external_id: 'ddd')
    create(:ticker, exchange: @exchange, base_asset: extra, quote_asset: @quote)
    bot = create(:dca_multi_asset, user: @user, exchange: @exchange,
                                   base_assets: @assets.values.map { it[:asset] }, quote_asset: @quote)
    all = @assets.values.map { it[:ticker].id }.sort

    assert_equal all, bot.tickers.pluck(:id).sort

    # A removed asset keeps its row, so it stays priceable and sellable.
    Bots::DcaMultiAsset.any_instance.stubs(:broadcast_metrics_panel)
    bot.settings = bot.settings.merge('allocations' => bot.allocations_removing(@assets['AAA'][:asset].id))
    bot.set_missed_quote_amount
    bot.save!
    bot = Bot.find(bot.id)

    assert_equal all, bot.tickers.pluck(:id).sort
    assert_equal [@assets['BBB'][:asset].id, @assets['CCC'][:asset].id].sort, bot.composition_tickers.map(&:base_asset_id).sort
  end

  test 'tickers_for_start, decimals, minimum_for_exchange and composition_size read the composition' do
    @assets['AAA'][:ticker].update!(minimum_quote_size: 10, quote_decimals: 4)
    @assets['BBB'][:ticker].update!(minimum_quote_size: 25, quote_decimals: 2)
    bot = create(:dca_multi_asset, user: @user, exchange: @exchange,
                                   base_assets: member_assets, quote_asset: @quote)

    # Empty like the index bot's: Bot::RebalanceJob#resumable? pre-checks tickers_for_start BEFORE
    # before_rebalance can refresh the composition, so a delisted member would wedge every poll.
    # The per-asset filter in Bot::Rebalancer and validate_tickers_available on :start cover it.
    assert_empty bot.tickers_for_start
    assert_equal({ quote: 2 }, bot.decimals)
    assert_equal 25.0, bot.minimum_for_exchange
    assert_equal 2, bot.composition_size
  end

  test 'type predicates, scope, partial and title key' do
    bot = create(:dca_multi_asset, user: @user, exchange: @exchange,
                                   base_assets: member_assets, quote_asset: @quote)

    assert_predicate bot, :dca_multi_asset?
    assert_equal [bot.id], Bot.dca_multi_asset.pluck(:id)
    assert_equal 'bots/composition/metrics', bot.metrics_partial
    assert_equal 'bot.dca_multi_asset.removed_from_portfolio', bot.exited_title_key
  end

  test 'execute_action refreshes the composition, then buys every member' do
    bot = create(:dca_multi_asset, :started, user: @user, exchange: @exchange,
                                             base_assets: member_assets, quote_asset: @quote)
    Exchanges::Binance.any_instance.stubs(:get_ask_price).returns(Result::Success.new(100.to_d))

    result = bot.execute_action

    assert_predicate result, :success?
    assert_equal 2, bot.transactions.count
    assert_equal [50.0, 50.0], bot.transactions.pluck(:quote_amount).map(&:to_f).sort
    assert_predicate bot.reload, :waiting?
  end

  test 'the rule set is the index bot\'s' do
    ancestors = Bots::DcaMultiAsset.ancestors

    [Bot::SmartIntervalable, Bot::LimitOrderable, Bot::Rebalanceable].each do |concern|
      assert_includes ancestors, concern
    end
    [Bot::PriceLimitable, Bot::PriceDropLimitable, Bot::MovingAverageLimitable,
     Bot::IndicatorLimitable, Bot::QuoteAmountLimitable, Bot::Reversible].each do |concern|
      assert_not_includes ancestors, concern
    end
  end

  test 'an exchange change is refused while a rebalance is pending, even when stopped' do
    other = create(:kraken_exchange)
    member_assets.each { |asset| create(:ticker, exchange: other, base_asset: asset, quote_asset: @quote) }
    bot = create(:dca_multi_asset, :stopped, user: @user, exchange: @exchange,
                                             base_assets: member_assets, quote_asset: @quote)
    bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, remaining_quote_amount: 10)
    bot = Bot.find(bot.id)

    bot.exchange = other
    bot.set_missed_quote_amount
    assert_not bot.save
    assert_predicate bot.errors[:exchange], :present?

    bot.clear_rebalance_pending!
    bot = Bot.find(bot.id)
    Bots::DcaMultiAsset.any_instance.stubs(:broadcast_metrics_panel)
    bot.exchange = other
    bot.set_missed_quote_amount
    assert bot.save
  end

  test 'the composition is frozen while a rebalance is pending, even when stopped' do
    bot = create(:dca_multi_asset, :stopped, user: @user, exchange: @exchange,
                                             base_assets: @assets.values.map { it[:asset] }, quote_asset: @quote)
    bot.set_rebalance_pending!(phase: Bot::Rebalanceable::PHASE_BUYING, remaining_quote_amount: 10)
    bot = Bot.find(bot.id)

    bot.assign_attributes(bot.parse_params(remove_asset_id: @assets['CCC'][:asset].id))
    bot.set_missed_quote_amount
    assert_not bot.save
    assert_predicate bot.errors[:allocations], :present?

    bot.reload
    bot.assign_attributes(
      bot.parse_params(
        allocations: {
          @assets['AAA'][:asset].id => 60,
          @assets['BBB'][:asset].id => 20,
          @assets['CCC'][:asset].id => 20
        }
      )
    )
    bot.set_missed_quote_amount
    assert_not bot.save
    assert_predicate bot.errors[:allocations], :present?

    bot.reload
    bot.quote_amount = 125
    bot.set_missed_quote_amount
    assert bot.save
  end

  test 'the quote asset is immutable once the bot has transactions' do
    eur = create(:asset, :eur)
    member_assets.each { |asset| create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: eur) }
    bot = create(:dca_multi_asset, :stopped, user: @user, exchange: @exchange,
                                             base_assets: member_assets, quote_asset: @quote)
    create(:transaction, bot: bot, exchange: @exchange, base: 'AAA', quote: 'USD', side: :buy,
                         status: :submitted, external_status: :closed)
    bot = Bot.find(bot.id)
    Bots::DcaMultiAsset.any_instance.stubs(:broadcast_metrics_panel)

    bot.settings = bot.settings.merge('quote_asset_id' => eur.id)
    bot.set_missed_quote_amount
    assert_not bot.save
    assert_predicate bot.errors[:quote_asset_id], :present?
  end

  private

  def member_assets
    @assets.values.first(2).map { it[:asset] }
  end

  def member_ids
    member_assets.map { it.id.to_s }
  end

  def build_bot(exchange: @exchange)
    build(:dca_multi_asset, user: @user, exchange:, base_assets: member_assets, quote_asset: @quote)
  end
end
