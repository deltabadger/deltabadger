require 'test_helper'

class TrackerControllerTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    create(:user, admin: true, setup_completed: true) # platform requires an admin to exist
    @user = create(:user, setup_completed: true)
    @api_key = create(:api_key, user: @user)
    @transacted_at = Time.zone.parse('2026-08-01 12:00:00')
    @report_paths = []
    self.default_url_options = { locale: nil }
    sign_in @user
  end

  teardown do
    @report_paths.each { |path| FileUtils.rm_f(path) }
  end

  test 'links a withdrawal to its only eligible deposit' do
    withdrawal = create_transaction(:withdrawal, base_amount: 1, transacted_at: @transacted_at)
    deposit = create_transaction(:deposit, base_amount: 0.999, transacted_at: @transacted_at + 2.days)

    patch toggle_transfer_tracker_transaction_path(withdrawal)

    assert_response :success
    assert_equal deposit.id, withdrawal.reload.linked_transaction_id
    # A wrong dom_id target is a silent no-op in the browser, so pin both rows of the pair.
    assert_match(/target="account_transaction_#{withdrawal.id}"/, @response.body)
    assert_match(/target="account_transaction_#{deposit.id}"/, @response.body)
  end

  test 'unlinks a linked pair from the withdrawal' do
    withdrawal, = create_linked_pair

    patch toggle_transfer_tracker_transaction_path(withdrawal)

    assert_response :success
    assert_nil withdrawal.reload.linked_transaction_id
  end

  test 'unlinking sticks: the matcher must not re-link it on the next sync' do
    withdrawal, = create_linked_pair

    patch toggle_transfer_tracker_transaction_path(withdrawal)
    TransferMatcher.run!(@user)

    assert_nil withdrawal.reload.linked_transaction_id
    assert_predicate withdrawal, :transfer_link_rejected?
  end

  test 'linking again clears the rejection so the matcher can maintain the pair' do
    withdrawal = create_transaction(:withdrawal, base_amount: 1, transacted_at: @transacted_at)
    deposit = create_transaction(:deposit, base_amount: 0.999, transacted_at: @transacted_at + 2.days)
    withdrawal.update!(transfer_link_rejected: true)

    patch toggle_transfer_tracker_transaction_path(withdrawal)

    assert_equal deposit.id, withdrawal.reload.linked_transaction_id
    assert_not_predicate withdrawal, :transfer_link_rejected?
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

  # tmp/tax_reports is shared by every parallel worker, user ids repeat across their databases, and the
  # runner splits tests rather than files — so each of these fixtures needs a year of its own.
  test 'downloads the broker report with the broker filename' do
    write_report(country: 'DE', year: 1991, report_scope: 'broker', contents: 'broker report')

    get download_tax_report_tracker_path(country: 'DE', year: 1991, report_scope: 'broker')

    assert_response :success
    assert_equal 'broker report', response.body
    assert_match(/filename="deltabadger-broker-tax-report-de-1991\.csv"/,
                 response.headers['Content-Disposition'])
  end

  test 'downloads the default crypto report with the existing filename' do
    write_report(country: 'DE', year: 1992, contents: 'crypto report')

    get download_tax_report_tracker_path(country: 'DE', year: 1992)

    assert_response :success
    assert_equal 'crypto report', response.body
    assert_match(/filename="deltabadger-tax-report-de-1992\.csv"/,
                 response.headers['Content-Disposition'])
  end

  test 'index auto-downloads a pending broker report with its scope' do
    @user.update!(tracker_settings: {
                    'export_type' => 'tax_report', 'country' => 'DE', 'year' => 1993, 'report_scope' => 'broker'
                  })
    write_report(country: 'DE', year: 1993, report_scope: 'broker', contents: 'broker report')

    get tracker_path

    assert_response :success
    assert_includes response.body, 'report_scope=broker'
  end

  test 'tax report persists and enqueues the broker scope' do
    base_adapter = ActiveJob::Base.queue_adapter
    job_adapter = Tax::GenerateReportJob.queue_adapter
    test_adapter = ActiveJob::QueueAdapters::TestAdapter.new
    ActiveJob::Base.queue_adapter = test_adapter
    Tax::GenerateReportJob.queue_adapter = test_adapter

    assert_enqueued_with(
      job: Tax::GenerateReportJob,
      args: [@user.id, 'DE', 2024, false, 'broker']
    ) do
      get tax_report_tracker_path(country: 'DE', year: 2024, report_scope: 'broker')
    end

    assert_response :success
    assert_equal 'broker', @user.reload.tracker_settings['report_scope']
  ensure
    Tax::GenerateReportJob.queue_adapter = job_adapter
    ActiveJob::Base.queue_adapter = base_adapter
  end

  private

  def write_report(country:, year:, contents:, report_scope: 'crypto')
    path = Tax::GenerateReportJob.report_path(@user.id, country, year, report_scope)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, contents)
    @report_paths << path
    path
  end

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
