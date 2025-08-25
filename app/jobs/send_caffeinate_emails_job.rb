class SendCaffeinateEmailsJob < ApplicationJob
  queue_as :low_priority

  def perform
    Caffeinate.perform!
  end
end
