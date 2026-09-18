require 'test_helper'

# The one-time backfill of transactions.base_asset_id / quote_asset_id. Rows recorded only a symbol string,
# and a string can name several assets, so a row is resolved from what the bot could have traded — and left
# NULL whenever more than one asset fits. Raw SQL: it runs inside a migration.
class Transaction::AssetBackfillTest < ActiveSupport::TestCase
  def setup
    @usd = create(:asset, :usd)
    @user = create(:user)
  end

  # == single-asset bots: structural ==

  test "a single-asset bot's rows are its one pair, whatever string they carry" do
    btc = create(:asset, :bitcoin)
    bot = create(:dca_single_asset, user: @user, base_asset: btc, quote_asset: @usd)
    ids = [row(bot, 'BTC'), row(bot, 'OLDNAME'), row(bot, nil)].map(&:id)

    backfill

    assert_equal [[btc.id, @usd.id]] * 3, Transaction.where(id: ids).order(:id).pluck(:base_asset_id, :quote_asset_id)
  end

  # == baskets: their members ==

  test "a basket's row is the member it names, by symbol or venue spelling" do
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    kraken = create(:kraken_exchange)
    create(:ticker, exchange: kraken, base_asset: btc, quote_asset: @usd, base_symbol: 'XBT')
    bot = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [btc, eth])
    by_symbol = row(bot, 'BTC')
    by_spelling = row(bot, 'XBT')
    eth_row = row(bot, 'ETH')

    backfill

    assert_equal([btc.id, btc.id, eth.id], [by_symbol, by_spelling, eth_row].map { |t| t.reload.base_asset_id })
  end

  test 'a string two basket members answer to stays NULL' do
    fan, portuma, mexc = por_assets
    bot = create(:dca_multi_asset, user: @user, exchange: mexc, quote_asset: @usd, base_assets: [fan, portuma])
    ambiguous = row(bot, 'POR')
    spelled = row(bot, 'PORTUMA')

    backfill

    assert_nil ambiguous.reload.base_asset_id
    assert_equal portuma.id, spelled.reload.base_asset_id
  end

  # == index bots ==

  test "an index bot's regular row is its member; a non-member is the one asset the venue knows by that name" do
    btc = create(:asset, :bitcoin)
    sol = create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana')
    kraken = create(:kraken_exchange)
    xbt = create(:ticker, exchange: kraken, base_asset: btc, quote_asset: @usd, base_symbol: 'XBT')
    create(:ticker, exchange: kraken, base_asset: sol, quote_asset: @usd)
    bot = index_bot(kraken, xbt)
    member = row(bot, 'BTC')
    redeploy = row(bot, 'BTC', transaction_type: 'REDEPLOY')
    outside = row(bot, 'SOL')

    backfill

    assert_equal([btc.id, btc.id, sol.id], [member, redeploy, outside].map { |t| t.reload.base_asset_id })
  end

  # Rebalance legs picked tickers by the member's SYMBOL matched against the venue's SPELLING, across the whole
  # venue, and recorded the traded ticker's asset symbol. Members AAA (spelled AAAX) and BBB; a non-member whose
  # symbol is BBB is spelled AAA on the venue — rebalancing AAA may have traded it and recorded 'BBB'.
  test 'a rebalance row that may have been routed to another asset stays NULL' do
    exchange = create(:mexc_exchange)
    aaa = listed('AAA', exchange, spelled: 'AAAX')
    bbb = listed('BBB', exchange)
    impostor = create(:asset, symbol: 'BBB', name: 'Impostor', external_id: 'impostor')
    create(:ticker, exchange:, base_asset: impostor, quote_asset: @usd, base_symbol: 'AAA')
    bot = index_bot(exchange, *[aaa, bbb].map { |asset| asset_ticker(exchange, asset) })
    regular = row(bot, 'BBB')
    rebalance = row(bot, 'BBB', transaction_type: 'REBALANCE')

    backfill

    assert_equal bbb.id, regular.reload.base_asset_id, 'a regular leg trades the member ticker'
    assert_nil rebalance.reload.base_asset_id
  end

  # Members AAA and NNN (spelled AAAX and NNNX). Non-members: BBB spelled AAA, CCC spelled BBB, and CCC' (also
  # symbol CCC) spelled NNN. A misrouted rebalance bought BBB; liquidating the held 'BBB' routed to CCC. That
  # liquidation row says 'CCC' and must not be read as CCC', the only CCC reachable through a member's symbol.
  test 'a liquidation row routed through a held string stays NULL, never the wrong same-symbol asset' do
    exchange = create(:mexc_exchange)
    aaa = listed('AAA', exchange, spelled: 'AAAX')
    nnn = listed('NNN', exchange, spelled: 'NNNX')
    bbb = create(:asset, symbol: 'BBB', name: 'Bee', external_id: 'bee')
    create(:ticker, exchange:, base_asset: bbb, quote_asset: @usd, base_symbol: 'AAA')
    ccc = create(:asset, symbol: 'CCC', name: 'Sea', external_id: 'sea')
    create(:ticker, exchange:, base_asset: ccc, quote_asset: @usd, base_symbol: 'BBB')
    other_ccc = create(:asset, symbol: 'CCC', name: 'Sea Two', external_id: 'sea-two')
    create(:ticker, exchange:, base_asset: other_ccc, quote_asset: @usd, base_symbol: 'NNN')
    bot = index_bot(exchange, asset_ticker(exchange, aaa), asset_ticker(exchange, nnn))
    row(bot, 'BBB', transaction_type: 'REBALANCE')
    liquidation = row(bot, 'CCC', transaction_type: 'LIQUIDATION')

    backfill

    assert_nil liquidation.reload.base_asset_id
  end

  test 'a tombstoned venue spelling is compared without its prefix' do
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    kraken = create(:kraken_exchange)
    xbt = create(:ticker, exchange: kraken, base_asset: btc, quote_asset: @usd, base_symbol: 'XBT')
    bot = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [btc, eth])
    xbt.update_columns(base: "__stale_#{xbt.id}_XBT", available: false)
    spelled = row(bot, 'XBT')

    backfill

    assert_equal btc.id, spelled.reload.base_asset_id
  end

  # == quote, reruns, what it never touches ==

  test "a row's own quote id is kept and used to resolve its base" do
    btc = create(:asset, :bitcoin)
    eur = create(:asset, :eur)
    bot = create(:dca_multi_asset, user: @user, quote_asset: @usd, base_assets: [btc, create(:asset, :ethereum)])
    create(:ticker, exchange: bot.exchange, base_asset: btc, quote_asset: eur)
    kept = row(bot, 'BTC', quote: 'USD', quote_asset_id: eur.id)

    backfill

    assert_equal [btc.id, eur.id], [kept.reload.base_asset_id, kept.quote_asset_id]
  end

  test 'a second run changes nothing, and an id already on a row is never overwritten' do
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    bot = create(:dca_multi_asset, user: @user, quote_asset: @usd, base_assets: [btc, eth])
    set_by_new_code = row(bot, 'BTC', base_asset_id: eth.id)
    open = row(bot, 'BTC')

    backfill
    first = Transaction.order(:id).pluck(:id, :base_asset_id, :quote_asset_id)
    backfill

    assert_equal first, Transaction.order(:id).pluck(:id, :base_asset_id, :quote_asset_id)
    assert_equal eth.id, set_by_new_code.reload.base_asset_id
    assert_equal btc.id, open.reload.base_asset_id
  end

  test 'a bot with unreadable settings has its rows left NULL and counted' do
    broken = create(:dca_single_asset, user: @user, base_asset: create(:asset, :bitcoin), quote_asset: @usd)
    unreadable = row(broken, 'BTC')
    Bot.where(id: broken.id).update_all("settings = 'not json'")

    report = backfill

    assert_nil unreadable.reload.base_asset_id
    assert_equal 1, report[:unresolved_rows]
  end

  private

  def backfill = Transaction::AssetBackfill.run!(ActiveRecord::Base.connection)

  def row(bot, base, quote: 'USD', transaction_type: 'REGULAR', **ids)
    create(:transaction, bot:, exchange: bot.exchange, status: :submitted, external_status: :closed, side: :buy,
                         transaction_type:, external_id: "r-#{SecureRandom.hex(4)}", base:, quote:, price: 1, amount: 1,
                         amount_exec: 1, quote_amount: 1, quote_amount_exec: 1, resolve_asset_ids: false,
                         base_asset_id: nil, quote_asset_id: nil, **ids)
  end

  def por_assets
    fan = create(:asset, symbol: 'POR', name: 'Portugal Fan Token', external_id: 'por-fan')
    portuma = create(:asset, symbol: 'POR', name: 'Portuma', external_id: 'portuma')
    mexc = create(:mexc_exchange)
    create(:ticker, exchange: mexc, base_asset: fan, quote_asset: @usd)
    create(:ticker, exchange: mexc, base_asset: portuma, quote_asset: @usd, base_symbol: 'PORTUMA')
    [fan, portuma, mexc]
  end

  def listed(symbol, exchange, spelled: symbol)
    asset = create(:asset, symbol:, name: symbol, external_id: symbol.downcase)
    create(:ticker, exchange:, base_asset: asset, quote_asset: @usd, base_symbol: spelled)
    asset
  end

  def asset_ticker(exchange, asset) = Ticker.find_by!(exchange:, base_asset: asset, quote_asset: @usd)

  def index_bot(exchange, *tickers)
    bot = create(:dca_index, user: @user, exchange:, quote_asset: @usd)
    tickers.each do |ticker|
      BotIndexAsset.create!(bot:, asset: ticker.base_asset, ticker:, target_allocation: 1.0 / tickers.size,
                            in_index: true, entered_at: Time.current)
    end
    bot
  end
end
