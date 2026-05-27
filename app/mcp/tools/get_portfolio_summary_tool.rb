# frozen_string_literal: true

class GetPortfolioSummaryTool < ApplicationMCPTool
  tool_name 'get_portfolio_summary'
  description 'Get a portfolio summary including global P/L, bot count breakdown, and per-bot P/L'
  read_only

  def perform
    result = BotApi::Portfolio::Summary.call(user: current_user)
    data = result.data

    if data[:empty]
      render text: 'No bots found. Create a bot to start tracking your portfolio.'
      return
    end

    render text: present(data)
  end

  private

  def present(data)
    lines = []
    totals = data[:totals]
    lines << 'Portfolio Summary'
    lines << '================'
    lines << "Total bots: #{totals[:total]} (#{totals[:working]} active, #{totals[:stopped]} stopped, #{totals[:created]} not started)"
    lines << ''
    lines.concat(global_pnl_lines(data[:global_pnl]))
    lines << ''
    lines << '--- Per-Bot Summary ---'
    data[:bots].each { |bot| lines << bot_line(bot) }
    lines.join("\n")
  end

  def global_pnl_lines(pnl)
    return ['Global P/L: Not available (needs market data)'] unless pnl

    out = []
    sign = pnl[:percent] >= 0 ? '+' : ''
    out << "Global P/L: #{sign}#{(pnl[:percent] * 100).round(2)}%"
    if pnl[:profit_usd]
      profit_sign = pnl[:profit_usd] >= 0 ? '+' : ''
      out << "Profit (USD): #{profit_sign}$#{pnl[:profit_usd].round(2)}"
    end
    out
  end

  def bot_line(bot)
    pair = bot[:pair] || 'N/A'

    if bot[:metrics_error]
      "- #{bot[:label]} (#{pair}) | #{bot[:status]} | Metrics unavailable"
    elsif bot[:metrics]
      metrics = bot[:metrics]
      pnl_sign = metrics[:pnl] >= 0 ? '+' : ''
      invested = metrics[:invested]&.round(2)
      "- #{bot[:label]} (#{pair}) | #{bot[:status]} | P/L: #{pnl_sign}#{(metrics[:pnl] * 100).round(2)}% | Invested: #{invested} #{bot[:quote_asset]}"
    else
      "- #{bot[:label]} (#{pair}) | #{bot[:status]} | No metrics yet"
    end
  end
end
