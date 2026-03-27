require 'test_helper'

class GetTaxReportStatusToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    ActionMCP::Current.stubs(:user).returns(@user)
  end

  test 'returns ready when report file exists' do
    path = write_report('RU', 1995)

    response = GetTaxReportStatusTool.call('country' => 'RU', 'year' => 1995)
    text = response.contents.first.text

    assert_match(/ready/, text)
    assert_match(/download_tax_report/, text)
  ensure
    FileUtils.rm_f(path)
  end

  test 'returns not ready when no report file' do
    path = report_path('RU', 1994)
    FileUtils.rm_f(path)

    response = GetTaxReportStatusTool.call('country' => 'RU', 'year' => 1994)
    text = response.contents.first.text

    assert_match(/not ready/, text)
    assert_match(/generate_tax_report/, text)
  end

  private

  def report_path(country, year)
    Rails.root.join('tmp', 'tax_reports', "#{@user.id}_#{country}_#{year}.csv")
  end

  def write_report(country, year)
    path = report_path(country, year)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, 'test')
    path
  end
end
