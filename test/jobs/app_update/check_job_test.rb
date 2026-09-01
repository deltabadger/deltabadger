require 'test_helper'

class AppUpdate::CheckJobTest < ActiveJob::TestCase
  test 'refreshes when the check is enabled' do
    AppUpdate.stubs(:enabled?).returns(true)
    AppUpdate.expects(:refresh!).once

    AppUpdate::CheckJob.perform_now
  end

  test 'does nothing when the check is disabled' do
    AppUpdate.stubs(:enabled?).returns(false)
    AppUpdate.expects(:refresh!).never

    AppUpdate::CheckJob.perform_now
  end
end
