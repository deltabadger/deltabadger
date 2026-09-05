# frozen_string_literal: true

module BotApi
  module Rules
    class Start
      def self.call(user:, rule_id:)
        new(user: user, rule_id: rule_id).call
      end

      def initialize(user:, rule_id:)
        @user = user
        @rule_id = rule_id
      end

      def call
        rule = @user.rules.where.not(status: :deleted).find_by(id: @rule_id.to_i)
        return Result.failure(:not_found, 'rule_not_found', 'Rule not found.') unless rule

        if rule.working?
          return Result.failure(:conflict, 'rule_already_active',
                                "Rule ##{rule.id} is already active (#{rule.status}).",
                                data: { id: rule.id, status: rule.status.to_s })
        end

        # :validation_failed, not a bespoke status — Api::V1::BaseController#status_for maps
        # unknown statuses to 500.
        if rule.exchange&.retired?
          return Result.failure(:validation_failed, 'exchange_retired',
                                I18n.t('errors.exchange_retired'),
                                data: { id: rule.id, exchange: rule.exchange.name })
        end

        rule.start
        Result.success({ id: rule.id, status: rule.status.to_s })
      end
    end
  end
end
