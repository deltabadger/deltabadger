# frozen_string_literal: true

module BotApi
  # Only these mean yes or no. ActiveModel's cast turns 'maybe' into true and nil into false,
  # and both of those have been the wrong answer for something irreversible.
  module Boolean
    ANSWERS = { true => true, false => false, 'true' => true, 'false' => false }.freeze

    # nil for absent (or `default:` when the caller has one), nil for anything unrecognised.
    def self.parse(value, default: nil)
      return default if value.nil? || (value.respond_to?(:empty?) && value.empty?)

      ANSWERS.fetch(value, nil)
    end
  end
end
