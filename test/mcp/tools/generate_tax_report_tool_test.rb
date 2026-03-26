require 'test_helper'

class GenerateTaxReportToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    ActionMCP::Current.stubs(:user).returns(@user)
    MarketData.stubs(:configured?).returns(true)
  end

  test 'enqueues tax report generation job' do
    Tax::GenerateReportJob.expects(:perform_later).with(@user.id, 'DE', 2025, false)

    GenerateTaxReportTool.call('country' => 'DE', 'year' => 2025)
  end

  test 'returns started message' do
    Tax::GenerateReportJob.stubs(:perform_later)

    response = GenerateTaxReportTool.call('country' => 'DE', 'year' => 2025)
    text = response.contents.first.text

    assert_match(/Tax report generation started for Germany \(2025\)/, text)
    assert_match(/get_tax_report_status/, text)
  end

  test 'passes stablecoin_as_fiat flag' do
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
    MarketData.stubs(:configured?).returns(false)
    Tax::GenerateReportJob.expects(:perform_later).never

    response = GenerateTaxReportTool.call('country' => 'DE', 'year' => 2025)
    text = response.contents.first.text

    assert_match(/Market data provider is not configured/, text)
  end

  test 'allows wealth snapshot without market data configured' do
    MarketData.stubs(:configured?).returns(false)
    Tax::GenerateReportJob.expects(:perform_later).with(@user.id, 'NL', 2025, false)

    GenerateTaxReportTool.call('country' => 'NL', 'year' => 2025)
  end

  test 'detects existing report file' do
    file_path = Rails.root.join('tmp', 'tax_reports', "#{@user.id}_DE_2025.csv")
    FileUtils.mkdir_p(File.dirname(file_path))
    File.write(file_path, 'test')
    Tax::GenerateReportJob.expects(:perform_later).never

    response = GenerateTaxReportTool.call('country' => 'DE', 'year' => 2025)
    text = response.contents.first.text

    assert_match(/already available/, text)
  ensure
    FileUtils.rm_f(file_path)
  end

  test 'updates tracker_settings' do
    Tax::GenerateReportJob.stubs(:perform_later)

    GenerateTaxReportTool.call('country' => 'DE', 'year' => 2025)
    @user.reload

    assert_equal 'tax_report', @user.tracker_settings['export_type']
    assert_equal 'DE', @user.tracker_settings['country']
    assert_equal 2025, @user.tracker_settings['year']
  end
end
