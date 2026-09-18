require 'test_helper'

# A split restates a holding — one asset — whatever symbol its rows were recorded under. A report that names
# exactly one asset on its venue restates that asset's holding and no other; one that names none or several
# restates every holding with a row under its string, as before.
class Bot::Composition::RestatedIdentityTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:alpaca_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
    @usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    @klac = create(:asset, external_id: 'klac', symbol: 'KLAC')
    @aapl = create(:asset, external_id: 'aapl', symbol: 'AAPL')
    @bot = create(:dca_multi_asset, user: @user, exchange: @exchange, with_api_key: false,
                                    base_assets: [@klac, @aapl], quote_asset: @usd)
  end

  test 'rows recorded under an older symbol are restated by a report under the current one' do
    buy('KLA', asset: @klac, at: 8.days.ago)
    split('KLAC')

    assert_equal({ 'KLAC' => 20 }, held)
  end

  test 'a report that names no asset still restates the holding with rows under its string, once' do
    buy('OLDKLAC', asset: @klac, at: 8.days.ago)
    split('OLDKLAC')

    assert_equal({ 'KLAC' => 20 }, held)
  end

  test 'a holding with its asset and one without, both under the reported name, are each restated once' do
    buy('KLAC', asset: @klac, at: 8.days.ago)
    buy('KLAC', asset: nil, at: 7.days.ago)
    split('KLAC')

    assert_equal({ "KLAC##{@klac.id}" => 20, 'KLAC#?' => 20 }, held)
  end

  test 'one split reported under the spelling and the symbol restates once' do
    Ticker.find_by!(exchange: @exchange, base_asset: @klac).update!(base: 'KLACX', ticker: 'KLACXUSD')
    buy('KLAC', asset: @klac, at: 8.days.ago)
    split('KLAC')
    split('KLACX')

    assert_equal({ 'KLAC' => 20 }, held, '10x, not 100x')
  end

  test 'a report naming one asset leaves another holding recorded under that string alone' do
    renamed = create(:asset, external_id: 'xyz', symbol: 'XYZ')
    create(:ticker, exchange: @exchange, base_asset: renamed, quote_asset: @usd)
    buy('KLAC', asset: @klac, at: 8.days.ago)
    buy('KLAC', asset: renamed, at: 8.days.ago) # recorded under the name it had before
    split('KLAC')

    assert_equal({ 'KLAC' => 20, 'XYZ' => 2 }, held)
  end

  test 'reported names are resolved in one batch, not a query each' do
    names = %w[AAA BBB CCC DDD EEE FFF]
    names.each_with_index do |name, index|
      buy(name, asset: nil, at: (30 - index).days.ago) # recorded before orders stored their asset
      split(name, at: (20 - index).days.ago) if index.zero?
    end
    one = count_queries { @bot.metrics(force: true) }
    names.drop(1).each_with_index { |name, index| split(name, at: (19 - index).days.ago) }

    assert_equal one, count_queries { @bot.metrics(force: true) }, 'five more reported names, no more queries'
  end

  test 'a restatement expires a bot that holds the asset under an older symbol' do
    buy('KLA', asset: @klac, at: 8.days.ago)

    assert_difference -> { @bot.reload.restatement_generation } do
      AccountTransactionSync.expire_restated_bots(user: @user, exchange: @exchange, symbol: 'KLAC')
    end
  end

  test 'the split announcement finds holders by asset, and by name before the asset column exists' do
    buy('KLA', asset: @klac, at: 8.days.ago)
    announce = -> { AccountTransactionSync.announce_split(user: @user, exchange: @exchange, symbol: 'KLAC', at: 1.day.ago) }

    assert_difference -> { @bot.bot_activity_logs.where(event: 'asset_split').count } do
      announce.call
    end

    older_columns = Transaction.column_names - %w[base_asset_id]
    Transaction.stubs(:column_names).returns(older_columns)
    @bot.bot_activity_logs.delete_all
    assert_nothing_raised { announce.call }
  end

  private

  def buy(symbol, asset:, at:)
    create(:transaction, bot: @bot, exchange: @exchange, base: symbol, quote: 'USD', side: :buy, amount: 2,
                         amount_exec: 2, price: 100, quote_amount: 200, quote_amount_exec: 200, created_at: at,
                         resolve_asset_ids: false, base_asset_id: asset&.id, quote_asset_id: @usd.id)
  end

  def split(symbol, at: 5.days.ago)
    create(:account_transaction, user: @user, api_key: @api_key, exchange: @exchange, entry_type: :adjustment,
                                 base_currency: symbol, base_amount: 90, quote_currency: nil, quote_amount: nil,
                                 transacted_at: at, raw_data: { 'corporate_action' => 'split', 'split_ratio' => '10:1' })
  end

  def held = @bot.metrics(force: true)[:asset_breakdown].transform_values { |holding| holding[:amount].to_i }

  def count_queries(&)
    count = 0
    counter = ->(*, payload) { count += 1 unless payload[:name] == 'SCHEMA' || payload[:cached] }
    ActiveSupport::Notifications.subscribed(counter, 'sql.active_record', &)
    count
  end
end
