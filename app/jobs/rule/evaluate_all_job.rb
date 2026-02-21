class Rule::EvaluateAllJob < ApplicationJob
  queue_as :default

  def perform
    Rule.where(status: :scheduled).find_each do |rule|
      Rule::ExecuteJob.perform_later(rule)
    end
  end
end
