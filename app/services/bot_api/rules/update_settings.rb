# frozen_string_literal: true

module BotApi
  module Rules
    # Updates fields on a stopped rule. The MCP tool exposes four: withdrawal
    # percentage, max fee percentage, min amount, and threshold type — REST
    # keeps the same surface.
    class UpdateSettings
      EDITABLE = %i[withdrawal_percentage max_fee_percentage min_amount threshold_type].freeze

      def self.call(user:, rule_id:, **attrs)
        new(user: user, rule_id: rule_id, **attrs).call
      end

      def initialize(user:, rule_id:,
                     withdrawal_percentage: nil, max_fee_percentage: nil,
                     min_amount: nil, threshold_type: nil)
        @user = user
        @rule_id = rule_id
        @attrs = {
          withdrawal_percentage: withdrawal_percentage,
          max_fee_percentage: max_fee_percentage,
          min_amount: min_amount,
          threshold_type: threshold_type
        }
      end

      def call
        rule = @user.rules.find_by(id: @rule_id.to_i)
        return Result.failure(:not_found, 'rule_not_found', 'Rule not found.') unless rule

        if rule.working?
          return Result.failure(:conflict, 'rule_active',
                                "Rule must be stopped before updating settings. Current status: #{rule.status}.")
        end

        updates = build_updates
        return no_updates if updates.empty?

        apply(rule, updates)
        if rule.save
          Result.success({ id: rule.id, updated: updates.keys.map(&:to_s) })
        else
          Result.failure(:validation_failed, 'rule_save_failed',
                         "Failed to update rule: #{rule.errors.full_messages.join(', ')}")
        end
      end

      private

      def build_updates
        updates = {}
        # Numeric fields are stored as strings on the Rule model — the MCP
        # tool converted via `.to_s`, REST matches that to avoid behavior drift.
        updates[:withdrawal_percentage] = @attrs[:withdrawal_percentage].to_s if @attrs[:withdrawal_percentage].present?
        updates[:max_fee_percentage] = @attrs[:max_fee_percentage].to_s if @attrs[:max_fee_percentage].present?
        updates[:min_amount] = @attrs[:min_amount].to_s if @attrs[:min_amount].present?
        updates[:threshold_type] = @attrs[:threshold_type] if @attrs[:threshold_type].present?
        updates
      end

      def apply(rule, updates)
        rule.withdrawal_percentage = updates[:withdrawal_percentage] if updates.key?(:withdrawal_percentage)
        rule.max_fee_percentage = updates[:max_fee_percentage] if updates.key?(:max_fee_percentage)
        rule.min_amount = updates[:min_amount] if updates.key?(:min_amount)
        rule.threshold_type = updates[:threshold_type] if updates.key?(:threshold_type)
      end

      def no_updates
        Result.failure(:validation_failed, 'no_updates_provided', 'No settings provided to update.')
      end
    end
  end
end
