class Rule < ApplicationRecord
  belongs_to :user
  belongs_to :exchange, optional: true
  belongs_to :asset, optional: true
  has_many :rule_logs, dependent: :destroy

  enum :status, %i[inactive active]

  scope :active, -> { where(status: :active) }
end
