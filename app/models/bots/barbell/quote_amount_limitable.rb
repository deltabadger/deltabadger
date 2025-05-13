module Bots::Barbell::QuoteAmountLimitable
  extend ActiveSupport::Concern

  included do
    store_accessor :settings, :quote_amount_limited, :quote_amount_limit

    before_save -> { self.quote_amount_limit = 10 * quote_amount }, unless: -> { quote_amount_limit.present? }

    validates :quote_amount_limited, inclusion: { in: [true, false] }, if: -> { quote_amount_limited.present? }
    validates :quote_amount_limit, numericality: { greater_than: 0 }, if: :quote_amount_limited?
  end

  def quote_amount_limited?
    quote_amount_limited.present? && quote_amount_limited
  end

  def quote_amount_available_before_limit
    return Float::INFINITY unless quote_amount_limited?

    quote_amount_spent = transactions.success.where('created_at > ?', started_at).sum(:quote_amount)
    [quote_amount_limit - quote_amount_spent, 0].max
  end

  def quote_amount_limit_reached?
    # give a 1% buffer to avoid rounding & fees errors
    quote_amount_limited? && quote_amount_available_before_limit <= [quote_amount, quote_amount_limit].min * 0.01
  end

  def broadcast_quote_amount_available_before_limit_update
    return if quote_amount_available_before_limit == quote_amount_limit

    broadcast_update_to(
      ["user_#{user.id}", :bot_updates],
      target: 'quote_amount_available_before_limit',
      html: "(#{quote_amount_available_before_limit.round(2)} #{quote_asset.symbol} left)"
    )
  end
end
