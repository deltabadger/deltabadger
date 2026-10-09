require 'test_helper'

class GetTaxReportStatusToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    stub_mcp_client(@user)
  end

  test 'returns ready when report file exists' do
    path = write_report('GB', 2027)

    response = GetTaxReportStatusTool.call('country' => 'GB', 'year' => 2027)
    text = response.contents.first.text

    assert_match(/ready/, text)
    assert_match(/download_tax_report/, text)
  ensure
    FileUtils.rm_f(path)
  end

  test 'returns not ready when no report file' do
    path = report_path('GB', 2028)
    FileUtils.rm_f(path)

    response = GetTaxReportStatusTool.call('country' => 'GB', 'year' => 2028)
    text = response.contents.first.text

    assert_match(/not ready/, text)
    assert_match(/generate_tax_report/, text)
  end

  test 'a refusal is explained by its own reason' do
    path = Tax::GenerateReportJob.refusal_path(@user.id, 'DE', 2029)
    FileUtils.rm_f(report_path('DE', 2029))
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, JSON.generate({ 'reason' => 'excess_return_of_capital', 'symbols' => ['BTC'] }))

    text = GetTaxReportStatusTool.call('country' => 'DE', 'year' => 2029).contents.first.text

    assert_match(/cannot be generated/, text)
    assert_match(/return of capital on BTC exceeded its cost basis/, text)
    assert_no_match(/tokenized/, text)
  ensure
    FileUtils.rm_f(path)
  end

  test 'a tokenized refusal names the tokenized securities' do
    path = Tax::GenerateReportJob.refusal_path(@user.id, 'DE', 2030)
    FileUtils.rm_f(report_path('DE', 2030))
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, JSON.generate({ 'reason' => 'tokenized_unsupported', 'symbols' => ['NVDAX'] }))

    text = GetTaxReportStatusTool.call('country' => 'DE', 'year' => 2030).contents.first.text

    assert_match(/NVDAX are tokenized securities/, text)
  ensure
    FileUtils.rm_f(path)
  end

  private

  def report_path(country, year)
    Tax::GenerateReportJob.report_path(@user.id, country, year)
  end

  def write_report(country, year)
    path = report_path(country, year)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, 'test')
    path
  end
end
