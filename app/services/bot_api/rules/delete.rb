# frozen_string_literal: true

module BotApi
  module Rules
    class Delete
      def self.call(user:, rule_id:)
        rule = user.rules.where.not(status: :deleted).find_by(id: rule_id.to_i)
        return Result.failure(:not_found, 'rule_not_found', 'Rule not found.') unless rule

        if rule.working?
          return Result.failure(:conflict, 'rule_active',
                                "Rule must be stopped before deleting. Current status: #{rule.status}.")
        end

        rule.delete
        Result.success({ id: rule.id, status: rule.status.to_s })
      end
    end
  end
end
