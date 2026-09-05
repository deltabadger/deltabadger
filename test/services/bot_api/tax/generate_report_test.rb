# frozen_string_literal: true

require 'test_helper'

# The suite runs in parallel workers that share tmp/tax_reports, so every test here owns a
# country/year pair no other test in the suite uses (the MCP tool tests take DE/AT/NL/IT).
#
# The service asks the queue whether the job survived, so every stubbed or expected
# perform_later in this file — and in the MCP and REST tests — returns a job that answers
# both `successfully_enqueued?` and `provider_job_id`.
class BotApi::Tax::GenerateReportTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    MarketData.stubs(:configured?).returns(true)
  end

  def path(country, year) = Tax::GenerateReportJob.report_path(@user.id, country, year)

  def enqueued = stub(successfully_enqueued?: true, provider_job_id: nil)

  test 'enqueues the job and records the pending report' do
    Tax::GenerateReportJob.expects(:perform_later).with(@user.id, 'ES', 2031, false).returns(enqueued)

    result = BotApi::Tax::GenerateReport.call(user: @user, country: 'ES', year: 2031)

    assert result.success?
    assert_equal :accepted, result.status
    assert_equal({ 'country' => 'ES', 'year' => 2031, 'report_scope' => 'crypto' },
                 @user.reload.tracker_settings['pending_report'])
  end

  test 'country is upcased before the registry lookup, and REST string booleans are cast' do
    Tax::GenerateReportJob.expects(:perform_later).with(@user.id, 'PT', 2032, false).returns(enqueued)
    result = BotApi::Tax::GenerateReport.call(user: @user, country: 'pt', year: '2032', stablecoin_as_fiat: 'false')
    assert result.success?, result.error_message
    assert_equal 'PT', result.data[:country]
  end

  test 'unknown country' do
    result = BotApi::Tax::GenerateReport.call(user: @user, country: 'XX', year: 2033)
    assert_equal 'unknown_country', result.error_code
    assert_equal :validation_failed, result.status
  end

  test 'missing parameters' do
    result = BotApi::Tax::GenerateReport.call(user: @user, country: nil, year: nil)
    assert_equal 'missing_required_parameter', result.error_code
  end

  test 'malformed inputs are refusals, never casts' do
    Tax::GenerateReportJob.expects(:perform_later).never
    assert_equal 'invalid_year', BotApi::Tax::GenerateReport.call(user: @user, country: 'ES', year: '20x5').error_code
    assert_equal 'invalid_year', BotApi::Tax::GenerateReport.call(user: @user, country: 'ES', year: '1999').error_code
    assert_equal 'invalid_flag', BotApi::Tax::GenerateReport.call(user: @user, country: 'ES', year: 2033, force: 'maybe').error_code
    assert_equal 'invalid_flag',
                 BotApi::Tax::GenerateReport.call(user: @user, country: 'ES', year: 2033, stablecoin_as_fiat: 'yes').error_code
  end

  # The job discards a second enqueue under the same per-user key, so accepting one would lose it.
  test 'a report already queued or running for this account is a conflict, and force does not delete anything' do
    key = Tax::GenerateReportJob.new(@user.id, 'XX', 0).concurrency_key
    SolidQueue::Job.create!(queue_name: 'low_priority', class_name: 'Tax::GenerateReportJob',
                            concurrency_key: key, arguments: { 'arguments' => [] })
    FileUtils.mkdir_p(File.dirname(path('ES', 2036)))
    File.write(path('ES', 2036), 'csv')
    Tax::GenerateReportJob.expects(:perform_later).never

    result = BotApi::Tax::GenerateReport.call(user: @user, country: 'ES', year: 2036, force: true)

    assert_equal 'report_generating', result.error_code
    assert_equal :conflict, result.status
    assert File.exist?(path('ES', 2036))
    assert_equal 'generating', BotApi::Tax::ReportStatus.call(user: @user, country: 'ES', year: 2037).data[:state]
  ensure
    FileUtils.rm_f(path('ES', 2036))
    SolidQueue::Job.where(class_name: 'Tax::GenerateReportJob').delete_all
  end

  test 'a failed earlier run does not pin the account on generating' do
    key = Tax::GenerateReportJob.new(@user.id, 'XX', 0).concurrency_key
    job = SolidQueue::Job.create!(queue_name: 'low_priority', class_name: 'Tax::GenerateReportJob',
                                  concurrency_key: key, arguments: { 'arguments' => [] })
    SolidQueue::FailedExecution.create!(job_id: job.id, error: { 'message' => 'boom' })
    Tax::GenerateReportJob.expects(:perform_later).with(@user.id, 'ES', 2039, false).returns(enqueued)

    assert BotApi::Tax::GenerateReport.call(user: @user, country: 'ES', year: 2039).success?
  ensure
    SolidQueue::FailedExecution.delete_all
    SolidQueue::Job.where(class_name: 'Tax::GenerateReportJob').delete_all
  end

  # The queue's discard is the last line of defence; when it fires, the answer is a 409 and the
  # previous report is still there. ActiveJob marks every job successfully_enqueued? once the
  # adapter returns, so the row's survival — not the flag — is what says the job was taken.
  test 'an enqueue the queue discarded is a conflict and deletes nothing' do
    FileUtils.mkdir_p(File.dirname(path('ES', 2040)))
    File.write(path('ES', 2040), 'csv')
    Tax::GenerateReportJob.stubs(:perform_later).returns(stub(successfully_enqueued?: true, provider_job_id: -1))

    result = BotApi::Tax::GenerateReport.call(user: @user, country: 'ES', year: 2040, force: true)

    assert_equal 'report_generating', result.error_code
    assert File.exist?(path('ES', 2040)), 'the previous report is restored'
    assert_not File.exist?("#{path('ES', 2040)}.stale")
    assert_nil @user.reload.tracker_settings&.dig('pending_report')
  ensure
    FileUtils.rm_f(path('ES', 2040))
    FileUtils.rm_f("#{path('ES', 2040)}.stale")
  end

  test 'an enqueue ActiveJob refused outright is a conflict too' do
    Tax::GenerateReportJob.stubs(:perform_later).returns(false)
    assert_equal 'report_generating', BotApi::Tax::GenerateReport.call(user: @user, country: 'ES', year: 2041).error_code
  end

  test 'status states' do
    assert_equal 'none', BotApi::Tax::ReportStatus.call(user: @user, country: 'ES', year: 2038).data[:state]
    FileUtils.mkdir_p(File.dirname(path('ES', 2038)))
    File.write(path('ES', 2038), 'csv')
    status = BotApi::Tax::ReportStatus.call(user: @user, country: 'es', year: '2038').data
    assert_equal 'ready', status[:state]
    assert status[:ready]
    assert_equal 'unknown_country', BotApi::Tax::ReportStatus.call(user: @user, country: 'XX', year: 2038).error_code
  ensure
    FileUtils.rm_f(path('ES', 2038))
  end

  test 'market data required unless the method is a wealth snapshot' do
    MarketData.stubs(:configured?).returns(false)
    assert_equal 'market_data_not_configured',
                 BotApi::Tax::GenerateReport.call(user: @user, country: 'GR', year: 2034).error_code
    Tax::GenerateReportJob.stubs(:perform_later).returns(enqueued)
    assert BotApi::Tax::GenerateReport.call(user: @user, country: 'CH', year: 2034).success?
  end

  test 'a report already on disk is a conflict unless forced, and force discards it' do
    FileUtils.mkdir_p(File.dirname(path('IE', 2035)))
    File.write(path('IE', 2035), 'csv')

    Tax::GenerateReportJob.expects(:perform_later).never
    result = BotApi::Tax::GenerateReport.call(user: @user, country: 'IE', year: 2035)
    assert_equal 'report_ready', result.error_code
    assert_equal :conflict, result.status

    Tax::GenerateReportJob.expects(:perform_later).with(@user.id, 'IE', 2035, false).returns(enqueued)
    result = BotApi::Tax::GenerateReport.call(user: @user, country: 'IE', year: 2035, force: true)
    assert result.success?, result.error_message
    assert_not File.exist?(path('IE', 2035)), 'a forced regeneration must not leave the stale report for status to find'
  ensure
    FileUtils.rm_f(path('IE', 2035))
  end

  test 'download reads the report and leaves it in place' do
    FileUtils.mkdir_p(File.dirname(path('IE', 2042)))
    File.write(path('IE', 2042), "a,b\n1,2\n")

    result = BotApi::Tax::DownloadReport.call(user: @user, country: 'ie', year: '2042')

    assert result.success?, result.error_message
    assert_equal "a,b\n1,2\n", result.data[:csv]
    assert_equal 'deltabadger-tax-report-ie-2042.csv', result.data[:filename]
    assert File.exist?(path('IE', 2042))
    FileUtils.rm_f(path('IE', 2042))
    assert_equal 'report_not_found', BotApi::Tax::DownloadReport.call(user: @user, country: 'IE', year: 2042).error_code
  ensure
    FileUtils.rm_f(path('IE', 2042))
  end

  test 'jurisdictions are projected, not dumped' do
    data = BotApi::Tax::ListJurisdictions.call.data

    assert_equal ::Tax::Jurisdictions.available.size, data[:count]
    germany = data[:jurisdictions].find { |row| row[:code] == 'DE' }
    assert_equal({ code: 'DE', name: 'Germany', method: 'fifo', currency: 'EUR' }, germany)
  end
end
