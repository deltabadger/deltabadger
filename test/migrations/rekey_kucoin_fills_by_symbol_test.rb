require 'test_helper'
require Dir[Rails.root.join('db/migrate/*_rekey_kucoin_fills_by_symbol.rb')].first

class RekeyKucoinFillsBySymbolTest < ActiveSupport::TestCase
  setup do
    ActiveRecord::Migration.verbose = false
    @exchange = create(:kucoin_exchange)
    @connection = create(:api_key, exchange: @exchange, raw_passphrase: 'pass', last_synced_at: 1.hour.ago)
  end

  def fill(symbol)
    { 'symbol' => symbol, 'side' => 'buy', 'size' => '1', 'funds' => '10', 'feeCurrency' => 'USDT',
      'fee' => '0', 'tradeId' => '7', 'createdAt' => 2.hours.ago.to_i * 1000 }
  end

  def stored(raw, tx_id: raw['tradeId'])
    create(:account_transaction, api_key: @connection, entry_type: :buy, base_currency: raw['symbol'].split('-')[0],
                                 base_amount: 1, quote_currency: 'USDT', quote_amount: 10, tx_id: tx_id, raw_data: raw,
                                 transacted_at: Time.at(raw['createdAt'] / 1000).utc)
  end

  def resync_with(*fills)
    page = Result::Success.new({ 'code' => '200000', 'data' => { 'items' => fills, 'totalPage' => 1 } })
    empty = Result::Success.new({ 'code' => '200000', 'data' => { 'items' => [], 'totalPage' => 1 } })
    hm_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(hm_client)
    at = fills.first['createdAt']
    hm_client.stubs(:get_fills).returns(empty)
    hm_client.stubs(:get_fills).with { |**kw| kw[:start_at] <= at && at < kw[:end_at] }.returns(page)
    hm_client.stubs(:get_deposits).returns(empty)
    hm_client.stubs(:get_withdrawals).returns(empty)
    assert_predicate AccountTransactionSync.new(@connection).sync!, :success?
  end

  test 'a fill held under its bare tradeId moves to symbol-tradeId, and the one the collision dropped comes back' do
    row = stored(fill('BTC-USDT'))

    RekeyKucoinFillsBySymbol.new.up
    assert_nil @connection.reload.last_synced_at, 'the next sync must re-read the full history'
    resync_with(fill('BTC-USDT'), fill('ETH-USDT'))

    assert_equal 'BTC-USDT-7', row.reload.tx_id
    assert_equal %w[BTC-USDT-7 ETH-USDT-7], AccountTransaction.where(exchange: @exchange).order(:tx_id).pluck(:tx_id)
  end

  test 'a row is never moved onto an id the user already holds' do
    held = stored(fill('BTC-USDT'), tx_id: 'BTC-USDT-7')
    bare = stored(fill('BTC-USDT'))

    RekeyKucoinFillsBySymbol.new.up

    assert_equal 'BTC-USDT-7', held.reload.tx_id
    assert_equal '7', bare.reload.tx_id
  end

  test 'other venues and file-shaped rows are left alone' do
    other = create(:account_transaction, entry_type: :buy, tx_id: '7', raw_data: fill('BTC-USDT'))
    file_row = create(:account_transaction, api_key: @connection, entry_type: :buy, tx_id: 'csv-1', raw_data: { 'id' => 'y' })

    RekeyKucoinFillsBySymbol.new.up

    assert_equal '7', other.reload.tx_id
    assert_equal 'csv-1', file_row.reload.tx_id
  end
end
