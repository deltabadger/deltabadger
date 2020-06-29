module Affiliates
  class Create < ::BaseService
    DEFAULT_AFFILIATE_PARAMS = {
      max_profit: 20,
      discount_percent: 0.2,
      total_bonus_percent: 0.3
    }.freeze

    def call(user:, affiliate_params:)
      affiliate = Affiliate.new(affiliate_params.merge(DEFAULT_AFFILIATE_PARAMS))
      user.affiliate = affiliate
      user.save!

      Result::Success.new(user.affiliate)
    rescue ActiveRecord::RecordNotSaved
      Result::Failure.new(*affiliate.errors)
    rescue StandardError => e
      Raven.capture_exception(e)
      Result::Failure.new('Affiliate program registration failed')
    end
  end
end
