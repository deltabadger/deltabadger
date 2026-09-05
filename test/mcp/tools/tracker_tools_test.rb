# frozen_string_literal: true

require 'test_helper'

class TrackerToolsTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @api_key = create(:api_key, user: @user)
    @at = Time.zone.parse('2026-08-01 12:00:00')
    @withdrawal = create(:account_transaction, :withdrawal, api_key: @api_key, base_amount: 1, transacted_at: @at)
    @deposit = create(:account_transaction, :deposit, api_key: @api_key, base_amount: 0.999,
                                                      transacted_at: @at + 2.days)
    Tracker::LedgerJob.stubs(:perform_later)
    PortfolioSnapshot::BackfillJob.stubs(:perform_later)
    %w[sync_tracker set_transfer_link set_transaction_price list_account_transactions]
      .each { |tool| @user.set_mcp_tool_enabled(tool, true) }
    stub_mcp_client(@user)
  end

  teardown { ActionMCP::Current.reset }

  test 'sync_tracker queues both syncs and names the venue' do
    AccountTransaction::SyncTrackerJob.expects(:perform_later).with(@user.id, [@api_key.id])
    AccountBalance::SyncJob.expects(:perform_later).with(@user.id, [@api_key.id])

    assert_match(/Sync started for Binance/, SyncTrackerTool.new.execute.contents.first.text)
  end

  test 'sync_tracker says so when nothing can read history' do
    @user.api_keys.destroy_all
    AccountTransaction::SyncTrackerJob.expects(:perform_later).never

    assert_match(/No exchange key can read history/, SyncTrackerTool.new.execute.contents.first.text)
  end

  test 'set_transfer_link links and unlinks the pair' do
    text = SetTransferLinkTool.new(transaction_id: @withdrawal.id, linked: true).execute.contents.first.text
    assert_match(/Linked withdrawal ##{@withdrawal.id} with deposit ##{@deposit.id}/, text)
    assert_equal @deposit.id, @withdrawal.reload.linked_transaction_id

    text = SetTransferLinkTool.new(transaction_id: @withdrawal.id, linked: false).execute.contents.first.text
    assert_match(/Unlinked withdrawal ##{@withdrawal.id}/, text)
    assert_nil @withdrawal.reload.linked_transaction_id
  end

  test 'set_transaction_price states and clears a price' do
    text = SetTransactionPriceTool.new(transaction_id: @deposit.id, price_usd: '123.45').execute.contents.first.text
    assert_match(/priced at 123.45 USD/, text)

    text = SetTransactionPriceTool.new(transaction_id: @deposit.id, price_usd: '').execute.contents.first.text
    assert_match(/Stated price cleared/, text)
    assert_nil @deposit.reload.manual_value(:price)
  end

  test 'set_transaction_price refuses garbage rather than clearing' do
    SetTransactionPriceTool.new(transaction_id: @deposit.id, price_usd: '50').execute

    text = SetTransactionPriceTool.new(transaction_id: @deposit.id, price_usd: 'abc').execute.contents.first.text

    assert_match(/must be a non-negative number/, text)
    assert_equal BigDecimal('50'), @deposit.reload.manual_value(:price)
  end

  # Without an id in the rendered line there is nothing for the two tools above to act on.
  test 'every row names its id, its transfer link and its stated price' do
    @withdrawal.update!(linked_transaction: @deposit)
    @withdrawal.set_manual(:price, '123.45')
    @withdrawal.save!

    text = ListAccountTransactionsTool.new.execute.contents.first.text

    assert_match(/##{@withdrawal.id} /, text)
    assert_match(/transfer ↔ ##{@deposit.id}/, text)
    assert_match(/stated price 123.45 USD/, text)
  end
end
