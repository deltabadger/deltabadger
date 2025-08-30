class SyncAwsSesOptOutsJob < ApplicationJob
  queue_as :low_priority

  def perform
    aws_ses = AwsSes.new
    aws_ses.sync_opt_outs
  end
end
