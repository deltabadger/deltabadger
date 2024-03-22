module Admin
  class GetFiatCommissions < BaseService
    def call
      referred_fiat_payments = get_referred_fiat_payments
      return Result::Failure.new("Couldn\\'t fetch the payments' data") unless referred_fiat_payments.success?

      subscription_plans = SubscriptionPlan.all.map { |s| [s['id'], s] }.to_h
      referred_fiat_payments.data.each do |payment|
        subscription_plan = subscription_plans[(payment['subscription_plan_id'])]
        affiliate = Affiliate.find(User.find(payment['user_id'])['referrer_id'])
        affiliate_commission_percent = affiliate.total_bonus_percent - affiliate.discount_percent
        commission_in_btc = commission_in_btc(payment.currency, affiliate_commission_percent, subscription_plan)
        return commission_in_btc if commission_in_btc.failure?

        update_commission(affiliate, commission_in_btc.data)
        payment.update(external_statuses: 'Commission granted')
      end
      Result::Success.new("Fiat payments\\' commissions granted")
    rescue StandardError
      return commission_in_btc if commission_in_btc.failure?
    end

    private

    # get paid fiat payments without the granted commission
    def get_referred_fiat_payments
      fiat_payments_list = Payment.where(status: 'paid', payment_type: %w[stripe zen wire]).where.not(external_statuses: 'Commission granted')
      fiat_payments_with_referrers = fiat_payments_list.to_a.filter { |payment| !User.find(payment['user_id'])['referrer_id'].nil? }
      Result::Success.new(fiat_payments_with_referrers)
    rescue StandardError
      Result::Failure.new("Couldn\\'t fetch the payments' data")
    end

    def btc_cost(subscription_plan, currency)
      undiscounted_cost = currency == 'EUR' ? subscription_plan.cost_eu : subscription_plan.cost_other
      btc_price_result = Admin::BitcoinPriceGetter.call(quote: currency)
      return btc_price_result if btc_price_result.failure?

      Result::Success.new(undiscounted_cost / btc_price.data)
    end

    def commission_in_btc(currency, commission_percent, subscription_plan)
      btc_cost = btc_cost(subscription_plan, currency)
      return btc_cost if btc_cost.failure?

      Result::Success.new((commission_percent * btc_cost.data).ceil(8))
    end

    def update_commission(affiliate, commission)
      previous_crypto_commission = affiliate.unexported_crypto_commission
      affiliate.update(unexported_crypto_commission: previous_crypto_commission + commission)
    end
  end
end
