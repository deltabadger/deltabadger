module Transaction::Broadcastable
  extend ActiveSupport::Concern

  included do
    after_create_commit :broadcast_to_bot, unless: -> { bot.legacy? }
    after_create_commit :broadcast_below_minimums_warning_to_barbell_bot, if: -> { bot.barbell? }
    after_create_commit :broadcast_quote_amount_limit_update, if: -> { bot.barbell? }
  end

  private

  def broadcast_to_bot
    # TODO: When transactions point to real asset ids, we can use the asset ids directly instead of symbols
    ticker = exchange.tickers.find_by(base_asset: base_asset, quote_asset: quote_asset)
    decimals = {
      base_asset.symbol => ticker.base_decimals,
      quote_asset.symbol => ticker.quote_decimals
    }

    if bot.transactions.limit(2).count == 1
      broadcast_remove_to(
        ["user_#{bot.user_id}", :bot_updates],
        target: 'orders_list_placeholder'
      )
    end

    broadcast_prepend_to(
      ["user_#{bot.user_id}", :bot_updates],
      target: 'orders_list',
      partial: 'bots/orders/order',
      locals: { order: self, decimals: decimals, exchange_name: exchange.name, current_user: bot.user }
    )
  end

  def broadcast_below_minimums_warning_to_barbell_bot
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

  def broadcast_quote_amount_limit_update
    return unless success? && bot.quote_amount_limited?

    broadcast_replace_to(
      ["user_#{bot.user_id}", :bot_updates],
      target: 'settings-amount-limit-info',
      partial: 'bots/settings/amount_limit_info',
      locals: { bot: bot }
    )
  end
end
