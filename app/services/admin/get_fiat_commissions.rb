module Admin
  class GetFiatCommissions < BaseService
    def call
      referred_fiat_payments = Payment.paid.where(payment_type: %w[stripe zen wire])
                                      .where('commission > ?', 0)
                                      .where.not(external_statuses: 'Commission granted')

      btc_price_result = get_btc_price
      return btc_price_result if btc_price_result.failure?

      referred_fiat_payments.each do |payment|
        commission_in_btc = (payment.commission / btc_price_result.data[payment.currency]).ceil(8)
        affiliate = Affiliate.find(User.find(payment['user_id'])['referrer_id'])
        update_commission(affiliate, commission_in_btc)
        payment.update!(external_statuses: 'Commission granted')
      end
      Result::Success.new("Fiat payments\\' commissions granted")
    rescue StandardError => e
      Result::Failure.new("Couldn\\'t grant the commissions: #{e}")
    end

    private

    def get_btc_price
      btc_price = {}
      %w[EUR USD].each do |currency|
        btc_price_result = Admin::BitcoinPriceGetter.call(quote: currency)
        return btc_price_result if btc_price_result.failure?

        btc_price[currency] = btc_price_result.data
      end
      Result::Success.new(btc_price)
    end

    def update_commission(affiliate, commission)
      previous_btc_commission = affiliate.unexported_btc_commission
      affiliate.update!(unexported_btc_commission: previous_btc_commission + commission)
    end
  end
end
