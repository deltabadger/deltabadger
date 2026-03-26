class Rule::EvaluateAllJob < ApplicationJob
  queue_as :default
  limits_concurrency to: 1, key: -> { 'evaluate_all_rules' }, on_conflict: :discard

  def perform
    Rule.where(status: :scheduled).find_each do |rule|
      Rule::ExecuteJob.perform_later(rule)
    end
  end
end
