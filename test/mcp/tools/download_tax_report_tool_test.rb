require 'test_helper'

class DownloadTaxReportToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    ActionMCP::Current.stubs(:user).returns(@user)
  end

  test 'returns CSV content when report exists' do
    csv_content = "date,asset,amount\n2025-01-01,BTC,0.5"
    file_path = Rails.root.join('tmp', 'tax_reports', "#{@user.id}_DE_2025.csv")
    FileUtils.mkdir_p(File.dirname(file_path))
    File.write(file_path, csv_content)

    response = DownloadTaxReportTool.call('country' => 'DE', 'year' => 2025)
    text = response.contents.first.text

    assert_equal csv_content, text
  ensure
    FileUtils.rm_f(file_path)
  end

  test 'does not delete file after reading' do
    file_path = Rails.root.join('tmp', 'tax_reports', "#{@user.id}_DE_2025.csv")
    FileUtils.mkdir_p(File.dirname(file_path))
    File.write(file_path, 'test')

    DownloadTaxReportTool.call('country' => 'DE', 'year' => 2025)

    assert File.exist?(file_path), 'Expected file to still exist after MCP download'
  ensure
    FileUtils.rm_f(file_path)
  end

  test 'returns error when no report found' do
    response = DownloadTaxReportTool.call('country' => 'DE', 'year' => 2025)
    text = response.contents.first.text

    assert_match(/No report found/, text)
    assert_match(/generate_tax_report/, text)
  end
end
