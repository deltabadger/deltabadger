# frozen_string_literal: true

class GetBotDetailsTool < ApplicationMCPTool
  tool_name 'get_bot_details'
  description 'Get detailed information about a specific bot including performance metrics (P/L, average price, invested, current value)'
  read_only

  property :bot_id, type: 'number', required: true, description: 'The bot ID'

  def perform
    user = current_user
    bot = user.bots.not_deleted.find_by(id: bot_id.to_i)

    unless bot
      render text: 'Bot not found.'
      return
    end

    lines = []
    lines << "Bot: #{bot.label}"
    lines << "Type: #{bot.type.demodulize.titleize}"
    lines << "Status: #{bot.status}"
    lines << "Exchange: #{bot.exchange&.name || 'N/A'}"

    if bot.dca_dual_asset?
      lines << "Pair: #{bot.base0_asset&.symbol}+#{bot.base1_asset&.symbol}/#{bot.quote_asset&.symbol}"
    elsif bot.respond_to?(:base_asset)
      lines << "Pair: #{bot.base_asset&.symbol}/#{bot.quote_asset&.symbol}"
    end

    lines << "Interval: #{bot.settings['interval'] || 'N/A'}"
    lines << "Amount per order: #{bot.settings['quote_amount']} #{bot.quote_asset&.symbol}"
    lines << "Orders executed: #{bot.successful_transaction_count}"

    lines << "Started: #{bot.started_at.strftime('%Y-%m-%d %H:%M UTC')}" if bot.started_at

    begin
      metrics = bot.metrics
      if metrics.present?
        lines << ''
        lines << '--- Performance ---'
        invested = metrics[:total_quote_amount_invested]
        value = metrics[:total_amount_value_in_quote]
        pnl = metrics[:pnl]
        avg_price = metrics[:average_buy_price]

        lines << "Total invested: #{format_number(invested)} #{bot.quote_asset&.symbol}" if invested
        lines << "Current value: #{format_number(value)} #{bot.quote_asset&.symbol}" if value
        lines << "P/L: #{format_percent(pnl)}" if pnl
        lines << "Average buy price: #{format_number(avg_price)} #{bot.quote_asset&.symbol}" if avg_price
        if metrics[:total_base_amount]
          lines << "Total acquired: #{format_number(metrics[:total_base_amount])} #{bot.respond_to?(:base_asset) ? bot.base_asset&.symbol : 'units'}"
        end
      end
    rescue StandardError => e
      lines << ''
      lines << "Metrics unavailable: #{e.message}"
    end

    render text: lines.join("\n")
  end

  private

  def format_number(num)
    return 'N/A' unless num

    num.is_a?(Numeric) ? num.round(2).to_s : num.to_s
  end

  def format_percent(pnl)
    return 'N/A' unless pnl

    sign = pnl >= 0 ? '+' : ''
    "#{sign}#{(pnl * 100).round(2)}%"
  end
end
