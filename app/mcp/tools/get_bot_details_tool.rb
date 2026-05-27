# frozen_string_literal: true

class GetBotDetailsTool < ApplicationMCPTool
  tool_name 'get_bot_details'
  description 'Get detailed information about a specific bot including performance metrics (P/L, average price, invested, current value)'
  read_only

  property :bot_id, type: 'number', required: true, description: 'The bot ID'

  def perform
    result = BotApi::Bots::Get.call(user: current_user, bot_id: bot_id)
    return render(text: result.error_message) unless result.success?

    render text: present(result.data)
  end

  private

  def present(data)
    lines = []
    lines << "Bot: #{data[:label]}"
    lines << "Type: #{data[:type].to_s.demodulize.titleize}"
    lines << "Status: #{data[:status]}"
    lines << "Exchange: #{data[:exchange] || 'N/A'}"
    lines << "Pair: #{data[:pair]}" if data[:pair]
    lines << "Interval: #{data[:interval] || 'N/A'}"
    lines << "Amount per order: #{data[:quote_amount]} #{data[:quote_asset]}"
    lines << "Orders executed: #{data[:orders_executed]}"
    lines << "Started: #{data[:started_at].strftime('%Y-%m-%d %H:%M UTC')}" if data[:started_at]

    if data[:metrics]
      metrics = data[:metrics]
      lines << ''
      lines << '--- Performance ---'
      lines << "Total invested: #{format_number(metrics[:invested])} #{data[:quote_asset]}" if metrics[:invested]
      lines << "Current value: #{format_number(metrics[:value])} #{data[:quote_asset]}" if metrics[:value]
      lines << "P/L: #{format_percent(metrics[:pnl])}" if metrics[:pnl]
      lines << "Average buy price: #{format_number(metrics[:average_buy_price])} #{data[:quote_asset]}" if metrics[:average_buy_price]
      lines << "Total acquired: #{format_number(metrics[:total_base_amount])} #{data[:base_asset] || 'units'}" if metrics[:total_base_amount]
    elsif data[:metrics_error]
      lines << ''
      lines << "Metrics unavailable: #{data[:metrics_error]}"
    end

    lines.join("\n")
  end

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
