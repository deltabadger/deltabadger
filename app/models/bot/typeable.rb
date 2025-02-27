module Bot::Typeable
  extend ActiveSupport::Concern

  # We use this concern to get enum-like functionality for the type column
  # We can't use enum because we are using STI.

  included do
    scope :trading, -> { where(type: 'DcaBot') }
    scope :withdrawal, -> { where(type: 'WithdrawalBot') }
    scope :webhook, -> { where(type: 'WebhookBot') }
    scope :barbell, -> { where(type: 'BarbellBot') }
  end

  def trading?
    type == 'DcaBot'
  end

  def withdrawal?
    type == 'WithdrawalBot'
  end

  def webhook?
    type == 'WebhookBot'
  end

  def barbell?
    type == 'BarbellBot'
  end
end
