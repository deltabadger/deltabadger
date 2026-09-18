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
      text = decimal_text(value)
      text.match?(FORMAT) ? BigDecimal(text) : nil
    end

    # A number that arrived AS a number (JSON, an MCP `number` property) prints in scientific
    # notation once it is small — 0.00005.to_s is "5.0e-05" — and that is not something anybody
    # typed. It is written out as the plain decimal it is, then judged by the same FORMAT, so a
    # negative, an overlong or a non-finite one is still nil. Text is judged exactly as typed.
    def self.decimal_text(value)
      return value.to_d.to_s('F') if value.is_a?(Float) && value.finite?

      value.to_s.strip
    end
    private_class_method :decimal_text

    # Integer or nil — a whole number, however it arrived. MCP casts a `type: 'number'` property
    # to Float, so the year 2025 reaches a service as 2025.0; 12.5 is still a refusal.
    def self.integer(value)
      # Of the Float itself: written out as a decimal it is rounded to 16 significant digits, and
      # 1.0000000000000002 must not come back as 1.
      return nil if value.is_a?(Float) && !(value.finite? && value == value.floor)

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
