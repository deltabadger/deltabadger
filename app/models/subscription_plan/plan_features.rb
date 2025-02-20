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
        crypto_index
        custom_portfolios
        portfolio_rebalancing
        portfolio_backtesting
        smart_allocation
        ai_insights
        legendary_badger_nft
      ]
    end

    def nft_features
      %w[
        legendary_badger_nft
      ]
    end

    def bots_features
      %w[
        dca
        fee_cutter
        automatic_withdrawals
        rebalanced_dca
        barbell_strategy
        crypto_index
        custom_portfolios
        portfolio_rebalancing
      ]
    end

    def analyzer_features
      %w[
        portfolio_backtesting
        smart_allocation
        ai_insights
      ]
    end

    def coming_features
      %w[
        rebalanced_dca
        barbell_strategy
        crypto_index
        custom_portfolios
        portfolio_rebalancing
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
      ]
    end

    def pro_features
      basic_features + %w[
        crypto_index
        custom_portfolios
        portfolio_rebalancing
        portfolio_backtesting
        smart_allocation
        ai_insights
      ]
    end

    def legendary_features
      pro_features + %w[
        legendary_badger_nft
      ]
    end
  end
end
