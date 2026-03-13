class Rule < ApplicationRecord
  include Automation::Statusable
  include Automation::Configurable
  include Automation::Executable
  include Automation::ExchangeConnectable

  belongs_to :user
  belongs_to :asset, optional: true
  has_many :rule_logs, dependent: :destroy

  scope :active, -> { where(status: :scheduled) }

  def broadcast_tile_update
    broadcast_replace_to(
      ["user_#{user_id}", :rule_updates],
      target: ActionView::RecordIdentifier.dom_id(self),
      partial: 'rules/rule_tile',
      locals: { rule: self }
    )
  end

  private

  def broadcast_replace_to(...)
    with_user_locale { super }
  end

  def with_user_locale(&block)
    I18n.with_locale(user.locale || I18n.default_locale, &block)
  end

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
