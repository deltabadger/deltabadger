module Bot::Barbell
  extend ActiveSupport::Concern

  included do # rubocop:disable Metrics/BlockLength
    validate :validate_barbell_bot_settings, if: :barbell?

    def set_barbell_orders
      quote_asset = settings['quote'].upcase
      quote_amount = settings['quote_amount'].to_f
      result = exchange.get_balance(asset: quote_asset)
      return result unless result.success?

      available_quote_balance = result.data
      if available_quote_balance < quote_amount
        update!(status: 'stopped')
        # TODO: notify user
        return Result::Failure.new('Insufficient quote balance')
      end

      result = get_balances_and_prices
      return result unless result.success?

      balances_and_prices = result.data
      order_sizes = calculate_order_sizes(**balances_and_prices)
      order_sizes.each_with_index do |amount, index|
        puts "amount: #{amount}, index: #{index}"

        base_asset = settings["base#{index}"].upcase
        # TODO: cache this value
        result = exchange.get_minimum_base_size(base_asset: base_asset, quote_asset: quote_asset)
        return result unless result.success?

        minimum_base_size = result.data
        puts "minimum_base_size: #{minimum_base_size}"
        if amount < minimum_base_size
          create_skipped_transaction!(amount, balances_and_prices["price#{index}".to_sym])
          next
        end

        result = market_buy(
          base_asset: base_asset,
          quote_asset: quote_asset,
          amount: amount,
          amount_type: 'base'
        )

        puts "result: #{result.inspect}"

        if result.success?
          update!(status: 'pending')
          order_id = result.data.dig('success_response', 'order_id')
          Bot::FetchOrderResultJob.perform_later(id, order_id)

          # bot.update(status: 'pending')
          # result = @fetch_order_result.call(bot.id, result.data, fixing_price)
          # check_allowable_balance(get_api(bot), bot, fixing_price, notify)
          # send_user_to_sendgrid(bot)
        else
          create_failed_transaction!(errors: result.errors)
          # TODO: notify user?
          # TODO: stop the bot?
          return result
        end
        # handle if order 1 or 2 fail
        # test default failures and retries
        # add views for it all

        # calculate missed amounts if bot was restarted
      end

      update!(status: 'working')
      Result::Success.new
    end

    def fetch_order_result(order_id)
      result = exchange.get_order(order_id: order_id)
      return result unless result.success?

      amount = result.data.dig('order', 'filled_size').to_f
      rate = result.data.dig('order', 'average_filled_price').to_f
      create_successful_transaction!(
        order_id,
        rate,
        amount
      )

      Result::Success.new
    end

    def cancel_scheduled_orders
      sidekiq_places = [
        Sidekiq::ScheduledSet.new,
        Sidekiq::Queue.new(exchange.name.downcase),
        Sidekiq::RetrySet.new
      ]
      sidekiq_places.each do |place|
        place.each do |job|
          job.delete if job.queue == exchange.name.downcase &&
                        job.display_class == 'Bot::SetBarbellOrdersJob' &&
                        job.display_args == [id]
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

    def get_balances_and_prices
      base0 = settings['base0']
      base1 = settings['base1']
      result = exchange.get_balance(asset: base0.upcase)
      return result unless result.success?

      balance0 = result.data
      result = exchange.get_balance(asset: base1.upcase)
      return result unless result.success?

      balance1 = result.data
      result = exchange.get_ask_price(base_asset: base0, quote_asset: quote)
      return result unless result.success?

      price0 = result.data
      result = exchange.get_ask_price(base_asset: base1, quote_asset: quote)
      return result unless result.success?

      price1 = result.data
      Result::Success.new({
                            balance0: balance0,
                            balance1: balance1,
                            price0: price0,
                            price1: price1
                          })
    end

    def calculate_order_sizes(balance0:, balance1:, price0:, price1:)
      quote_amount = settings['quote_amount'].to_f
      allocation0 = settings['allocation0'].to_f
      allocation1 = 1 - allocation0
      balance0_in_quote = balance0 * price0
      balance1_in_quote = balance1 * price1
      total_balance_in_quote = balance0_in_quote + balance1_in_quote + quote_amount
      target_balance0_in_quote = total_balance_in_quote * allocation0
      target_balance1_in_quote = total_balance_in_quote * allocation1
      base0_offset = [0, target_balance0_in_quote - balance0_in_quote].max
      base1_offset = [0, target_balance1_in_quote - balance1_in_quote].max
      base0_order_size_in_quote = [base0_offset, quote_amount].min
      base1_order_size_in_quote = [base1_offset, quote_amount - base0_order_size_in_quote].min
      base0_order_size_in_base = base0_order_size_in_quote / price0
      base1_order_size_in_base = base1_order_size_in_quote / price1
      [base0_order_size_in_base, base1_order_size_in_base]
    end

    def create_successful_transaction!(order_id, rate, amount)
      bot_quote_amount = settings['quote_amount'].to_f
      transactions.create!(
        offer_id: order_id,
        status: :success,
        rate: rate,
        amount: amount,
        bot_interval: interval,
        bot_price: bot_quote_amount,  # this is the quote amount in the bot settings
        transaction_type: 'REGULAR'
      )
    end

    def create_skipped_transaction!(base_amount, rate)
      bot_quote_amount = settings['quote_amount'].to_f
      transactions.create!(
        status: :skipped,
        rate: rate,
        amount: base_amount,
        bot_interval: interval,
        bot_price: bot_quote_amount,  # this is the quote amount in the bot settings
        transaction_type: 'REGULAR'
      )
    end

    def create_failed_transaction!(errors: nil, order_id: nil, rate: nil, amount: nil)
      bot_quote_amount = settings['quote_amount'].to_f
      transactions.create!(
        error_messages: errors,
        offer_id: order_id,
        status: :failure,
        rate: rate,
        amount: amount,
        bot_interval: interval,
        bot_price: bot_quote_amount,  # this is the quote amount in the bot settings
        transaction_type: 'REGULAR'
      )
    end
  end
end
