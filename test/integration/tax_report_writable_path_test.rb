require 'test_helper'
require 'tmpdir'

class TaxReportWritablePathTest < ActionDispatch::IntegrationTest
  COUNTRY = 'DE'.freeze
  # Inside the documented 2009..2100 range the API accepts; the report lives in its own tmpdir.
  YEAR = 2044
  CSV_DATA = "date,asset,amount\n2044-01-01,BTC,0.5".freeze

  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
  end

  test 'job output in APP_TMP_DIR is found by every tax report reader' do
    Dir.mktmpdir do |tmp_dir|
      with_env('APP_TMP_DIR', tmp_dir) do
        generate_report
        report_path = Pathname(tmp_dir).join('tax_reports', "#{@user.id}_#{COUNTRY}_#{YEAR}_crypto.csv")

        assert_equal CSV_DATA, report_path.read
        assert_mcp_readers_find_report
        assert_tracker_finds_pending_report
        assert_tracker_downloads_report(report_path)
      end
    end
  ensure
    FileUtils.rm_f(Rails.root.join('tmp', 'tax_reports', "#{@user.id}_#{COUNTRY}_#{YEAR}_crypto.csv"))
  end

  private

  def generate_report
    Tax::Report.any_instance.stubs(:to_csv).returns(CSV_DATA)
    Tax::GenerateReportJob.any_instance.stubs(:sleep)
    Turbo::StreamsChannel.stubs(:broadcast_replace_to)

    Tax::GenerateReportJob.perform_now(@user.id, COUNTRY, YEAR)
  end

  def assert_mcp_readers_find_report
    stub_mcp_client(@user)
    MarketData.stubs(:configured?).returns(true)
    Tax::GenerateReportJob.expects(:perform_later).never

    generate_response = GenerateTaxReportTool.call('country' => COUNTRY, 'year' => YEAR)
    assert_match(/already available/, generate_response.contents.first.text)

    status_response = GetTaxReportStatusTool.call('country' => COUNTRY, 'year' => YEAR)
    assert_match(/is ready/, status_response.contents.first.text)

    download_response = DownloadTaxReportTool.call('country' => COUNTRY, 'year' => YEAR)
    assert_equal CSV_DATA, download_response.contents.first.text
  end

  def assert_tracker_finds_pending_report
    settings = { 'pending_report' => { 'country' => COUNTRY, 'year' => YEAR, 'report_scope' => 'crypto' } }
    @user.update!(tracker_settings: settings)

    get tracker_path

    assert_response :success
    expected_path = download_tax_report_tracker_path(country: COUNTRY, year: YEAR, report_scope: 'crypto')
    assert_select '[data-controller="auto-download"]' do |elements|
      assert(elements.any? { |element| element['data-auto-download-url-value'] == expected_path })
    end
  end

  def assert_tracker_downloads_report(report_path)
    get download_tax_report_tracker_path(country: COUNTRY, year: YEAR)

    assert_response :success
    assert_equal CSV_DATA, response.body
    assert_not report_path.exist?
  end

  def with_env(key, value)
    original = ENV[key]
    ENV[key] = value
    yield
  ensure
    original.nil? ? ENV.delete(key) : ENV[key] = original
  end
end
