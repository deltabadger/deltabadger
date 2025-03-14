class Bots::Barbell < Bot
  include ActionCable::Channel::Broadcasting

  store_accessor :settings, :base0, :base1, :quote, :quote_amount, :allocation0, :interval

  validates :quote_amount, presence: true, numericality: { greater_than: 0 }, on: :start
  validates :quote, :base0, :base1, presence: true, format: { with: /\A[A-Z0-9]+\z/ }, on: :start
  validates :interval, presence: true, inclusion: { in: INTERVALS }, on: :start
  validates :allocation0, presence: true, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }, on: :start

  validate :validate_barbell_bot_exchange, if: :exchange_id?, on: :start
  validate :validate_unchangeable_assets, on: :update
  validate :validate_unchangeable_interval, on: :update

  after_initialize :set_exchange_client, if: :exchange_id?
  after_save :set_exchange_client, if: :saved_change_to_exchange_id?
  after_update_commit :broadcast_status_bar_update, if: :saved_change_to_status?

  include Schedulable
  include Bots::Barbell::OrderSetter
  include Bots::Barbell::OrderCreator
  include Bots::Barbell::Measurable
  include Bots::Barbell::Schedulable

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

  def start(ignore_missed_orders: true)
    update_params = {
      status: 'working',
      started_at: ignore_missed_orders ? Time.current : nil,
      transient_data: ignore_missed_orders ? {} : nil
    }.compact

    if valid?(:start) && update(update_params)
      Bot::SetBarbellOrdersJob.perform_later(self)
      true
    else
      false
    end
  end

  def stop
    if update(
      status: 'stopped',
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
      stopped_at: Time.current
    )
      cancel_scheduled_orders if exchange.present?
      true
    else
      false
    end
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

  def restarting?
    stopped? && last_pending_quote_amount_calculated_at_iso8601.present?
  end

  def restarting_within_interval?
    restarting? && last_action_job_at_iso8601.present? &&
      DateTime.parse(last_action_job_at_iso8601) > 1.public_send(interval).ago
  end

  def broadcast_status_bar_update
    broadcast_update_to(
      ["bot_#{id}", :status_bar],
      target: "bot_#{id}_status_bar",
      partial: 'barbell_bots/status/status_bar',
      locals: { bot: self }
    )
  end

  private

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

  def cancel_scheduled_orders
    sidekiq_places = [
      Sidekiq::ScheduledSet.new,
      Sidekiq::Queue.new(exchange.name.downcase),
      Sidekiq::RetrySet.new
    ]
    sidekiq_places.each do |place|
      place.each do |job|
        job.delete if job.queue == action_job_config[:queue] &&
                      job.display_class == action_job_config[:class] &&
                      job.display_args.first == action_job_config[:args].first
      end
    end
  end

  def action_job_config
    {
      queue: exchange.name.downcase,
      class: 'Bot::SetBarbellOrdersJob',
      args: [{ '_aj_globalid' => to_global_id.to_s }]
    }
  end
end
