class Rule::ExecuteJob < ApplicationJob
  limits_concurrency to: 1, key: ->(rule, *) { "exchange_#{rule.exchange&.name_id}" }

  def queue_name
    rule = arguments.first
    rule.exchange&.name_id&.to_sym || :default
  end

  def perform(rule)
    return unless rule.scheduled?

    rule.execute
  rescue StandardError => e
    Rails.logger.error("Rule::ExecuteJob for rule #{rule.id} failed: #{e.message}")
    rule.rule_logs.create!(status: :failed, message: "Unexpected error: #{e.message}")
  end
end
