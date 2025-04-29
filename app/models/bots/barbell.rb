class Bots::Barbell < Bot
  include ActionCable::Channel::Broadcasting

  store_accessor :settings, :base0_asset_id, :base1_asset_id, :quote_asset_id, :quote_amount,
                 :allocation0, :interval, :market_cap_adjusted

  validates :quote_amount, presence: true, numericality: { greater_than: 0 }
  validates :interval, presence: true, inclusion: { in: INTERVALS }
  validates :allocation0, presence: true, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }
  validate :validate_barbell_bot_exchange, if: :exchange_id?, on: :update
  validate :validate_external_ids, on: :update
  validate :validate_unchangeable_assets, on: :update
  validate :validate_unchangeable_interval, on: :update

  after_save :unset_exchange_implementation, if: :saved_change_to_exchange_id?

  include Schedulable
  include Bots::Barbell::OrderSetter
  include Bots::Barbell::OrderCreator
  include Bots::Barbell::Measurable
  include Bots::Barbell::Schedulable

  def exchange
    @exchange ||= super
    if @exchange.present? && !exchange_implementation_set?
      @exchange.set_exchange_implementation(api_key: api_key)
      @exchange_implementation_set = true
    end
    @exchange
  end

  def exchange_implementation_set?
    @exchange_implementation_set || false
  end

  def unset_exchange_implementation
    @exchange_implementation_set = false
  end

  def api_key
    @api_key ||= user.api_keys.trading.find_by(exchange_id: exchange_id) ||
                 user.api_keys.trading.new(exchange_id: exchange_id, status: :pending)
  end

  def start(ignore_missed_orders: true)
    update_params = {
      status: 'working',
      started_at: ignore_missed_orders ? Time.current : nil,
      transient_data: ignore_missed_orders ? {} : nil
    }.compact

    if valid?(:start) && update(update_params)
      if ignore_missed_orders
        Bot::SetBarbellOrdersJob.perform_later(self)
      else
        Bot::SetBarbellOrdersJob.set(wait_until: next_interval_checkpoint_at).perform_later(self)
        #  Schedule the broadcast status bar update to make sure sidekiq has time to schedule the job
        Bot::BroadcastStatusBarUpdateJob.set(wait: 0.25.seconds).perform_later(self)
      end
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
    base_asset_ids = [base0_asset_id, base1_asset_id].compact
    scope = ExchangeTicker.where(exchange: Exchange.available_for_barbell_bots)
    scope = scope.where(quote_asset_id: quote_asset_id) if quote_asset_id.present?
    scope = scope.where(base_asset_id: base_asset_ids) if base_asset_ids.any?
    exchange_ids = if base_asset_ids.size > 1
                     scope.group_by(&:exchange_id)
                          .transform_values { |tickers| tickers.map(&:base_asset_id).uniq }
                          .select { |_, b_a_ids| b_a_ids.size >= base_asset_ids.size }
                          .keys
                   else
                     scope.pluck(:exchange_id).uniq
                   end
    Exchange.where(id: exchange_ids)
  end

  # @param asset_type: :base_asset or :quote_asset
  def available_assets_for_current_settings(asset_type:, include_exchanges: false)
    available_exchanges = exchange.present? ? [exchange] : Exchange.available_for_barbell_bots
    base_asset_ids = [base0_asset_id, base1_asset_id].compact

    case asset_type
    when :base_asset
      scope = ExchangeTicker.where(exchange: available_exchanges)
                            .where.not(base_asset_id: base_asset_ids)
      scope = scope.where(quote_asset_id: quote_asset_id) if quote_asset_id.present?
    when :quote_asset
      scope = ExchangeTicker.where(exchange: available_exchanges)
                            .where.not(quote_asset_id: base_asset_ids + [quote_asset_id])
      if base_asset_ids.any?
        scope = scope.where(base_asset_id: base_asset_ids)
        valid_quote_asset_ids = scope.pluck(:quote_asset_id, :base_asset_id)
                                     .group_by(&:first)
                                     .transform_values { |pairs| pairs.map(&:last) }
                                     .select { |_, bb| base_asset_ids.map { |b| bb.include?(b.to_i) }.all? }
                                     .keys
        scope = scope.where(quote_asset_id: valid_quote_asset_ids)
      end
    end
    asset_ids = scope.pluck("#{asset_type}_id").uniq
    include_exchanges ? Asset.includes(:exchanges).where(id: asset_ids) : Asset.where(id: asset_ids)
  end

  def restarting?
    stopped? && last_pending_quote_amount_calculated_at.present?
  end

  def restarting_within_interval?
    restarting? && last_action_job_at_iso8601.present? &&
      DateTime.parse(last_action_job_at_iso8601) > 1.public_send(interval).ago
  end

  def market_cap_adjusted?
    market_cap_adjusted.present? && market_cap_adjusted
  end

  def effective_allocation0
    if market_cap_adjusted?
      result0 = base0_asset.get_market_cap
      result1 = base1_asset.get_market_cap
      if result0.success? && result1.success?
        (result0.data.to_f / (result0.data + result1.data)).round(2)
      else
        Rails.logger.error("Failed to get market cap for #{base0_asset.symbol}") if result0.failure?
        Rails.logger.error("Failed to get market cap for #{base1_asset.symbol}") if result1.failure?
        raise StandardError, "Failed to get market cap adjusted allocation for barbell bot #{id}"
      end
    else
      allocation0
    end
  end

  def base0_asset
    @base0_asset ||= base0_asset_id.present? ? Asset.find(base0_asset_id) : nil
  end

  def base1_asset
    @base1_asset ||= base1_asset_id.present? ? Asset.find(base1_asset_id) : nil
  end

  def quote_asset
    @quote_asset ||= quote_asset_id.present? ? Asset.find(quote_asset_id) : nil
  end

  private

  def validate_external_ids
    errors.add(:base0_asset_id, :invalid) unless Asset.exists?(base0_asset_id)
    errors.add(:base1_asset_id, :invalid) unless Asset.exists?(base1_asset_id)
    errors.add(:quote_asset_id, :invalid) unless Asset.exists?(quote_asset_id)
  end

  def validate_barbell_bot_exchange
    return if exchange.tickers.exists?(base_asset: base0_asset, quote_asset: quote_asset) &&
              exchange.tickers.exists?(base_asset: base1_asset, quote_asset: quote_asset)

    errors.add(:exchange, :unsupported, message: I18n.t('errors.bots.exchange_asset_mismatch', exchange_name: exchange.name))
  end

  def validate_unchangeable_assets
    return unless transactions.exists?
    return unless settings_changed?

    errors.add(:base0_asset_id, :unchangeable) if settings_was['base0_asset_id'] != settings['base0_asset_id']
    errors.add(:base1_asset_id, :unchangeable) if settings_was['base1_asset_id'] != settings['base1_asset_id']
    errors.add(:quote_asset_id, :unchangeable) if settings_was['quote_asset_id'] != settings['quote_asset_id']
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
