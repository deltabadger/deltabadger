module Bot::Barbell
  extend ActiveSupport::Concern

  included do # rubocop:disable Metrics/BlockLength
    validate :validate_barbell_bot_settings, if: :barbell?

    def set_barbell_orders
      result = get_order_sizes
      return result unless result.success?

      order_sizes = result.data
      order_sizes.each_with_index do |order_size, index|
        next unless order_size.positive?

        result = exchange.get_minimum_base_size(base_asset: settings["base#{index}"], quote_asset: quote)
        return result unless result.success?

        minimum_base_size = result.data
        next unless order_size >= minimum_base_size

        result = market_buy(
          base_asset: settings["base#{index}"],
          quote_asset: quote,
          amount: order_size,
          amount_type: 'base'
        )
        if result.success?
          puts "bought #{order_size} #{settings["base#{index}"]} for #{result.data['id']}"
        else
          puts "error buying #{order_size} #{settings["base#{index}"]}: #{result.data}"
        end
      end
    end

    private

    def validate_barbell_bot_settings # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
      return if settings['quote_amount'].present? && settings['quote_amount'].to_f.positive? &&
                settings['quote'].present? &&
                settings['interval'].present? &&
                settings['base0'].present? &&
                settings['base1'].present? &&
                settings['allocation0'].present? && settings['allocation0'].to_f.between?(0, 1)

      errors.add(:settings, :invalid_settings, message: 'Invalid settings')
    end

    def get_order_sizes
      base0 = settings['base0']
      base1 = settings['base1']
      allocation0 = settings['allocation0'].to_f
      quote_amount = settings['quote_amount'].to_f
      allocation1 = 1 - allocation0
      result = exchange.get_balance(asset: base0.upcase)
      return result unless result.success?

      base0_balance = result.data
      result = exchange.get_balance(asset: base1.upcase)
      return result unless result.success?

      base1_balance = result.data
      result = exchange.get_ask_price(base_asset: base0, quote_asset: quote)
      return result unless result.success?

      base0_price = result.data
      result = exchange.get_ask_price(base_asset: base1, quote_asset: quote)
      return result unless result.success?

      base1_price = result.data
      base0_balance_in_quote = base0_balance * base0_price
      base1_balance_in_quote = base1_balance * base1_price
      total_balance_in_quote = base0_balance_in_quote + base1_balance_in_quote + quote_amount
      base0_target_balance_in_quote = total_balance_in_quote * allocation0
      base1_target_balance_in_quote = total_balance_in_quote * allocation1
      base0_offset = [0, base0_target_balance_in_quote - base0_balance_in_quote].max
      base1_offset = [0, base1_target_balance_in_quote - base1_balance_in_quote].max
      base0_order_size_in_quote = [base0_offset, quote_amount].min
      base1_order_size_in_quote = [base1_offset, quote_amount - base0_order_size_in_quote].min
      base0_order_size_in_base = base0_order_size_in_quote / base0_price
      base1_order_size_in_base = base1_order_size_in_quote / base1_price

      Result::Success.new([base0_order_size_in_base, base1_order_size_in_base])
    end
  end
end
