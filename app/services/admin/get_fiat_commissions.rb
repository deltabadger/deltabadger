module Admin
  class GetFiatCommissions < BaseService
    def call
      referred_fiat_payments = Payment.paid.where(payment_type: %w[stripe zen wire])
                                      .where('commission > ?', 0)
                                      .where.not(external_statuses: 'Commission granted')

      btc_price_result = get_btc_price
      return btc_price_result if btc_price_result.failure?

      Rails.logger.info("BTC price: #{btc_price_result.data}")

      referred_fiat_payments.each do |payment|
        Rails.logger.info("Processing payment #{payment.id}, commission: #{payment.commission}, currency: #{payment.currency}")
        commission_in_btc = (payment.commission / btc_price_result.data[payment.currency]).ceil(8)
        Rails.logger.info("Commission in BTC: #{commission_in_btc}")
        affiliate = Affiliate.find(User.find(payment['user_id'])['referrer_id'])
        Rails.logger.info("Affiliate: #{affiliate.id}, unexported_btc_commission: #{affiliate.unexported_btc_commission}")
        update_commission(affiliate, commission_in_btc)
        Rails.logger.info('Commission granted')
        payment.update!(external_statuses: 'Commission granted')
        Rails.logger.info('Payment updated')
      end
      Rails.logger.info('Fiat payments\' commissions granted')
      Result::Success.new("Fiat payments\\' commissions granted")
    rescue StandardError => e
      Rails.logger.error("Couldn't grant the commissions: #{e}")
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
