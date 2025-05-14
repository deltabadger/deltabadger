module Transaction::Barbell
  extend ActiveSupport::Concern

  included do
    after_create_commit :broadcast_below_minimums_warning_to_bot
    after_create_commit :broadcast_quote_amount_limit_update, if: -> { success? && bot.quote_amount_limited? }
  end

  private

  def broadcast_below_minimums_warning_to_bot
    first_transactions = bot.transactions.limit(3)
    return unless first_transactions.count == 2
    return unless [first_transactions.first.skipped?, first_transactions.last.skipped?].any?

    first_transaction = first_transactions.first
    second_transaction = first_transactions.last

    broadcast_replace_to(
      ["user_#{bot.user_id}", :bot_updates],
      target: 'modal',
      partial: 'bots/barbell/warning_below_minimums',
      locals: locals_for_below_minimums_warning(first_transaction, second_transaction)
    )
  end

  def locals_for_below_minimums_warning(first_transaction, second_transaction)
    if first_transaction.skipped? && second_transaction.skipped?
      ticker0 = first_transaction.exchange.tickers.find_by(
        base_asset_id: first_transaction.base_asset.id,
        quote_asset_id: first_transaction.quote_asset.id
      )
      ticker1 = second_transaction.exchange.tickers.find_by(
        base_asset_id: second_transaction.base_asset.id,
        quote_asset_id: second_transaction.quote_asset.id
      )
      {
        base0_symbol: first_transaction.base_asset.symbol,
        base1_symbol: second_transaction.base_asset.symbol,
        base0_minimum_base_size: ticker0.minimum_base_size,
        base0_minimum_quote_size: ticker0.minimum_quote_size,
        quote_symbol: first_transaction.quote_asset.symbol,
        base1_minimum_base_size: ticker1.minimum_base_size,
        base1_minimum_quote_size: ticker1.minimum_quote_size,
        exchange_name: first_transaction.exchange.name,
        missed_count: 2
      }
    else
      bought_transaction = first_transaction.skipped? ? second_transaction : first_transaction
      missed_transaction = first_transaction.skipped? ? first_transaction : second_transaction
      {
        bought_quote_amount: bought_transaction.quote_amount,
        quote_symbol: bought_transaction.quote_asset.symbol,
        bought_symbol: bought_transaction.base_asset.symbol,
        missed_symbol: missed_transaction.base_asset.symbol,
        missed_minimum_base_size: missed_transaction.base_asset.min_base_size,
        missed_minimum_quote_size: missed_transaction.base_asset.min_quote_size,
        exchange_name: first_transaction.exchange.name,
        missed_count: 1
      }
    end
  end

  def broadcast_quote_amount_available_before_limit_update
    broadcast_replace_to(
      ["user_#{bot.user_id}", :bot_updates],
      target: 'settings-amount-limit-info',
      partial: 'bots/barbell/settings/amount_limit_info',
      locals: { bot: bot }
    )
    return unless bot.quote_amount_limit_reached?

    bot.stop
    bot.notify_stopped_by_quote_amount_limit

    # after stopping outside of the controller, we need to broadcast the streams the same way as
    # app/views/bots/stop.turbo_stream.erb
    broadcast_replace_to(
      ["user_#{bot.user_id}", :bot_updates],
      target: 'settings',
      partial: 'bots/barbell/settings',
      locals: { bot: bot }
    )
    broadcast_replace_to(
      ["user_#{bot.user_id}", :bot_updates],
      target: 'exchange_select',
      partial: 'bots/exchange_select',
      locals: { bot: bot }
    )
  end
end
