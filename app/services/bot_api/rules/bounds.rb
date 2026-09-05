# frozen_string_literal: true

module BotApi
  module Rules
    # Rules::Withdrawal validates its numbers only once the rule is scheduled, so a stopped rule
    # could be written with values that make its first start raise. Both write paths — create
    # and update_rule_settings — check them here first. Same bounds as the model: percentages in
    # (0, 100], amounts > 0.
    module Bounds
      THRESHOLD_TYPES = %w[fee_percentage min_amount].freeze
      PERCENT = (0.0..100.0)
      AMOUNT = (0.0..Float::INFINITY)

      # nil when every given value is acceptable; a Result otherwise.
      def self.check(threshold_type:, withdrawal_percentage: nil, max_fee_percentage: nil, min_amount: nil)
        unless THRESHOLD_TYPES.include?(threshold_type.to_s)
          return Result.failure(:validation_failed, 'invalid_threshold_type',
                                "threshold_type must be one of: #{THRESHOLD_TYPES.join(', ')}.")
        end

        # The active threshold's own field is required — the model's conditional presence
        # validation would otherwise fire at the first start, not here.
        given = { min_amount: min_amount, max_fee_percentage: max_fee_percentage }
        required = threshold_type.to_s == 'min_amount' ? :min_amount : :max_fee_percentage
        if given[required].blank?
          return Result.failure(:validation_failed, 'missing_required_parameter',
                                "#{required} is required for threshold_type #{threshold_type}.")
        end

        # Every supplied number is checked, active threshold or not: both are persisted, and the
        # web threshold switch can activate the inactive one later without re-validating it.
        checks = { withdrawal_percentage: [withdrawal_percentage, PERCENT],
                   max_fee_percentage: [max_fee_percentage, PERCENT], min_amount: [min_amount, AMOUNT] }
        checks.each do |name, (value, range)|
          next if value.blank?
          next if Number.within(value, range)&.positive?

          return Result.failure(:validation_failed, 'invalid_number',
                                "#{name} must be a number greater than 0#{' and at most 100' if range == PERCENT}.")
        end
        nil
      end
    end
  end
end
