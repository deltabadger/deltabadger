require 'test_helper'
require Rails.root.join('db/migrate/20260727120000_retire_bitmart_exchange.rb')

# The migration has two branches, and which one runs is decided by whether anything still points at
# the venue. Every production container takes the delete branch (the fleet scan found zero
# references of any kind); a self-hosted install holding Bitmart history takes the retain branch,
# where trade records — tax records — must survive untouched.
class RetireBitmartExchangeTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @bitmart = Exchanges::Bitmart.create!(name: 'Bitmart', available: true)
    @ticker = create(:ticker, exchange: @bitmart, available: true, trading_enabled: true)
    # The ticker factory already links both assets to the exchange.
    @exchange_asset = ExchangeAsset.find_by!(exchange: @bitmart, asset: @ticker.base_asset)
  end

  # ── delete branch ─────────────────────────────────────────────────────────

  test 'a clean install loses the exchange, its tickers and its exchange_assets' do
    migrate!

    assert_empty Exchange.where(type: 'Exchanges::Bitmart')
    assert_empty Ticker.where(id: @ticker.id)
    assert_empty ExchangeAsset.where(id: @exchange_asset.id)
  end

  test 'live exchanges are untouched' do
    binance = create(:binance_exchange)
    binance_ticker = create(:ticker, exchange: binance)

    migrate!

    assert_predicate binance.reload, :available?
    assert_predicate binance_ticker.reload, :available?
  end

  # ── retain branch ─────────────────────────────────────────────────────────

  test 'an install holding a Bitmart bot keeps the row and tickers, but untradeable' do
    create(:dca_single_asset, user: @user, exchange: @bitmart, with_api_key: false)

    migrate!

    refute_predicate @bitmart.reload, :available?
    refute_predicate @ticker.reload, :available?
    refute_predicate @ticker, :trading_enabled?
  end

  test 'trade history survives the retain branch untouched' do
    bot = create(:dca_single_asset, user: @user, exchange: @bitmart, with_api_key: false)
    history = create(:transaction, bot: bot, exchange: @bitmart, status: :submitted,
                                   external_status: :closed, external_id: 'hist-1')

    migrate!

    history.reload
    assert_equal 'submitted', history.status
    assert_equal 'closed', history.external_status
  end

  test 'a working bot is stopped with the retired message; a deleted bot stays deleted' do
    # Both statuses are set the way the world produced them — the bots were already in these
    # states when the venue died, which is before any of the new guards existed.
    # Both bots share the seeded pair — a second bot building its own :bitcoin asset would collide
    # on external_id.
    assets = { base_asset: @ticker.base_asset, quote_asset: @ticker.quote_asset }
    working = create(:dca_single_asset, user: @user, exchange: @bitmart, with_api_key: false, **assets)
    working.update_column(:status, Bot.statuses[:scheduled])
    deleted = create(:dca_single_asset, user: @user, exchange: @bitmart, with_api_key: false, **assets)
    deleted.update_column(:status, Bot.statuses[:deleted])

    migrate!

    assert_predicate working.reload, :stopped?
    assert_equal 'bot.status.exchange_retired', working.stop_message_key
    assert_predicate deleted.reload, :deleted?, 'a deleted bot must not be resurrected as stopped'
  end

  test 'an order still being polled is abandoned so the exchange can be switched' do
    bot = create(:dca_single_asset, user: @user, exchange: @bitmart, with_api_key: false)
    open_order = create(:transaction, bot: bot, exchange: @bitmart, status: :submitted,
                                      external_status: :open, external_id: 'open-1')
    unknown_order = create(:transaction, bot: bot, exchange: @bitmart, status: :submitted,
                                         external_status: :unknown, external_id: 'unknown-1')

    migrate!

    assert_equal 'abandoned', open_order.reload.external_status
    assert_equal 'abandoned', unknown_order.reload.external_status
    assert_empty bot.reload.transactions.waiting
  end

  # Balance rows are snapshots refreshed from a trading key. The migration deletes that key, so a
  # surviving row could never be refreshed or cleared — and the tracker portfolio sums every
  # nonzero balance across all exchanges, so it would inflate the user's total forever.
  test 'balance snapshots are deleted even when the exchange row is retained' do
    create(:dca_single_asset, user: @user, exchange: @bitmart, with_api_key: false)
    balance = AccountBalance.create!(user: @user, exchange: @bitmart, asset: @ticker.base_asset,
                                     free: 1.5, locked: 0, usd_value: 90_000, synced_at: Time.current)

    migrate!

    assert Exchange.exists?(type: 'Exchanges::Bitmart'), 'this case must take the retain branch'
    assert_empty AccountBalance.where(id: balance.id)
  end

  test 'keys are deleted and the account_transactions that referenced them are nullified' do
    key = build(:api_key, user: @user, exchange: @bitmart)
    key.save!(validate: false)
    account_txn = create(:account_transaction, user: @user, api_key: key, exchange: @bitmart)
    create(:dca_single_asset, user: @user, exchange: @bitmart, with_api_key: false)

    migrate!

    assert_empty ApiKey.where(id: key.id)
    assert_nil account_txn.reload.api_key_id
    assert_predicate account_txn, :persisted?
  end

  # A raw-SQL stop cannot cancel Solid Queue work, and cancel_scheduled_action_jobs would only
  # reach the ACTION job anyway — a delayed limit check (queue :default) outlives both a stop and a
  # later start, and could hand off to a second chain once the bot is waiting on its NEW exchange.
  test 'queued jobs for a retired bot are cleared' do
    assets = { base_asset: @ticker.base_asset, quote_asset: @ticker.quote_asset }
    bot = create(:dca_single_asset, user: @user, exchange: @bitmart, with_api_key: false, **assets)
    other = create(:dca_single_asset, user: @user, exchange: create(:binance_exchange), **assets)

    Bot::PriceLimitCheckJob.set(wait: 1.hour).perform_later(bot)
    Bot::ActionJob.set(wait: 1.hour).perform_later(bot)
    Bot::PriceLimitCheckJob.set(wait: 1.hour).perform_later(other)

    migrate!

    assert_empty queued_job_gids & ["gid://#{GlobalID.app}/#{bot.class}/#{bot.id}"],
                 'no queued job may still reference the stranded bot'
    assert_includes queued_job_gids, "gid://#{GlobalID.app}/#{other.class}/#{other.id}",
                    'a bot on a live exchange keeps its schedule'
  end

  # bot_index_assets.ticker_id is NOT NULL with an FK, so a stale row from an index bot that moved
  # away would make the ticker delete fail. Retention is the safe answer.
  test 'a stale bot_index_asset forces the retain branch' do
    bot = create(:dca_single_asset, user: @user, exchange: create(:binance_exchange))
    insert_bot_index_asset(bot_id: bot.id, asset_id: @ticker.base_asset_id, ticker_id: @ticker.id)

    migrate!

    assert Exchange.exists?(type: 'Exchanges::Bitmart'), 'the row must be kept, not deleted'
    assert Ticker.exists?(@ticker.id)
    refute_predicate @ticker.reload, :available?
  end

  # ── index JSON, both branches ─────────────────────────────────────────────

  test 'index availability maps lose their Bitmart keys' do
    index = Index.create!(
      external_id: 'top-10', source: 'coingecko', name: 'Top 10',
      top_coins: %w[bitcoin ethereum],
      available_exchanges: { 'Exchanges::Binance' => 9, 'Exchanges::Bitmart' => 7 },
      top_coins_by_exchange: { 'Exchanges::Binance' => %w[bitcoin], 'Exchanges::Bitmart' => %w[bitcoin] }
    )

    migrate!

    index.reload
    assert_equal({ 'Exchanges::Binance' => 9 }, index.available_exchanges)
    assert_equal({ 'Exchanges::Binance' => %w[bitcoin] }, index.top_coins_by_exchange)
  end

  test 'the migration is irreversible' do
    assert_raises(ActiveRecord::IrreversibleMigration) { RetireBitmartExchange.new.down }
  end

  private

  def migrate!
    RetireBitmartExchange.new.tap { |m| m.verbose = false }.up
  end

  def queued_job_gids
    SolidQueue::Job.all.flat_map do |job|
      args = job.arguments.is_a?(Hash) ? job.arguments['arguments'] : job.arguments
      Array(args).filter_map { |arg| arg.is_a?(Hash) ? arg['_aj_globalid'] : nil }
    end
  end

  def insert_bot_index_asset(bot_id:, asset_id:, ticker_id:)
    now = Time.current.to_fs(:db)
    ActiveRecord::Base.connection.execute(<<~SQL.squish)
      INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, created_at, updated_at)
      VALUES (#{bot_id}, #{asset_id}, #{ticker_id}, '#{now}', '#{now}')
    SQL
  end
end
