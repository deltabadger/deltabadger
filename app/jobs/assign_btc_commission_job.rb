require 'utilities/hash'

class AssignBtcCommissionJob < ApplicationJob
  queue_as :default

  def perform(payment_id)
    payment = Payment.find(payment_id)
    fiat_commission = payment.commission
    btc_price = get_btc_price(quote: payment.currency)
    btc_commission = (fiat_commission / btc_price).ceil(8)
    payment.update!(btc_commission: btc_commission)

    affiliate = payment.user.referrer
    previous_btc_commission = affiliate.unexported_btc_commission
    affiliate.update!(unexported_btc_commission: previous_btc_commission + btc_commission)
  end

  private

  def client
    @client ||= CoingeckoClient.new
  end

  def get_btc_price(quote:)
    quote = quote.downcase
    result = @client.simple_price(['bitcoin'], [quote])
    raise StandardError, result.errors if result.failure?

    Utilities::Hash.dig_or_raise(result.data, 'bitcoin', quote)
  end
end
