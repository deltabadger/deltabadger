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

    def private_permitted_params
      %i[btc_address code visible_name visible_link discount_percent check].freeze
    end

    def eu_company_permitted_params
      (private_permitted_params + %i[name address vat_number]).freeze
    end

    def private_params(affiliate_params)
      affiliate_params.permit(private_permitted_params)
    end

    def eu_company_params(affiliate_params)
      affiliate_params.permit(eu_company_permitted_params)
    end
  end
end
