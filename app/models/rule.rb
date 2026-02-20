class Rule < ApplicationRecord
  include Automation::Statusable
  include Automation::Configurable
  include Automation::Executable
  include Automation::ExchangeConnectable

  belongs_to :user
  belongs_to :asset, optional: true
  has_many :rule_logs, dependent: :destroy

  scope :active, -> { where(status: :scheduled) }

  private

  def log_success(message, details: {})
    rule_logs.create!(status: :success, message:, details:)
  end

  def log_failed(message, details: {})
    rule_logs.create!(status: :failed, message:, details:)
  end

  def log_skipped(message, details: {})
    rule_logs.create!(status: :pending, message:, details:)
  end
end
