# frozen_string_literal: true

module BotApi
  module Rules
    class Stop
      def self.call(user:, rule_id:)
        new(user: user, rule_id: rule_id).call
      end

      def initialize(user:, rule_id:)
        @user = user
        @rule_id = rule_id
      end

      def call
        rule = @user.rules.find_by(id: @rule_id.to_i)
        return Result.failure(:not_found, 'rule_not_found', 'Rule not found.') unless rule

        unless rule.working?
          return Result.failure(:conflict, 'rule_not_active',
                                "Rule ##{rule.id} is not active (#{rule.status}).",
                                data: { id: rule.id, status: rule.status.to_s })
        end

        rule.stop
        Result.success({ id: rule.id, status: rule.status.to_s })
      end
    end
  end
end
