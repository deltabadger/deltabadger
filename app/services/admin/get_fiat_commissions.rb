module Admin
  class GetFiatCommissions < BaseService
    def call
      referred_fiat_payments = get_referred_fiat_payments
      return Result::Failure.new("Couldn\\'t fetch the payments' data") unless referred_fiat_payments.success?

      subscription_plan_variants = SubscriptionPlanVariant.all.map { |s| [s['id'], s] }.to_h
      referred_fiat_payments.data.each do |payment|
        subscription_plan_variant = subscription_plan_variants[(payment['subscription_plan_variant_id'])]
        affiliate = Affiliate.find(User.find(payment['user_id'])['referrer_id'])
        affiliate_commission_percent = affiliate.total_bonus_percent - affiliate.discount_percent
        commission_in_btc_result = get_commission_in_btc(payment.currency,
                                                         affiliate_commission_percent,
                                                         subscription_plan_variant)
        return commission_in_btc_result if commission_in_btc_result.failure?

        update_commission(affiliate, commission_in_btc_result.data)
        payment.update(external_statuses: 'Commission granted')
      end
      Result::Success.new("Fiat payments\\' commissions granted")
    rescue StandardError => e
      Result::Failure.new("Couldn\\'t grant the commissions: #{e}")
    end

    private

    # get paid fiat payments without the granted commission
    def get_referred_fiat_payments
      fiat_payments_list = Payment.paid.where(payment_type: %w[stripe zen wire])
                                  .where.not(external_statuses: 'Commission granted')
      fiat_payments_with_referrers = fiat_payments_list.to_a.filter do |payment|
        !User.find(payment['user_id'])['referrer_id'].nil?
      end
      Result::Success.new(fiat_payments_with_referrers)
    rescue StandardError
      Result::Failure.new("Couldn\\'t fetch the payments' data")
    end

    def get_btc_cost(subscription_plan_variant, currency)
      undiscounted_cost = currency == 'EUR' ? subscription_plan_variant.cost_eur : subscription_plan_variant.cost_usd
      btc_price_result = Admin::BitcoinPriceGetter.call(quote: currency)
      return btc_price_result if btc_price_result.failure?

      Result::Success.new(undiscounted_cost / btc_price.data)
    end

    def get_commission_in_btc(currency, commission_percent, subscription_plan_variant)
      btc_cost_result = get_btc_cost(subscription_plan_variant, currency)
      return btc_cost_result if btc_cost_result.failure?

      Result::Success.new((commission_percent * btc_cost_result.data).ceil(8))
    end

    def update_commission(affiliate, commission)
      previous_btc_commission = affiliate.unexported_btc_commission
      affiliate.update(unexported_btc_commission: previous_btc_commission + commission)
    end
  end
end
