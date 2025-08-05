module Payment::Typeable
  extend ActiveSupport::Concern

  # We use this concern to get enum-like functionality for the type column
  # We can't use enum because we are using STI.

  included do
    scope :btcpay, -> { where(type: 'Payments::Btcpay') }
    scope :not_btcpay, -> { where.not(type: 'Payments::Btcpay') }

    scope :stripe, -> { where(type: 'Payments::Stripe') }
    scope :not_stripe, -> { where.not(type: 'Payments::Stripe') }

    scope :wire, -> { where(type: 'Payments::Wire') }
    scope :not_wire, -> { where.not(type: 'Payments::Wire') }

    scope :zen, -> { where(type: 'Payments::Zen') }
    scope :not_zen, -> { where.not(type: 'Payments::Zen') }

    scope :fiat, -> { where(type: %w[Payments::Stripe Payments::Wire Payments::Zen]) }
    scope :bitcoin, -> { where.not(type: %w[Payments::Btcpay]) }
  end

  def btcpay?
    type == 'Payments::Btcpay'
  end

  def stripe?
    type == 'Payments::Stripe'
  end

  def wire?
    type == 'Payments::Wire'
  end

  def zen?
    type == 'Payments::Zen'
  end

  def fiat?
    type.in?(%w[Payments::Stripe Payments::Wire Payments::Zen])
  end

  def bitcoin?
    type.in?(%w[Payments::Btcpay])
  end
end
