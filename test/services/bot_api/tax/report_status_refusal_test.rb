require 'test_helper'

# REST and MCP callers never see the Turbo broadcast; they poll. Without a distinct state a refusal
# is indistinguishable from "never generated", and the caller is told to generate it again — forever.
class BotApi::Tax::ReportStatusRefusalTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    # Reports are real files under a shared tmp root, so a leftover CSV from another test would
    # read as "ready" here.
    FileUtils.rm_f(Tax::GenerateReportJob.report_path(@user.id, 'DE', 2014))
    path = Tax::GenerateReportJob.refusal_path(@user.id, 'DE', 2014)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, JSON.generate({ 'reason' => 'tokenized_unsupported', 'symbols' => ['NVDAX'] }))
  end

  teardown { FileUtils.rm_f(Tax::GenerateReportJob.refusal_path(@user.id, 'DE', 2014)) }

  test 'a refused report reports its reason and symbols, not none' do
    result = BotApi::Tax::ReportStatus.call(user: @user, country: 'DE', year: 2014)

    assert result.success?
    assert_equal 'refused', result.data[:state]
    assert_equal ['NVDAX'], result.data[:symbols]
    assert_equal 'tokenized_unsupported', result.data[:reason]
  end

  test 'downloading a refused report explains instead of inviting a retry' do
    result = BotApi::Tax::DownloadReport.call(user: @user, country: 'DE', year: 2014)

    assert_not result.success?
    assert_equal 'report_refused', result.error_code
    assert_match 'NVDAX', result.error_message
    assert_no_match(/generate_tax_report/, result.error_message)
  end
end
