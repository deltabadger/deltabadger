require 'test_helper'

# A bot type with no fill-time reconciliation of its own — single-asset, signal — is protected only
# by the account ledger, and the ledger only hears about a sale when something asks it to sync. A
# resting limit sell fills long after it was placed, so the placement-time pull is not enough.
class TransactionLedgerSyncTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  # The app pins the SolidQueue adapter, and ActiveJob::TestHelper leaves a configured adapter alone.
  def queue_adapter_for_test
    ActiveJob::QueueAdapters::TestAdapter.new
  end

  setup do
    @bot = create(:dca_single_asset, user: create(:user))
    @order = @bot.transactions.create!(exchange: @bot.exchange, side: :sell, status: :submitted,
                                       external_status: :open, base: @bot.ticker.base,
                                       quote: @bot.ticker.quote, amount: 1, price: 100,
                                       order_type: :limit_order)
  end

  test 'a sell that fills asks the ledger to catch up' do
    assert_enqueued_with(job: AccountTransaction::SyncJob) do
      @order.update!(external_status: :closed, amount_exec: 1, quote_amount_exec: 90)
    end
  end

  test 'the ask is delayed past a sync that may already be running' do
    enqueued_jobs.clear # the placement already pulled one; this is about the FILL's
    @order.update!(external_status: :closed, amount_exec: 1, quote_amount_exec: 90)

    job = enqueued_jobs.find { |enqueued| enqueued['job_class'] == 'AccountTransaction::SyncJob' }
    assert job['scheduled_at'].present?,
           'the sync discards a conflict, so an immediate enqueue during one is simply dropped'
  end

  test 'a cancelled sell that partially filled asks too — it realised just as much' do
    assert_enqueued_with(job: AccountTransaction::SyncJob) do
      @order.update!(external_status: :cancelled, amount_exec: 0.4, quote_amount_exec: 36)
    end
  end

  test 'proceeds arriving after the status does still asks' do
    @order.update!(external_status: :cancelled, amount_exec: 0.4)

    assert_enqueued_with(job: AccountTransaction::SyncJob) do
      @order.update!(quote_amount_exec: 36)
    end
  end

  test 'a cancelled sell that never filled asks nothing' do
    assert_no_enqueued_jobs(only: AccountTransaction::SyncJob) do
      @order.update!(external_status: :cancelled)
    end
  end

  test 'a buy is not a disposal' do
    buy = @bot.transactions.create!(exchange: @bot.exchange, side: :buy, status: :submitted,
                                    external_status: :open, base: @bot.ticker.base,
                                    quote: @bot.ticker.quote, amount: 1, price: 100,
                                    order_type: :limit_order)

    assert_no_enqueued_jobs(only: AccountTransaction::SyncJob) do
      buy.update!(external_status: :closed, amount_exec: 1, quote_amount_exec: 100)
    end
  end
end
