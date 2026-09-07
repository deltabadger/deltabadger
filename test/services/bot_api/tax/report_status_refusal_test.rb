require 'test_helper'

# REST and MCP callers never see the Turbo broadcast; they poll. Without a distinct state a refusal
# is indistinguishable from "never generated", and the caller is told to generate it again — forever.
#
# Each test owns a year: reports are real files under a shared tmp root keyed on user id, which
# repeats across parallel workers, so sharing one would let a sibling's teardown delete it mid-run.
class BotApi::Tax::ReportStatusRefusalTest < ActiveSupport::TestCase
  def refuse(year)
    user = create(:user)
    FileUtils.rm_f(Tax::GenerateReportJob.report_path(user.id, 'DE', year))
    path = Tax::GenerateReportJob.refusal_path(user.id, 'DE', year)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, JSON.generate({ 'reason' => 'tokenized_unsupported', 'symbols' => ['NVDAX'] }))
    @cleanup = [user.id, year]
    user
  end

  def teardown
    return unless defined?(@cleanup) && @cleanup

    user_id, year = @cleanup
    FileUtils.rm_f(Tax::GenerateReportJob.refusal_path(user_id, 'DE', year))
    FileUtils.rm_f(Tax::GenerateReportJob.report_path(user_id, 'DE', year))
  end

  test 'a refused report reports its reason and symbols, not none' do
    user = refuse(2012)

    result = BotApi::Tax::ReportStatus.call(user: user, country: 'DE', year: 2012)

    assert result.success?
    assert_equal 'refused', result.data[:state]
    assert_equal ['NVDAX'], result.data[:symbols]
    assert_equal 'tokenized_unsupported', result.data[:reason]
  end

  test 'downloading a refused report explains instead of inviting a retry' do
    user = refuse(2013)

    result = BotApi::Tax::DownloadReport.call(user: user, country: 'DE', year: 2013)

    assert_not result.success?
    assert_equal 'report_refused', result.error_code
    assert_match 'NVDAX', result.error_message
    assert_no_match(/generate_tax_report/, result.error_message)
  end
end
