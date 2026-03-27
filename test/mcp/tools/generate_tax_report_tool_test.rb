require 'test_helper'

class GenerateTaxReportToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    ActionMCP::Current.stubs(:user).returns(@user)
    MarketData.stubs(:configured?).returns(true)
  end

  test 'enqueues tax report generation job' do
    cleanup_report('DE', 2025)
    Tax::GenerateReportJob.expects(:perform_later).with(@user.id, 'DE', 2025, false)

    GenerateTaxReportTool.call('country' => 'DE', 'year' => 2025)
  end

  test 'returns started message' do
    cleanup_report('DE', 2025)
    Tax::GenerateReportJob.stubs(:perform_later)

    response = GenerateTaxReportTool.call('country' => 'DE', 'year' => 2025)
    text = response.contents.first.text

    assert_match(/Tax report generation started for Germany \(2025\)/, text)
    assert_match(/get_tax_report_status/, text)
  end

  test 'passes stablecoin_as_fiat flag' do
    cleanup_report('AT', 2025)
    Tax::GenerateReportJob.expects(:perform_later).with(@user.id, 'AT', 2025, true)

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
    Tax::GenerateReportJob.expects(:perform_later).with(@user.id, 'NL', 2025, false)

    GenerateTaxReportTool.call('country' => 'NL', 'year' => 2025)
  end

  test 'detects existing report file' do
    # Use a unique country/year combo so other parallel processes cannot interfere
    path = report_path('IT', 1999)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, 'test')
    Tax::GenerateReportJob.expects(:perform_later).never

    response = GenerateTaxReportTool.call('country' => 'IT', 'year' => 1999)
    text = response.contents.first.text

    assert_match(/already available/, text)
  ensure
    FileUtils.rm_f(path)
  end

  test 'updates tracker_settings' do
    cleanup_report('DE', 2025)
    Tax::GenerateReportJob.stubs(:perform_later)

    GenerateTaxReportTool.call('country' => 'DE', 'year' => 2025)
    @user.reload

    assert_equal 'tax_report', @user.tracker_settings['export_type']
    assert_equal 'DE', @user.tracker_settings['country']
    assert_equal 2025, @user.tracker_settings['year']
  end

  private

  def report_path(country, year)
    Rails.root.join('tmp', 'tax_reports', "#{@user.id}_#{country}_#{year}.csv")
  end

  def cleanup_report(country, year)
    FileUtils.rm_f(report_path(country, year))
  end
end
