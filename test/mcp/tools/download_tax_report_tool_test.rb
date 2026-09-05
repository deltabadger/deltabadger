require 'test_helper'

class DownloadTaxReportToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    stub_mcp_client(@user)
  end

  test 'returns CSV content when report exists' do
    csv_content = "date,asset,amount\n2025-01-01,BTC,0.5"
    path = write_report('PL', 2027, csv_content)

    response = DownloadTaxReportTool.call('country' => 'PL', 'year' => 2027)
    text = response.contents.first.text

    assert_equal csv_content, text
  ensure
    FileUtils.rm_f(path)
  end

  test 'does not delete file after reading' do
    path = write_report('PL', 2028, 'test')

    DownloadTaxReportTool.call('country' => 'PL', 'year' => 2028)

    assert File.exist?(path), 'Expected file to still exist after MCP download'
  ensure
    FileUtils.rm_f(path)
  end

  test 'returns error when no report found' do
    # Use a unique combo to avoid collisions with file-creating tests
    path = report_path('US', 2029)
    FileUtils.rm_f(path)

    response = DownloadTaxReportTool.call('country' => 'US', 'year' => 2029)
    text = response.contents.first.text

    assert_match(/No report found/, text)
    assert_match(/generate_tax_report/, text)
  end

  private

  def report_path(country, year)
    Tax::GenerateReportJob.report_path(@user.id, country, year)
  end

  def write_report(country, year, content)
    path = report_path(country, year)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, content)
    path
  end
end
