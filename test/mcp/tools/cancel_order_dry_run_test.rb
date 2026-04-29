# frozen_string_literal: true

require 'test_helper'

class CancelOrderDryRunTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @bot = create(:dca_single_asset, user: @user)
    @exchange = @bot.exchange
    @transaction = create(:transaction, bot: @bot, exchange: @exchange, status: :submitted, external_status: :open,
                                        side: :buy, order_type: :limit_order, base: 'BTC', quote: 'USD',
                                        amount: 0.5, price: 50_000, external_id: 'order-dry-123')
    ActionMCP::Current.user = @user
    @user.set_mcp_tool_enabled('cancel_order', true)
  end

  teardown do
    ActionMCP::Current.reset
    Thread.current[:force_dry_run] = nil
  end

  test 'dry run enabled: response contains DRY RUN prefix' do
    @user.mcp_dry_run = true

    Transaction.any_instance.expects(:cancel).once.returns(Result::Success.new(@transaction))

    response = CancelOrderTool.new(order_id: @transaction.id.to_s).execute
    text = response.contents.first.text

    assert_match(/\[DRY RUN\]/, text)
    assert_match(/cancellation submitted/i, text)
  end

  test 'dry run disabled: response does not contain DRY RUN prefix' do
    @user.mcp_dry_run = false

    Transaction.any_instance.expects(:cancel).once.returns(Result::Success.new(@transaction))

    response = CancelOrderTool.new(order_id: @transaction.id.to_s).execute
    text = response.contents.first.text

    assert_no_match(/\[DRY RUN\]/, text)
    assert_match(/cancellation submitted/i, text)
  end

  test 'thread-local is cleaned up after execution' do
    @user.mcp_dry_run = true

    Transaction.any_instance.expects(:cancel).once.returns(Result::Success.new(@transaction))

    CancelOrderTool.new(order_id: @transaction.id.to_s).execute

    assert_nil Thread.current[:force_dry_run]
  end
end
