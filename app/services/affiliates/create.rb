module Affiliates
  class Create < ::BaseService
    DEFAULT_AFFILIATE_PARAMS = {
      max_profit: Affiliate::DEFAULT_MAX_PROFIT,
      total_bonus_percent: Affiliate::DEFAULT_BONUS_PERCENT
    }.freeze

    BASE_PERMITTED_PARAMS =
      %i[type btc_address code visible_name visible_link discount_percent check].freeze
    INDIVIDUAL_PERMITTED_PARAMS = BASE_PERMITTED_PARAMS
    EU_COMPANY_PERMITTED_PARAMS = (BASE_PERMITTED_PARAMS + %i[name address vat_number]).freeze

    def call(user:, affiliate_params:)
      add_visible_link_scheme!(affiliate_params)

      affiliate_params = if affiliate_params[:type] == 'individual'
                           individual_params(affiliate_params)
                         else
                           eu_company_params(affiliate_params)
                         end

      affiliate = Affiliate.new(affiliate_params.merge(DEFAULT_AFFILIATE_PARAMS))
      user.affiliate = affiliate
      user.save!

      Result::Success.new(user.affiliate)
    rescue ActiveRecord::RecordNotSaved
      remove_visible_link_scheme!(affiliate)
      Result::Failure.new(*affiliate.errors, data: affiliate)
    rescue StandardError => e
      Raven.capture_exception(e)
      Result::Failure.new('Referral program registration failed')
    end

    private

    def add_visible_link_scheme!(affiliate_params)
      visible_link = affiliate_params[:visible_link]
      return if visible_link.blank?

      visible_link_scheme = affiliate_params[:visible_link_scheme]
      affiliate_params[:visible_link] = visible_link_scheme + '://' + visible_link
    end

    def remove_visible_link_scheme!(affiliate)
      affiliate.visible_link = %r{(?:https?://)?(.*)}.match(affiliate[:visible_link])[1]
    end

    def individual_params(affiliate_params)
      affiliate_params.permit(INDIVIDUAL_PERMITTED_PARAMS)
    end

    def eu_company_params(affiliate_params)
      affiliate_params.permit(EU_COMPANY_PERMITTED_PARAMS)
    end
  end
end
