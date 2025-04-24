# FIXME: Remove this file after migrating to new store methods

module Bot::LegacyMethods
  extend ActiveSupport::Concern

  def bot_type
    {
      'Bots::Basic' => 'trading',
      'Bots::Withdrawal' => 'withdrawal',
      'Bots::Webhook' => 'webhook'
    }[type] || nil
  end

  def base
    settings['base']
  end

  def quote
    settings['quote']
  end

  def price
    settings['price']
  end

  def additional_price
    settings['additional_price']
  end

  def percentage
    settings['percentage']
  end

  def interval
    settings['interval']
  end

  def interval_enabled
    settings['interval_enabled']
  end

  def side
    settings['type']
  end

  def additional_type
    settings['additional_type']
  end

  def order_type
    settings['order_type']
  end

  def force_smart_intervals
    settings['force_smart_intervals']
  end

  def smart_intervals_value
    settings['smart_intervals_value']
  end

  def price_range_enabled
    settings['price_range_enabled']
  end

  def price_range
    settings['price_range']
  end

  def currency
    settings['currency']
  end

  def threshold
    settings['threshold']
  end

  def threshold_enabled
    settings['threshold_enabled']
  end

  def address
    settings['address']
  end

  def already_triggered_types
    settings['already_triggered_types']
  end

  def already_triggered_types=(triggered_type)
    settings['already_triggered_types'] = triggered_type
  end

  def trigger_possibility
    settings['trigger_possibility']
  end

  def additional_trigger_url
    settings['additional_trigger_url']
  end

  def trigger_url
    settings['trigger_url']
  end

  def name
    settings['name']
  end

  def called_bot(webhook)
    return 'additional_bot' if additional_type_enabled? && additional_trigger_url == webhook

    'main_bot' if trigger_url == webhook
  end

  def already_triggered?(type)
    already_triggered_types.include? type
  end

  def possible_to_call_a_webhook?(webhook)
    return true if every_time?

    !already_triggered?(called_bot(webhook))
  end

  def first_time?
    trigger_possibility == 'first_time'
  end

  def every_time?
    trigger_possibility == 'every_time'
  end

  def additional_type_enabled?
    settings['additional_type_enabled']
  end

  def market?
    order_type == 'market'
  end

  def limit?
    !market?
  end

  def buyer?
    side == 'buy'
  end

  def seller?
    !buyer?
  end

  def use_subaccount
    settings.fetch('use_subaccount', false)
  end

  def selected_subaccount
    settings.fetch('selected_subaccount', '')
  end

  def pnl
    return if transactions.empty? || bot_type == 'withdrawal' || last_successful_transaction.nil?

    stats = Presenters::Api::Stats.call(
      bot: self,
      transactions: daily_transaction_aggregates.order(created_at: :desc)
    )
    (stats[:currentValue].to_f - stats[:totalInvested].to_f) / stats[:totalInvested].to_f
  end
end
