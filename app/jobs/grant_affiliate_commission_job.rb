require 'utilities/hash'

class GrantAffiliateCommissionJob < ApplicationJob
  queue_as :default

  def perform(payment_id)
    payment = Payment.find(payment_id)
    return if payment.commission.zero?

    affiliate = payment.user.referrer
    return unless affiliate.active?

    previous_unexported_btc_commission = affiliate.unexported_btc_commission
    btc_commission_amount = btc_commission(payment)
    return if btc_commission_amount.zero?

    send_registration_reminder(affiliate, btc_commission_amount) if affiliate.btc_address.blank?
    User.transaction do
      payment.update!(btc_commission: btc_commission_amount)
      affiliate.update!(unexported_btc_commission: previous_unexported_btc_commission + btc_commission_amount)
    end
  end

  private

  def client
    @client ||= CoingeckoClient.new
  end

  def get_btc_price(quote:)
    quote = quote.downcase
    Rails.cache.fetch("btc_price_#{quote}", expires_in: 1.minute) do
      result = @client.simple_price(['bitcoin'], [quote])
      raise StandardError, result.errors if result.failure?

      Utilities::Hash.dig_or_raise(result.data, 'bitcoin', quote)
    end
  end

  def btc_commission(payment)
    if payment.by_bitcoin?
      commission_multiplier = payment.commission / payment.total
      (payment.btc_paid * commission_multiplier).floor(8)
    else
      btc_price = get_btc_price(quote: payment.currency)
      (payment.commission / btc_price).floor(8)
    end
  end

  def send_registration_reminder(affiliate, amount)
    AffiliateMailer.with(
      referrer: affiliate,
      amount: amount
    ).registration_reminder.deliver_later
  end
end
