require 'test_helper'

class GetTaxReportStatusToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    ActionMCP::Current.stubs(:user).returns(@user)
  end

  test 'returns ready when report file exists' do
    file_path = Rails.root.join('tmp', 'tax_reports', "#{@user.id}_DE_2025.csv")
    FileUtils.mkdir_p(File.dirname(file_path))
    File.write(file_path, 'test')

    response = GetTaxReportStatusTool.call('country' => 'DE', 'year' => 2025)
    text = response.contents.first.text

    assert_match(/ready/, text)
    assert_match(/download_tax_report/, text)
  ensure
    FileUtils.rm_f(file_path)
  end

  test 'returns not ready when no report file' do
    response = GetTaxReportStatusTool.call('country' => 'DE', 'year' => 2025)
    text = response.contents.first.text

    assert_match(/not ready/, text)
    assert_match(/generate_tax_report/, text)
  end
end
