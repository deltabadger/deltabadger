require 'test_helper'
require Dir[Rails.root.join('db/migrate/*_upcase_bitrue_ledger_currencies.rb')].first

class UpcaseBitrueLedgerCurrenciesTest < ActiveSupport::TestCase
  setup do
    ActiveRecord::Migration.verbose = false
    @bitrue = create(:bitrue_exchange)
    @key = create(:api_key, exchange: @bitrue, key_type: :trading, key: 'k', secret: 's')
    @key.update_column(:last_synced_at, 1.day.ago)
  end

  def up = UpcaseBitrueLedgerCurrencies.new.up

  def row(exchange: @bitrue, **attrs)
    AccountTransaction.create!({ user: @key.user, exchange: exchange, entry_type: :buy, base_currency: 'eth',
                                 base_amount: 1, quote_currency: 'usdt', quote_amount: 2000, fee_currency: 'usdt',
                                 fee_amount: 1, tx_id: SecureRandom.hex(4), transacted_at: 2.days.ago }.merge(attrs))
  end

  test 'Bitrue ledger currencies are upper-cased and the ledger is read again' do
    old = row
    old.update_column(:updated_at, 1.week.ago)
    other = row(exchange: create(:binance_exchange), base_currency: 'eth')

    up

    old.reload
    assert_equal %w[ETH USDT USDT], [old.base_currency, old.quote_currency, old.fee_currency]
    assert_operator old.updated_at, :>, 1.minute.ago, 'the tracker cache key moves'
    assert_equal 'eth', other.reload.base_currency, 'other venues untouched'
    assert_nil @key.reload.last_synced_at, 'the next sync reads the full history'
  end

  test 'a row already upper case keeps its updated_at' do
    clean = row(base_currency: 'BTC', quote_currency: 'USDT', fee_currency: nil)
    clean.update_column(:updated_at, 1.week.ago)

    up

    assert_operator clean.reload.updated_at, :<, 1.day.ago
  end
end
