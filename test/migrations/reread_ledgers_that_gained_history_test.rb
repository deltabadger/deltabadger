require 'test_helper'
require Dir[Rails.root.join('db/migrate/*_reread_ledgers_that_gained_history.rb')].first

class RereadLedgersThatGainedHistoryTest < ActiveSupport::TestCase
  setup { ActiveRecord::Migration.verbose = false }

  # Watermarks are all this migration touches; the venues' own key formats are beside the point.
  def synced_key(factory)
    build(:api_key, exchange: create(factory), key_type: :trading).tap do |key|
      key.save!(validate: false)
      key.update_column(:last_synced_at, 1.day.ago)
    end
  end

  test 'keys on the venues whose ledger gained history read it again; others keep their watermark' do
    widened = %i[bybit_exchange bitget_exchange coinbase_exchange gemini_exchange mexc_exchange bingx_exchange
                 bitvavo_exchange hyperliquid_exchange].map { |factory| synced_key(factory) }
    kept = synced_key(:kraken_exchange)

    RereadLedgersThatGainedHistory.new.up

    assert(widened.all? { |key| key.reload.last_synced_at.nil? })
    assert_not_nil kept.reload.last_synced_at
  end
end
