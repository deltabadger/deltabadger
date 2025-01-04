require 'utilities/hash'

class GrantAffiliateCommissionJob < ApplicationJob
  queue_as :default

  def perform(payment_id)
    payment = Payment.find(payment_id)
    fiat_commission = payment.commission
    btc_price = get_btc_price(quote: payment.currency)
    btc_commission = (fiat_commission / btc_price).ceil(8)
    affiliate = payment.user.referrer
    previous_btc_commission = affiliate.unexported_btc_commission
    User.transaction do
      payment.update!(btc_commission: btc_commission)
      affiliate.update!(unexported_btc_commission: previous_btc_commission + btc_commission)
    end
    send_registration_reminder(affiliate, btc_commission) if affiliate.btc_address.blank?
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
end
