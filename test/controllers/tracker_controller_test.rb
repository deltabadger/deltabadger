require 'test_helper'

class TrackerControllerTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    create(:user, admin: true, setup_completed: true) # platform requires an admin to exist
    @user = create(:user, setup_completed: true)
    @api_key = create(:api_key, user: @user)
    @transacted_at = Time.zone.parse('2026-08-01 12:00:00')
    self.default_url_options = { locale: nil }
    sign_in @user
  end

  test 'links a withdrawal to its only eligible deposit' do
    withdrawal = create_transaction(:withdrawal, base_amount: 1, transacted_at: @transacted_at)
    deposit = create_transaction(:deposit, base_amount: 0.999, transacted_at: @transacted_at + 2.days)

    patch toggle_transfer_tracker_transaction_path(withdrawal)

    assert_response :success
    assert_equal deposit.id, withdrawal.reload.linked_transaction_id
  end

  test 'unlinks a linked pair from the withdrawal' do
    withdrawal, = create_linked_pair

    patch toggle_transfer_tracker_transaction_path(withdrawal)

    assert_response :success
    assert_nil withdrawal.reload.linked_transaction_id
  end

  test 'unlinks a linked pair from the deposit' do
    withdrawal, deposit = create_linked_pair

    patch toggle_transfer_tracker_transaction_path(deposit)

    assert_response :success
    assert_nil withdrawal.reload.linked_transaction_id
  end

  test 'does not link a withdrawal with no candidate' do
    withdrawal = create_transaction(:withdrawal, base_amount: 1, transacted_at: @transacted_at)

    patch toggle_transfer_tracker_transaction_path(withdrawal)

    assert_response :success
    assert_nil withdrawal.reload.linked_transaction_id
    assert flash[:alert].present?
  end

  test 'does not guess between multiple candidates' do
    withdrawal = create_transaction(:withdrawal, base_amount: 1, transacted_at: @transacted_at)
    create_transaction(:deposit, base_amount: 0.999, transacted_at: @transacted_at + 1.day)
    create_transaction(:deposit, base_amount: 0.998, transacted_at: @transacted_at + 2.days)

    patch toggle_transfer_tracker_transaction_path(withdrawal)

    assert_response :success
    assert_nil withdrawal.reload.linked_transaction_id
    assert flash[:alert].present?
  end

  test "does not find another user's transaction" do
    show_exceptions = Rails.application.env_config['action_dispatch.show_exceptions']
    other_user = create(:user, setup_completed: true)
    other_api_key = create(:api_key, user: other_user, exchange: @api_key.exchange)
    withdrawal = create(
      :account_transaction, :withdrawal,
      api_key: other_api_key, transacted_at: @transacted_at
    )
    Rails.application.env_config['action_dispatch.show_exceptions'] = :none

    assert_raises(ActiveRecord::RecordNotFound) do
      patch toggle_transfer_tracker_transaction_path(withdrawal)
    end
    assert_nil withdrawal.reload.linked_transaction_id
  ensure
    Rails.application.env_config['action_dispatch.show_exceptions'] = show_exceptions
  end

  test 'links a deposit to its only eligible earlier withdrawal' do
    withdrawal = create_transaction(:withdrawal, base_amount: 1, transacted_at: @transacted_at)
    deposit = create_transaction(:deposit, base_amount: 0.999, transacted_at: @transacted_at + 2.days)

    patch toggle_transfer_tracker_transaction_path(deposit)

    assert_response :success
    assert_equal deposit.id, withdrawal.reload.linked_transaction_id
  end

  test 'does not consider a deposit larger than the withdrawal' do
    withdrawal = create_transaction(:withdrawal, base_amount: 1, transacted_at: @transacted_at)
    create_transaction(:deposit, base_amount: 1.5, transacted_at: @transacted_at + 2.days)

    patch toggle_transfer_tracker_transaction_path(withdrawal)

    assert_response :success
    assert_nil withdrawal.reload.linked_transaction_id
    assert flash[:alert].present?
  end

  # The turbo-stream tests above render the row partial in isolation. This is the only check that
  # the transactions table itself still renders once the badge and the toggle button are in it.
  test 'the transactions table shows the transfer badge and the toggle action' do
    withdrawal, = create_linked_pair

    get tracker_path

    assert_response :success
    assert_no_match(/translation_missing/, @response.body)
    assert_includes @response.body, I18n.t('tracker.transfer_badge')
    assert_includes @response.body, I18n.t('tracker.transfer_unlink')
    assert_includes @response.body, toggle_transfer_tracker_transaction_path(withdrawal)
  end

  private

  def create_transaction(entry_type, **attributes)
    create(:account_transaction, entry_type, api_key: @api_key, **attributes)
  end

  def create_linked_pair
    withdrawal = create_transaction(:withdrawal, base_amount: 1, transacted_at: @transacted_at)
    deposit = create_transaction(:deposit, base_amount: 0.999, transacted_at: @transacted_at + 2.days)
    withdrawal.update!(linked_transaction: deposit)
    [withdrawal, deposit]
  end
end
