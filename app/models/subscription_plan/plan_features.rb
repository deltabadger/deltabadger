module SubscriptionPlan::PlanFeatures
  extend ActiveSupport::Concern

  included do # rubocop:disable Metrics/BlockLength
    def self.feature_list
      %w[
        dca
        fee_cutter
        automatic_withdrawals
        rebalanced_dca
        barbell_strategy
        crypto_barbell
        crypto_index
        custom_portfolios
        portfolio_rebalancing
        portfolio_backtesting
        smart_allocation
        ai_insights
        fireheads_community
        legendary_badger_nft
      ]
    end

    def features
      if free?
        free_features
      elsif basic?
        basic_features
      elsif pro?
        pro_features
      elsif legendary?
        legendary_features
      end
    end

    private

    def free_features
      %w[
        dca
      ]
    end

    def basic_features
      free_features + %w[
        fee_cutter
        automatic_withdrawals
        rebalanced_dca
        barbell_strategy
        crypto_barbell
        crypto_index
      ]
    end

    def pro_features
      basic_features + %w[
        custom_portfolios
        portfolio_rebalancing
        portfolio_backtesting
        smart_allocation
        ai_insights
        fireheads_community
      ]
    end

    def legendary_features
      pro_features + %w[
        legendary_badger_nft
      ]
    end
  end
end
