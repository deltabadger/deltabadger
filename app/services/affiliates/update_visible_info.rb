module Affiliates
  class UpdateVisibleInfo < BaseService
    VISIBLE_INFO_PARAMS = %i[visible_name visible_link_scheme visible_link].freeze

    def call(affiliate:, params:)
      return unless affiliate.active?

      affiliate_params = params.permit(VISIBLE_INFO_PARAMS)

      affiliate.update(affiliate_params)
    end
  end
end
