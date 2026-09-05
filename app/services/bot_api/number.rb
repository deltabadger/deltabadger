# frozen_string_literal: true

module BotApi
  # The one way a service turns user input into a number. `'abc'.to_f` is 0, `'1e9'.to_f` is a
  # billion and a long enough digit string is Infinity; none of that is what anybody typed. Plain
  # decimals only — no sign, no exponent, bounded digits — parsed exactly. Callers convert to
  # Float only where the model already stores floats.
  module Number
    FORMAT = /\A\d{1,15}(\.\d{1,18})?\z/

    # BigDecimal or nil.
    def self.parse(value)
      text = value.to_s.strip
      text.match?(FORMAT) ? BigDecimal(text) : nil
    end

    # Integer or nil — a whole number, however it arrived. MCP casts a `type: 'number'` property
    # to Float, so the year 2025 reaches a service as 2025.0; 12.5 is still a refusal.
    def self.integer(value)
      number = parse(value)
      number.to_i if number&.frac&.zero?
    end

    # Present and inside the range; nil otherwise.
    def self.within(value, range)
      number = parse(value)
      number if number && range.cover?(number)
    end
  end
end
