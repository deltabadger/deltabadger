require 'test_helper'
require Dir[Rails.root.join('db/migrate/*_rekey_kucoin_transfers_by_record_id.rb')].first

class RekeyKucoinTransfersByRecordIdTest < ActiveSupport::TestCase
  setup do
    ActiveRecord::Migration.verbose = false
    @exchange = create(:kucoin_exchange)
    @api_key = create(:api_key, exchange: @exchange, raw_passphrase: 'pass', last_synced_at: 1.hour.ago)
  end

  def record(id: nil)
    { 'currency' => 'USDT', 'amount' => '5', 'fee' => '1', 'walletTxId' => '0xbatch',
      'createdAt' => 2.hours.ago.to_i * 1000 }.merge(id ? { 'id' => id } : {})
  end

  def stored(raw)
    create(:account_transaction, api_key: @api_key, entry_type: :withdrawal, base_currency: 'USDT', base_amount: 5,
                                 quote_currency: nil, quote_amount: nil, tx_id: '0xbatch', raw_data: raw,
                                 transacted_at: Time.at(raw['createdAt'] / 1000).utc)
  end

  # The sync re-reads a 25 h overlap, so a transfer held under its hash is read again under its id.
  def resync_with(raw)
    page = Result::Success.new({ 'code' => '200000', 'data' => { 'items' => [raw], 'totalPage' => 1 } })
    empty = Result::Success.new({ 'code' => '200000', 'data' => { 'items' => [], 'totalPage' => 1 } })
    hm_client = mock('honeymaker_client')
    Honeymaker.stubs(:client).returns(hm_client)
    hm_client.stubs(:get_fills).returns(empty)
    hm_client.stubs(:get_deposits).returns(empty)
    hm_client.stubs(:get_withdrawals).returns(page)
    assert_predicate AccountTransactionSync.new(@api_key).sync!, :success?
  end

  test 'a hash-keyed transfer moves to its record id, and a re-sync of it adds no row' do
    row = stored(record(id: 'w-1'))

    RekeyKucoinTransfersByRecordId.new.up
    resync_with(record(id: 'w-1'))

    assert_equal 'w-1', row.reload.tx_id
    assert_equal 1, AccountTransaction.where(exchange: @exchange).count
  end

  test 'a transfer whose record carried no id keeps its hash, and a re-sync of it adds no row' do
    row = stored(record)

    RekeyKucoinTransfersByRecordId.new.up
    resync_with(record)

    assert_equal '0xbatch', row.reload.tx_id
    assert_equal 1, AccountTransaction.where(exchange: @exchange).count
  end

  test 'other venues and file-shaped rows are left alone' do
    other = create(:account_transaction, entry_type: :withdrawal, tx_id: '0xother', raw_data: { 'id' => 'x', 'walletTxId' => '0xother' })
    file_row = create(:account_transaction, api_key: @api_key, entry_type: :deposit, tx_id: 'csv-1', raw_data: { 'id' => 'y' })

    RekeyKucoinTransfersByRecordId.new.up

    assert_equal '0xother', other.reload.tx_id
    assert_equal 'csv-1', file_row.reload.tx_id
  end
end
