class BarbellBot < Bot
  include ActionCable::Channel::Broadcasting

  store_accessor :transient_data, :pending_quote_amount, :pending_orders, :last_scheduled_orders_at
  store_accessor :settings, :base0, :base1, :quote, :quote_amount, :allocation0, :interval

  validates :quote_amount, presence: true, numericality: { greater_than: 0 }, on: :start
  validates :quote, :base0, :base1, presence: true, format: { with: /\A[A-Z0-9]+\z/ }, on: :start
  validates :interval, presence: true, inclusion: { in: INTERVAL_OPTIONS }, on: :start
  validates :allocation0, presence: true, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }, on: :start

  validates :pending_quote_amount,
            numericality: { greater_than_or_equal_to: 0 },
            on: :update,
            if: -> { pending_quote_amount.present? }

  validate :validate_barbell_bot_exchange, if: :exchange_id?, on: :start
  validate :validate_unchangeable_assets, on: :update
  validate :validate_unchangeable_interval, on: :update

  after_initialize :set_exchange_client, if: :exchange_id?
  after_save :set_exchange_client, if: :saved_change_to_exchange_id?
  after_update_commit :broadcast_countdown_update, if: :saved_change_to_status?
  after_update_commit :broadcast_metrics_update, if: -> { saved_change_to_metrics_status? && metrics_ready? }

  def set_exchange_client
    exchange&.set_client(api_key: api_key)
  end

  def api_key
    if exchange.present?
      user.api_keys.trading.where(exchange: exchange).first || new_api_key
    else
      new_api_key
    end
  end

  def start
    if valid?(:start) && update(
      status: 'pending',
      restarts: 0,
      delay: 0,
      current_delay: 0,
      started_at: Time.current,
      transient_data: {
        pending_quote_amount: quote_amount
      }
    )
      Bot::SetBarbellOrdersJob.perform_later(id)
      true
    else
      false
    end
  end

  def stop
    if update(
      status: 'stopped',
      transient_data: {},
      stopped_at: Time.current
    )
      cancel_scheduled_orders
      true
    else
      false
    end
  end

  def delete
    if update(
      status: 'deleted',
      transient_data: {},
      stopped_at: Time.current
    )
      cancel_scheduled_orders if exchange.present?
      true
    else
      false
    end
  end

  def set_barbell_orders(temp_quote_amount)
    # return Result::Success.new
    update!(
      status: 'pending',
      pending_quote_amount: (pending_quote_amount.presence || 0) + temp_quote_amount
    )

    result = exchange.get_balance(asset: quote)
    return result unless result.success?

    available_quote_balance = result.data
    if available_quote_balance < temp_quote_amount
      stop
      # TODO: notify user
      return Result::Failure.new('Insufficient quote balance')
    end

    result = get_balances_and_prices
    return result unless result.success?

    balances_and_prices = result.data
    order_sizes = calculate_order_sizes(**balances_and_prices)
    order_sizes.each_with_index do |order_size, index|
      base_amount = order_size[:amount]
      base_amount_in_quote = order_size[:amount_in_quote]
      symbol_info = exchange.get_symbol_info(base_asset: base(index), quote_asset: quote)
      return symbol_info unless symbol_info.success?

      # TODO: in some cases rate is 0 ?!
      rate = balances_and_prices["price#{index}".to_sym]

      puts "order_size: #{order_size.inspect}"
      puts "amount: #{base_amount}, index: #{index}, rate: #{rate}"

      minimum_base_size = symbol_info.data[:minimum_base_size]
      puts "minimum_base_size: #{minimum_base_size}"
      if base_amount < minimum_base_size
        create_skipped_transaction!(
          base: base(index),
          quote: quote,
          amount: base_amount,
          rate: rate
        )
        next
      end

      result = market_buy(
        base_asset: base(index),
        quote_asset: quote,
        amount: base_amount,
        amount_type: :base
      )

      if result.success?
        order_id = result.data.dig('success_response', 'order_id')
        update!(
          pending_orders: (pending_orders.presence || []) << order_id,
          pending_quote_amount: [0, (pending_quote_amount.presence || 0) - base_amount_in_quote].max
        )
        Bot::FetchOrderResultJob.perform_later(id, order_id)

        # bot.update(status: 'pending')
        # result = @fetch_order_result.call(bot.id, result.data, fixing_price)
        # check_allowable_balance(get_api(bot), bot, fixing_price, notify)
        # send_user_to_sendgrid(bot)
      else
        create_failed_transaction!(
          base: base(index),
          quote: quote,
          errors: result.errors,
          amount: base_amount,
          rate: rate
        )
        # TODO: notify user?
        # TODO: stop the bot?
        return result
      end

      # test default failures and retries
      # add views for it all

      # calculate missed amounts if bot was restarted
    end

    update!(status: 'working') unless pending_orders.any?
    Result::Success.new
  end

  def fetch_order_result(order_id)
    result = exchange.get_order(order_id: order_id)
    return result unless result.success?

    create_successful_transaction!(
      order_id: order_id,
      base: result.data[:base],
      quote: result.data[:quote],
      rate: result.data[:rate],
      amount: result.data[:amount]
    )

    new_pending_orders = (pending_orders || []) - [order_id]
    if new_pending_orders.empty?
      update!(pending_orders: new_pending_orders, status: 'working')
    else
      update!(pending_orders: new_pending_orders)
    end

    Result::Success.new
  end

  def next_set_barbell_orders_job_at
    sidekiq_places = [
      Sidekiq::RetrySet.new,
      Sidekiq::ScheduledSet.new
    ]
    sidekiq_places.each do |place|
      place.each do |job|
        return job.at if job.queue == exchange.name.downcase &&
                         job.display_class == 'Bot::SetBarbellOrdersJob' &&
                         job.display_args == [id]
      end
    end
    nil
  end

  def next_scheduled_orders_quote_amount
    interval = settings['interval']
    last_scheduled_orders_at = if last_scheduled_orders_at.present?
                                 DateTime.parse(last_scheduled_orders_at)
                               else
                                 started_at
                               end
    intervals_since_last_scheduled_orders = ((Time.current - last_scheduled_orders_at) / 1.public_send(interval)).floor
    missed_quote_amount = quote_amount * intervals_since_last_scheduled_orders
    (pending_quote_amount.presence || 0) + missed_quote_amount
  end

  def next_scheduled_orders_at
    interval = settings['interval']
    checkpoint = started_at
    loop do
      checkpoint += 1.public_send(interval)
      return checkpoint if checkpoint > Time.current
    end
  end

  def metrics(recalculate: false) # rubocop:disable Metrics/MethodLength
    Rails.cache.delete("bot_#{id}_metrics") if recalculate
    data = Rails.cache.fetch("bot_#{id}_metrics", expires_in: 30.days) do
      update!(metrics_status: :pending)
      puts "recalculating metrics for bot #{id}"

      data = {
        chart: {
          labels: [],
          series: [
            [], # value
            []  # invested
          ]
        }
      }

      total_quote_amount_invested = {
        base0 => 0,
        base1 => 0
      }
      total_base_amount_acquired = {
        base0 => 0,
        base1 => 0
      }
      current_value_in_quote = {
        base0 => 0,
        base1 => 0
      }

      rates = {
        base0 => [],
        base1 => []
      }
      amounts = {
        base0 => [],
        base1 => []
      }

      transactions.order(created_at: :asc).each do |transaction|
        next if transaction.rate.zero?
        next if transaction.base.nil?

        # chart data
        data[:chart][:labels] << transaction.created_at
        amount = transaction.success? ? transaction.amount : 0
        total_quote_amount_invested[transaction.base] += amount * transaction.rate
        total_base_amount_acquired[transaction.base] += amount
        data[:chart][:series][1] << total_quote_amount_invested.values.sum
        current_value_in_quote[transaction.base] = total_base_amount_acquired[transaction.base] * transaction.rate
        data[:chart][:series][0] << current_value_in_quote.values.sum

        # metrics data
        rates[transaction.base] << transaction.rate
        amounts[transaction.base] << amount
      end

      rates.each_with_index do |(base, rates_array), index|
        weighted_average = Utilities::Math.weighted_average(rates_array, amounts[base])
        data["base#{index}_average_buy_rate".to_sym] = weighted_average
      end

      data[:total_base0_amount_acquired] = total_base_amount_acquired[base0]
      data[:total_base1_amount_acquired] = total_base_amount_acquired[base1]
      from_quote_value = total_quote_amount_invested.values.sum
      to_quote_value = current_value_in_quote.values.sum
      data[:total_quote_amount_invested] = from_quote_value
      data[:current_investment_value_in_quote] = to_quote_value
      data[:pnl] = (to_quote_value - from_quote_value) / from_quote_value

      data
    end
    update!(metrics_status: :ready)
    data
  end

  def available_exchanges_for_current_settings
    return Exchange.available_for_barbell_bots unless base0.present? && base1.present? && quote.present?

    Exchange.available_for_barbell_bots.select do |exchange|
      [
        exchange.get_symbol_info(base_asset: base0, quote_asset: quote),
        exchange.get_symbol_info(base_asset: base1, quote_asset: quote)
      ].map { |result| result.success? && result.data }.all?
    end
  end

  # @param asset_type: :base_asset or :quote_asset
  def available_assets_for_current_settings(asset_type:)
    Exchange.available_for_barbell_bots.each_with_object({}) do |exchange, asset_map|
      next if exchange.get_info.failure?

      exchange.get_info.data[:symbols].each do |symbol|
        ticker = symbol[asset_type]
        name = symbol["#{asset_type}_name".to_sym]
        key = [ticker, name]

        asset_map[key] ||= { ticker: ticker, name: name, exchanges: [] }
        asset_map[key][:exchanges] << exchange.name if asset_map[key][:exchanges].exclude?(exchange.name)
      end
    end.values
  end

  private

  def base(index)
    public_send("base#{index}")
  end

  def new_api_key
    user.api_keys.new(exchange: exchange, status: :pending, key_type: :trading)
  end

  def validate_barbell_bot_exchange
    result0 = exchange.get_symbol_info(base_asset: base0, quote_asset: quote)
    result1 = exchange.get_symbol_info(base_asset: base1, quote_asset: quote)
    return unless result0.failure? || result1.failure? || result0.data.nil? || result1.data.nil?

    errors.add(:exchange, :unsupported, message: 'Invalid combination of assets for the selected exchange')
  end

  def validate_unchangeable_assets
    return unless transactions.exists?
    return unless settings_changed?
    return unless settings_was['quote'] != settings['quote'] ||
                  settings_was['base0'] != settings['base0'] ||
                  settings_was['base1'] != settings['base1']

    errors.add(:settings, :unchangeable_assets,
               message: 'Assets cannot be changed after orders have been created')
  end

  def validate_unchangeable_interval
    return unless working? || pending?
    return unless settings_changed?
    return unless settings_was['interval'] != settings['interval']

    errors.add(:settings, :unchangeable_interval,
               message: 'Interval cannot be changed while the bot is running')
  end

  def get_balances_and_prices
    result = exchange.get_balance(asset: base0)
    return result unless result.success?

    balance0 = result.data
    result = exchange.get_balance(asset: base1)
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
    [
      {
        amount: base0_order_size_in_base,
        amount_in_quote: base0_order_size_in_quote
      },
      {
        amount: base1_order_size_in_base,
        amount_in_quote: base1_order_size_in_quote
      }
    ]
  end

  def create_successful_transaction!(order_id:, base:, quote:, rate:, amount:)
    interval = settings['interval']
    transactions.create!(
      offer_id: order_id,
      status: :success,
      rate: rate,
      amount: amount,
      base: base,
      quote: quote,
      bot_interval: interval,
      bot_price: quote_amount,  # this is the quote amount in the bot settings
      transaction_type: 'REGULAR'
    )
  end

  def create_skipped_transaction!(base:, quote:, amount:, rate:)
    interval = settings['interval']
    transactions.create!(
      status: :skipped,
      rate: rate,
      amount: amount,
      base: base,
      quote: quote,
      bot_interval: interval,
      bot_price: quote_amount,  # this is the quote amount in the bot settings
      transaction_type: 'REGULAR'
    )
  end

  def create_failed_transaction!(base:, quote:, errors: nil, order_id: nil, rate: nil, amount: nil)
    interval = settings['interval']
    transactions.create!(
      error_messages: errors,
      offer_id: order_id,
      status: :failure,
      rate: rate,
      amount: amount,
      base: base,
      quote: quote,
      bot_interval: interval,
      bot_price: quote_amount,  # this is the quote amount in the bot settings
      transaction_type: 'REGULAR'
    )
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

  def broadcast_countdown_update
    broadcast_update_to(
      ["bot_#{id}", :countdown],
      target: "bot_#{id}_countdown",
      partial: 'barbell_bots/bot/countdown',
      locals: { bot: self }
    )
  end

  def broadcast_metrics_update
    broadcast_render_to(
      ["bot_#{id}", :metrics],
      partial: 'barbell_bots/bot/metrics',
      locals: { bot: self }
    )
  end
end
