module Affiliates
  class Create < ::BaseService
    DEFAULT_AFFILIATE_PARAMS = {
      max_profit: 20,
      total_bonus_percent: 0.3
    }.freeze

    def call(user:, affiliate_params:)
      if affiliate_params[:type] == "PrivateAffiliate"
        affiliate_params = private_params(affiliate_params)
        affiliate_class = PrivateAffiliate
      else
        affiliate_params = eu_company_params(affiliate_params)
        affiliate_class = EuCompanyAffiliate
      end

      affiliate = affiliate_class.new(affiliate_params.merge(DEFAULT_AFFILIATE_PARAMS))
      user.affiliate = affiliate
      user.save!

      Result::Success.new(user.affiliate)
    rescue ActiveRecord::RecordNotSaved
      Result::Failure.new(*affiliate.errors)
    rescue StandardError => e
      Raven.capture_exception(e)
      Result::Failure.new('Referral program registration failed')
    end

    private

    def private_params(affiliate_params)
      affiliate_params.permit(:btc_address, :code, :visible_name, :visible_link, :discount_percent, :check)
    end

    def eu_company_params(affiliate_params)
      private_params(affiliate_params).permit(:name, :address, :vat_number)
    end
  end
end
