# frozen_string_literal: true

require 'test_helper'

class BotApi::Tracker::SetTransferLinkTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @api_key = create(:api_key, user: @user)
    @at = Time.zone.parse('2026-08-01 12:00:00')
    # The pair is only linkable because of these four things: same base_currency (the factory's
    # BTC), the deposit after the withdrawal and inside 14 days, and no bigger than it.
    @withdrawal = create(:account_transaction, :withdrawal, api_key: @api_key, base_amount: 1, transacted_at: @at)
    @deposit = create(:account_transaction, :deposit, api_key: @api_key, base_amount: 0.999,
                                                      transacted_at: @at + 2.days)
    Tracker::LedgerJob.stubs(:perform_later)
  end

  def link(id, linked) = BotApi::Tracker::SetTransferLink.call(user: @user, transaction_id: id, linked: linked)

  test 'links a withdrawal to its one candidate deposit and asks for a snapshot rebuild' do
    PortfolioSnapshot::BackfillJob.expects(:perform_later).with(@user.id)
    Tracker::LedgerJob.expects(:perform_later).with(@user.id)

    result = link(@withdrawal.id, true)

    assert result.success?, result.error_message
    assert_equal @deposit.id, @withdrawal.reload.linked_transaction_id
    assert_equal({ withdrawal_id: @withdrawal.id, deposit_id: @deposit.id, linked: true }, result.data)
  end

  test 'linking from the deposit side works too' do
    PortfolioSnapshot::BackfillJob.stubs(:perform_later)
    assert link(@deposit.id, true).success?
    assert_equal @deposit.id, @withdrawal.reload.linked_transaction_id
  end

  test 'unlinking is sticky so the matcher does not redo it' do
    PortfolioSnapshot::BackfillJob.stubs(:perform_later)
    @withdrawal.update!(linked_transaction_id: @deposit.id)

    assert link(@withdrawal.id, false).success?

    @withdrawal.reload
    assert_nil @withdrawal.linked_transaction_id
    assert @withdrawal.transfer_link_rejected
  end

  test 'refusals' do
    PortfolioSnapshot::BackfillJob.expects(:perform_later).never
    assert_equal 'not_linked', link(@withdrawal.id, false).error_code
    assert_equal 'transaction_not_found', link(0, true).error_code
    @deposit.destroy
    assert_equal 'no_transfer_candidate', link(@withdrawal.id, true).error_code
  end

  test 'a row already linked cannot be linked again' do
    PortfolioSnapshot::BackfillJob.stubs(:perform_later)
    assert link(@withdrawal.id, true).success?
    assert_equal 'already_linked', link(@withdrawal.id, true).error_code
  end

  test 'two candidates are an ambiguity, not a guess' do
    create(:account_transaction, :deposit, api_key: @api_key, base_amount: 0.998, transacted_at: @at + 3.days)
    PortfolioSnapshot::BackfillJob.expects(:perform_later).never

    assert_equal 'ambiguous_transfer_candidate', link(@withdrawal.id, true).error_code
  end

  # A missing value must never unlink: that would rewrite history on an omitted parameter.
  test 'linked must be a real boolean' do
    PortfolioSnapshot::BackfillJob.expects(:perform_later).never

    [nil, 'yes', ''].each { |value| assert_equal 'linked_required', link(@withdrawal.id, value).error_code, value.inspect }
    assert_nil @withdrawal.reload.linked_transaction_id
  end

  # The unique index on linked_transaction_id is the lock; this is its message, not a 500.
  test 'a concurrent link is a conflict' do
    AccountTransaction.any_instance.stubs(:update!).raises(ActiveRecord::RecordNotUnique, 'UNIQUE constraint failed')

    result = link(@withdrawal.id, true)

    assert_equal 'already_linked', result.error_code
    assert_equal :conflict, result.status
  end
end
