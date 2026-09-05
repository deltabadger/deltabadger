require 'test_helper'

class GenerateTaxReportToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    stub_mcp_client(@user)
    MarketData.stubs(:configured?).returns(true)
  end

  test 'enqueues tax report generation job' do
    cleanup_report('DE', 2025)
    Tax::GenerateReportJob.expects(:perform_later).with(@user.id, 'DE', 2025, false).returns(accepted)

    GenerateTaxReportTool.call('country' => 'DE', 'year' => 2025)
  end

  test 'returns started message' do
    cleanup_report('DE', 2025)
    Tax::GenerateReportJob.stubs(:perform_later).returns(accepted)

    response = GenerateTaxReportTool.call('country' => 'DE', 'year' => 2025)
    text = response.contents.first.text

    assert_match(/Tax report generation started for Germany \(2025\)/, text)
    assert_match(/get_tax_report_status/, text)
  end

  test 'passes stablecoin_as_fiat flag' do
    cleanup_report('AT', 2025)
    Tax::GenerateReportJob.expects(:perform_later).with(@user.id, 'AT', 2025, true).returns(accepted)

    GenerateTaxReportTool.call('country' => 'AT', 'year' => 2025, 'stablecoin_as_fiat' => true)
  end

  test 'rejects unknown country code' do
    Tax::GenerateReportJob.expects(:perform_later).never

    response = GenerateTaxReportTool.call('country' => 'XX', 'year' => 2025)
    text = response.contents.first.text

    assert_match(/Unknown country code/, text)
  end

  test 'rejects when market data not configured' do
    cleanup_report('DE', 2025)
    MarketData.stubs(:configured?).returns(false)
    Tax::GenerateReportJob.expects(:perform_later).never

    response = GenerateTaxReportTool.call('country' => 'DE', 'year' => 2025)
    text = response.contents.first.text

    assert_match(/Market data provider is not configured/, text)
  end

  test 'allows wealth snapshot without market data configured' do
    cleanup_report('NL', 2025)
    MarketData.stubs(:configured?).returns(false)
    Tax::GenerateReportJob.expects(:perform_later).with(@user.id, 'NL', 2025, false).returns(accepted)

    GenerateTaxReportTool.call('country' => 'NL', 'year' => 2025)
  end

  test 'detects existing report file' do
    # Use a unique country/year combo so other parallel processes cannot interfere
    path = report_path('IT', 2030)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, 'test')
    Tax::GenerateReportJob.expects(:perform_later).never

    response = GenerateTaxReportTool.call('country' => 'IT', 'year' => 2030)
    text = response.contents.first.text

    assert_match(/already available/, text)
  ensure
    FileUtils.rm_f(path)
  end

  test 'records the pending report so the tracker auto-downloads it' do
    cleanup_report('DE', 2025)
    Tax::GenerateReportJob.stubs(:perform_later).returns(accepted)

    GenerateTaxReportTool.call('country' => 'DE', 'year' => 2025)
    @user.reload

    assert_equal({ 'country' => 'DE', 'year' => 2025, 'report_scope' => 'crypto' },
                 @user.tracker_settings['pending_report'])
    # The export form's own preferences are the user's, not an MCP call's, to set.
    assert_nil @user.tracker_settings['country']
  end

  test 'force replaces a report that is already on disk' do
    path = report_path('IT', 2036)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, 'stale')
    Tax::GenerateReportJob.expects(:perform_later).with(@user.id, 'IT', 2036, false).returns(accepted)

    response = GenerateTaxReportTool.call('country' => 'IT', 'year' => 2036, 'force' => true)

    assert_match(/generation started/, response.contents.first.text)
  ensure
    FileUtils.rm_f(path)
  end

  private

  # The service asks the queue whether the job survived its concurrency control.
  def accepted = stub(successfully_enqueued?: true, provider_job_id: nil)

  def report_path(country, year)
    Tax::GenerateReportJob.report_path(@user.id, country, year)
  end

  def cleanup_report(country, year)
    FileUtils.rm_f(report_path(country, year))
  end
end
