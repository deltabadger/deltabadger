module Bot::Typeable
  extend ActiveSupport::Concern

  # We use this concern to get enum-like functionality for the type column
  # We can't use enum because we are using STI.

  included do
    scope :basic, -> { where(type: 'Bots::Basic') }
    scope :withdrawal, -> { where(type: 'Bots::Withdrawal') }
    scope :webhook, -> { where(type: 'Bots::Webhook') }
    scope :barbell, -> { where(type: 'Bots::Barbell') }
  end

  def basic?
    type == 'Bots::Basic'
  end

  def withdrawal?
    type == 'Bots::Withdrawal'
  end

  def webhook?
    type == 'Bots::Webhook'
  end

  def barbell?
    type == 'Bots::Barbell'
  end
end
