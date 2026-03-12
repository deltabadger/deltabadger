# frozen_string_literal: true

class GetPortfolioSummaryTool < ApplicationMCPTool
  tool_name 'get_portfolio_summary'
  description 'Get a portfolio summary including global P/L, bot count breakdown, and per-bot P/L'
  read_only

  def perform
    user = current_user
    bots = user.bots.not_deleted

    if bots.empty?
      render text: 'No bots found. Create a bot to start tracking your portfolio.'
      return
    end

    lines = []

    # Bot count breakdown
    total = bots.size
    working = bots.select(&:working?).size
    stopped = bots.where(status: :stopped).size
    created = bots.where(status: :created).size

    lines << 'Portfolio Summary'
    lines << '================'
    lines << "Total bots: #{total} (#{working} active, #{stopped} stopped, #{created} not started)"
    lines << ''

    # Global PnL
    global_pnl = user.global_pnl(use_cache: true)
    if global_pnl
      sign = global_pnl[:percent] >= 0 ? '+' : ''
      lines << "Global P/L: #{sign}#{(global_pnl[:percent] * 100).round(2)}%"
      if global_pnl[:profit_usd]
        profit_sign = global_pnl[:profit_usd] >= 0 ? '+' : ''
        lines << "Profit (USD): #{profit_sign}$#{global_pnl[:profit_usd].round(2)}"
      end
    else
      lines << 'Global P/L: Not available (needs market data)'
    end

    lines << ''
    lines << '--- Per-Bot Summary ---'

    bots.each do |bot|
      pair = if bot.dca_dual_asset?
               "#{bot.base0_asset&.symbol}+#{bot.base1_asset&.symbol}/#{bot.quote_asset&.symbol}"
             elsif bot.respond_to?(:base_asset) && bot.base_asset
               "#{bot.base_asset.symbol}/#{bot.quote_asset&.symbol}"
             else
               'N/A'
             end

      begin
        metrics = bot.metrics
        if metrics.present? && metrics[:pnl]
          pnl_sign = metrics[:pnl] >= 0 ? '+' : ''
          invested = metrics[:total_quote_amount_invested]&.round(2)
          pnl_pct = "#{pnl_sign}#{(metrics[:pnl] * 100).round(2)}%"
          lines << "- #{bot.label} (#{pair}) | #{bot.status} | P/L: #{pnl_pct} | Invested: #{invested} #{bot.quote_asset&.symbol}"
        else
          lines << "- #{bot.label} (#{pair}) | #{bot.status} | No metrics yet"
        end
      rescue StandardError
        lines << "- #{bot.label} (#{pair}) | #{bot.status} | Metrics unavailable"
      end
    end

    render text: lines.join("\n")
  end
end
